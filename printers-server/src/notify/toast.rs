//! The printing changes worth interrupting someone for.

use cosmic_settings_printers_core::{JobState, PrinterEntry};
use std::collections::HashMap;

use super::events::Notification;

/// What each job was last reported as, because a resubscribe replays endings.
#[derive(Default)]
pub(super) struct Jobs(HashMap<i32, i32>);

impl Jobs {
    /// Announces a job that reached the end of its life, one way or the other.
    pub(super) fn announce_end(&mut self, notification: &Notification) {
        if !notification.event.starts_with("job-") {
            return;
        }

        // Cancelling is the one ending the user already knows about.
        let state = crate::cups::job_state(notification.job_state);
        let completed = matches!(state, JobState::Completed);
        let summary = match state {
            JobState::Completed => "Printing complete",
            JobState::Stopped => "Printing stopped",
            JobState::Aborted | JobState::Failed => "Printing failed",
            _ => return,
        };

        if self.0.insert(notification.job_id, notification.job_state)
            == Some(notification.job_state)
        {
            return;
        }

        let document = if notification.job_name.is_empty() {
            "Document"
        } else {
            &notification.job_name
        };
        let mut body = format!("{document} on {}", notification.printer);
        if !completed && let Some(reason) = reason_phrase(&notification.reasons) {
            body.push_str(" — ");
            body.push_str(reason);
        }

        let icon = if completed {
            "printer-symbolic"
        } else {
            "printer-error-symbolic"
        };
        show(summary, body, icon);
    }
}

/// Announces a printer that turned up after the first enumeration.
pub(crate) fn printer_available(printer: &PrinterEntry) {
    let body = match printer.location() {
        Some(location) if !location.trim().is_empty() => {
            format!("{} — {location}", printer.name())
        }
        _ => printer.name().to_string(),
    };

    show("Printer available", body, "printer-symbolic");
}

/// Announces a printer that is no longer offered.
pub(crate) fn printer_removed(printer_name: &str) {
    show(
        "Printer removed",
        printer_name.to_string(),
        "printer-symbolic",
    );
}

/// The enumeration calls this from threads with no runtime to spawn onto,
/// so it takes one of its own.
fn show(summary: &'static str, body: String, icon: &'static str) {
    std::thread::spawn(move || {
        let shown = notify_rust::Notification::new()
            .appname("")
            .summary(summary)
            .body(&body)
            .icon(icon)
            .show();

        // A session with no notification daemon is not a printing problem.
        if let Err(error) = shown {
            tracing::debug!(summary, error = %error, "could not show a notification");
        }
    });
}

/// The first reason worth reading, if the event carried one.
fn reason_phrase(reasons: &str) -> Option<&'static str> {
    reasons
        .split([',', ' '])
        .filter_map(|reason| {
            // Servers append a severity to most reasons.
            let reason = reason
                .trim()
                .trim_end_matches("-error")
                .trim_end_matches("-warning")
                .trim_end_matches("-report");

            Some(match reason {
                "media-empty" | "media-needed" => "out of paper",
                "media-jam" => "paper jam",
                "cover-open" | "door-open" => "cover is open",
                "marker-supply-empty" | "toner-empty" => "out of toner",
                "marker-supply-low" | "toner-low" => "low on toner",
                "offline" | "shutdown" => "offline",
                _ => return None,
            })
        })
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: i32, event: &str, state: i32) -> Notification {
        Notification {
            event: event.to_string(),
            printer: "Acme_Laser".to_string(),
            job_id: id,
            job_name: "report.pdf".to_string(),
            job_state: state,
            ..Default::default()
        }
    }

    #[test]
    fn a_recognized_reason_reads_as_a_phrase() {
        assert_eq!(reason_phrase("media-empty-warning"), Some("out of paper"));
        assert_eq!(
            reason_phrase("none,marker-supply-low-report"),
            Some("low on toner")
        );
        assert_eq!(reason_phrase("media-jam"), Some("paper jam"));
    }

    #[test]
    fn an_unrecognized_reason_is_left_unsaid() {
        assert_eq!(reason_phrase("none"), None);
        assert_eq!(reason_phrase(""), None);
    }

    #[test]
    fn a_job_is_announced_once_for_one_ending() {
        let mut jobs = Jobs::default();

        jobs.announce_end(&job(7, "job-completed", 9));
        assert_eq!(jobs.0.get(&7), Some(&9));

        // The same ending again, as a resubscribe replays it.
        jobs.announce_end(&job(7, "job-completed", 9));
        assert_eq!(jobs.0.len(), 1);
    }

    #[test]
    fn a_job_still_printing_is_not_announced() {
        let mut jobs = Jobs::default();

        jobs.announce_end(&job(7, "job-progress", 5));
        jobs.announce_end(&job(7, "job-created", 3));

        assert!(jobs.0.is_empty());
    }

    #[test]
    fn a_cancelled_job_is_not_announced() {
        let mut jobs = Jobs::default();

        jobs.announce_end(&job(7, "job-state-changed", 7));

        assert!(jobs.0.is_empty());
    }

    #[test]
    fn a_printer_event_is_not_taken_for_a_job() {
        let mut jobs = Jobs::default();

        jobs.announce_end(&job(0, "printer-state-changed", 9));

        assert!(jobs.0.is_empty());
    }
}
