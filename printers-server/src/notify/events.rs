//! The polling loop, and reading events out of what comes back.

use cups_rs::{IppAttribute, IppOperation, IppRequest, IppResponse, IppTag, IppValueTag};
use std::time::{Duration, Instant};

use super::subscription::{LEASE_SECONDS, Subscription};
use super::{apply, toast};
use crate::error::{BackendError, BackendResult};
use crate::ipp::{CupsResultExt, add_requesting_user, ensure_success, send_on_default_connection};
use crate::state::State;

const GET_NOTIFICATIONS: u16 = 28;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(super) enum Event {
    JobsChanged(String),
    PrinterChanged(String),
    PrinterRemoved(String),
}

pub(super) async fn watch(context: &State) -> BackendResult<()> {
    let subscription = blocking(Subscription::create).await?;
    tracing::debug!(id = subscription.id, "subscribed to IPP notifications");

    // start from a full reading.
    crate::cups::refresh_available_destinations(context.clone());

    let mut next_sequence = 1;
    let mut renewed = Instant::now();
    let renew_after = Duration::from_secs(LEASE_SECONDS as u64 / 2);
    let mut jobs = toast::Jobs::default();

    loop {
        let fetching = subscription.clone();
        let (notifications, highest) = blocking(move || fetch(&fetching, next_sequence)).await?;

        if !notifications.is_empty() {
            tracing::debug!(
                count = notifications.len(),
                "IPP notification events arrived"
            );
        }

        for notification in notifications {
            jobs.announce_end(&notification);
            if let Some(event) = notification.into_event() {
                apply::apply(context, event).await;
            }
        }

        if highest >= next_sequence {
            next_sequence = highest + 1;
        }

        if renewed.elapsed() >= renew_after {
            let renewing = subscription.clone();
            blocking(move || renewing.renew()).await?;
            renewed = Instant::now();
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn blocking<T, F>(work: F) -> BackendResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> BackendResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(BackendError::Join)?
}

fn fetch(subscription: &Subscription, since: i32) -> BackendResult<(Vec<Notification>, i32)> {
    let mut request = IppRequest::new(IppOperation::Other(GET_NOTIFICATIONS)).cups_err()?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            subscription.uri_attribute(),
            subscription.uri(),
        )
        .cups_err()?;
    add_requesting_user(&mut request)?;
    request
        .add_integer(
            IppTag::Operation,
            IppValueTag::Integer,
            "notify-subscription-ids",
            subscription.id,
        )
        .cups_err()?;
    request
        .add_integer(
            IppTag::Operation,
            IppValueTag::Integer,
            "notify-sequence-numbers",
            since,
        )
        .cups_err()?;

    let response = send_on_default_connection(request, subscription.uri())?;
    ensure_success(&response, "Get-Notifications")?;

    let notifications = read_notifications(&response);
    // Counted for ignored events too, because the server sends everything from the
    // sequence number it is given.
    let highest = notifications
        .iter()
        .map(|notification| notification.sequence)
        .max()
        .unwrap_or(0);

    Ok((notifications, highest))
}

/// Events arrive one group each, separated by an attribute with no name.
fn read_notifications(response: &IppResponse) -> Vec<Notification> {
    let mut notifications = Vec::new();
    let mut current: Option<Notification> = None;

    for attribute in response.attributes() {
        let Some(name) = attribute.name() else {
            notifications.extend(current.take());
            continue;
        };

        if attribute.group_tag() != Some(IppTag::EventNotification) {
            continue;
        }

        current
            .get_or_insert_with(Notification::default)
            .read(&name, &attribute);
    }
    notifications.extend(current);

    notifications
}

#[derive(Default)]
pub(super) struct Notification {
    pub(super) event: String,
    pub(super) printer: String,
    pub(super) sequence: i32,
    pub(super) job_id: i32,
    pub(super) job_name: String,
    pub(super) job_state: i32,
    /// The job or the printer reasons, whichever this event carried.
    pub(super) reasons: String,
}

impl Notification {
    fn read(&mut self, name: &str, attribute: &IppAttribute) {
        match name {
            "notify-subscribed-event" => self.event = attribute.get_string(0).unwrap_or_default(),
            "printer-name" => self.printer = attribute.get_string(0).unwrap_or_default(),
            "notify-sequence-number" => self.sequence = attribute.get_integer(0),
            "job-id" => self.job_id = attribute.get_integer(0),
            "job-name" => self.job_name = attribute.get_string(0).unwrap_or_default(),
            "job-state" => self.job_state = attribute.get_integer(0),
            "job-state-reasons" | "printer-state-reasons" => {
                self.reasons = attribute.get_string(0).unwrap_or_default();
            }
            _ => {}
        }
    }

    fn into_event(self) -> Option<Event> {
        if self.printer.is_empty() {
            return None;
        }

        Some(match self.event.as_str() {
            "job-created" | "job-completed" | "job-progress" | "job-state-changed"
            | "job-stopped" => Event::JobsChanged(self.printer),
            "printer-deleted" => Event::PrinterRemoved(self.printer),
            // Servers differ on the rest of the printer event names, and one that is
            // not recognized still means that printer is worth re-reading.
            other if other.starts_with("printer-") => Event::PrinterChanged(self.printer),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(event: &str, printer: &str) -> Notification {
        Notification {
            event: event.to_string(),
            printer: printer.to_string(),
            sequence: 1,
            ..Default::default()
        }
    }

    #[test]
    fn a_job_event_refreshes_the_printer_that_owns_it() {
        assert!(matches!(
            notification("job-completed", "Acme_Laser").into_event(),
            Some(Event::JobsChanged(printer)) if printer == "Acme_Laser"
        ));
    }

    #[test]
    fn a_deleted_printer_is_not_re_read() {
        assert!(matches!(
            notification("printer-deleted", "Acme_Laser").into_event(),
            Some(Event::PrinterRemoved(_))
        ));
    }

    #[test]
    fn an_unknown_printer_event_still_re_reads_the_printer() {
        assert!(matches!(
            notification("printer-media-changed", "Acme_Laser").into_event(),
            Some(Event::PrinterChanged(_))
        ));
    }

    #[test]
    fn an_event_without_a_name_is_skipped() {
        assert!(notification("", "Acme_Laser").into_event().is_none());
    }

    #[test]
    fn an_event_without_a_printer_is_skipped() {
        assert!(notification("job-created", "").into_event().is_none());
    }
}
