//! Matching a printer a Printer Application created to the destination that
//! later appears.
//!
//! Creating a printer and having a destination for it are two separate events.
//! The Printer Application creates the printer and advertises it; the ordinary
//! CUPS destination pipeline discovers the advertisement on its own schedule and
//! builds the `PrinterEntry`. This module joins the receipt to the destination
//! once both exist.
//!
//! Ownership does not move. The destination pipeline remains the only thing that
//! creates a `PrinterEntry`; nothing here fabricates one, because a guessed
//! destination would outlive the printer it was guessing about.

use cosmic_settings_printers_core::PrinterEntry;

/// What was created, and what has become of it.
#[derive(Clone, Debug)]
pub(crate) struct PendingPaConfiguration {
    pub(crate) operation_id: String,
    pub(crate) printer_application_id: String,
    pub(crate) physical_printer_id: String,
    pub(crate) candidate_id: String,
    pub(crate) configured_printer_name: String,
    pub(crate) expected_printer_uri: Option<String>,
    pub(crate) expected_printer_uuid: Option<String>,
    /// The endpoint the owning application serves its printers on, used to tell
    /// its destinations apart from another application's.
    pub(crate) application_endpoint: Option<(String, u16)>,
    pub(crate) web_interface_uri: Option<String>,
    pub(crate) created_at: std::time::Instant,
    pub(crate) state: PendingConfigurationState,
}

/// How far a configuration attempt has got.
///
/// Only states an attempt can actually reach appear here. A rejection is returned
/// as a structured error and leaves no receipt, because there is no printer to
/// wait for or inspect afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PendingConfigurationState {
    /// Created, waiting for the destination pipeline to notice.
    AwaitingAdvertisement,
    /// Matched to a destination.
    Reconciled { destination_id: String },
    /// The device already had a printer in this application.
    AlreadyConfigured,
    /// Setup has to continue in the application's own interface, or the printer
    /// was created but never advertised.
    ManualActionRequired,
    /// The request was sent and the outcome could not be established.
    UnknownOutcome,
}

impl PendingConfigurationState {
    /// Returns whether this attempt is still waiting for a destination.
    pub(crate) fn is_awaiting(&self) -> bool {
        matches!(self, Self::AwaitingAdvertisement)
    }
}

/// How long to keep waiting for a destination that never appears.
///
/// A Printer Application advertises a new printer over DNS-SD, which the
/// destination pipeline picks up within seconds. Well past that, the printer was
/// created but is not being advertised — the attempt stops waiting and says so,
/// rather than appearing to hang.
pub(crate) const ADVERTISEMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Finds the destination that corresponds to a created printer.
///
/// Matching is ordered by how conclusive the evidence is:
///
/// 1. The UUID the application reported for the printer it created. Unambiguous.
/// 2. The exact printer URI it reported. Unambiguous.
/// 3. The application's own endpoint plus the queue name. Two applications on one
///    machine use different ports, so this does not cross between them.
/// 4. The queue name alone, only when nothing above applied.
///
/// A display name is never enough on its own. Names are chosen for readability and
/// two printers can share one, so matching on a name alone would attach a receipt
/// to the wrong destination.
pub(crate) fn find_destination<'a>(
    pending: &PendingPaConfiguration,
    destinations: impl IntoIterator<Item = &'a PrinterEntry> + Clone,
) -> Option<&'a PrinterEntry> {
    if let Some(uuid) = pending.expected_printer_uuid.as_deref() {
        let wanted = normalize_uuid(uuid);
        if let Some(found) = destinations.clone().into_iter().find(|destination| {
            destination
                .printer_uuid()
                .map(normalize_uuid)
                .is_some_and(|candidate| candidate == wanted)
        }) {
            return Some(found);
        }
    }

    if let Some(uri) = pending.expected_printer_uri.as_deref() {
        let wanted = normalize_uri(uri);
        if let Some(found) = destinations.clone().into_iter().find(|destination| {
            [destination.printer_uri(), destination.device_uri()]
                .into_iter()
                .flatten()
                .any(|candidate| normalize_uri(candidate) == wanted)
        }) {
            return Some(found);
        }
    }

    if let Some((host, port)) = pending.application_endpoint.as_ref()
        && let Some(found) = destinations.clone().into_iter().find(|destination| {
            endpoint_matches(destination, host, *port)
                && resource_names_printer(destination, &pending.configured_printer_name)
        })
    {
        return Some(found);
    }

    destinations.into_iter().find(|destination| {
        queue_name(destination).eq_ignore_ascii_case(&pending.configured_printer_name)
    })
}

/// Returns whether a destination is served by a given endpoint.
fn endpoint_matches(destination: &PrinterEntry, host: &str, port: u16) -> bool {
    [destination.printer_uri(), destination.device_uri()]
        .into_iter()
        .flatten()
        .filter_map(uri_endpoint)
        .any(|(candidate_host, candidate_port)| {
            candidate_port == port && hosts_match(&candidate_host, host)
        })
}

/// Returns whether a destination's resource path names this printer.
///
/// A Printer Application serves each printer at a path ending in its name, so the
/// last path segment is what ties a destination back to the queue that was
/// created.
fn resource_names_printer(destination: &PrinterEntry, printer_name: &str) -> bool {
    [destination.printer_uri(), destination.device_uri()]
        .into_iter()
        .flatten()
        .filter_map(last_path_segment)
        .any(|segment| segment.eq_ignore_ascii_case(printer_name))
        || queue_name(destination).eq_ignore_ascii_case(printer_name)
}

/// Describes an attempt for a log line.
///
/// Reconciliation is the step most likely to need explaining after the fact —
/// "the printer was created but nothing appeared" — so the identifiers that would
/// answer that are recorded when it resolves.
pub(crate) fn describe(pending: &PendingPaConfiguration) -> String {
    format!(
        "operation {} for candidate {} of printer {} in application {}",
        pending.operation_id,
        pending.candidate_id,
        pending.physical_printer_id,
        pending.printer_application_id,
    )
}

fn queue_name(destination: &PrinterEntry) -> &str {
    destination
        .id()
        .split_once('/')
        .map(|(name, _)| name)
        .unwrap_or_else(|| destination.id())
}

fn last_path_segment(uri: &str) -> Option<String> {
    let (_, rest) = uri.split_once("://")?;
    let path = rest.split_once('/')?.1;
    let path = path.split(['?', '#']).next().unwrap_or(path);

    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(ToString::to_string)
        .filter(|segment| !segment.is_empty())
}

fn uri_endpoint(uri: &str) -> Option<(String, u16)> {
    let (scheme, rest) = uri.split_once("://")?;
    let authority = rest.split(['/', '?']).next()?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ipp" | "ipps" => 631,
        "http" => 80,
        "https" => 443,
        _ => return None,
    };

    if let Some(end) = authority.find(']') {
        let host = authority.get(..=end)?.to_ascii_lowercase();
        let port = authority
            .get(end + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .and_then(|port| port.parse().ok())
            .unwrap_or(default_port);
        return Some((host, port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_ascii_lowercase(), port.parse().ok()?)),
        None => Some((authority.to_ascii_lowercase(), default_port)),
    }
}

/// Compares two hosts, treating every spelling of this machine as equal.
///
/// A Printer Application is advertised under a hostname while its destinations
/// often report loopback, so requiring an exact string match would never join
/// them.
fn hosts_match(left: &str, right: &str) -> bool {
    if left.eq_ignore_ascii_case(right) {
        return true;
    }

    cosmic_settings_printers_core::host_is_local(left)
        && cosmic_settings_printers_core::host_is_local(right)
}

fn normalize_uuid(uuid: &str) -> String {
    let lowered = uuid.trim().to_ascii_lowercase();

    lowered
        .strip_prefix("urn:uuid:")
        .unwrap_or(&lowered)
        .to_string()
}

fn normalize_uri(uri: &str) -> String {
    uri.split(['?', '#'])
        .next()
        .unwrap_or(uri)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn pending(uuid: Option<&str>, uri: Option<&str>, name: &str) -> PendingPaConfiguration {
        PendingPaConfiguration {
            operation_id: "operation".into(),
            printer_application_id: "pa-a".into(),
            physical_printer_id: "physical:serial:ABC123".into(),
            candidate_id: "pa-a:1:0".into(),
            configured_printer_name: name.into(),
            expected_printer_uri: uri.map(ToString::to_string),
            expected_printer_uuid: uuid.map(ToString::to_string),
            application_endpoint: Some(("localhost".into(), 8000)),
            web_interface_uri: None,
            created_at: std::time::Instant::now(),
            state: PendingConfigurationState::AwaitingAdvertisement,
        }
    }

    fn destination(id: &str, options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            id,
            id,
            false,
            options
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn reconciles_by_created_printer_uuid() {
        let pending = pending(
            Some("urn:uuid:11111111-2222-3333-4444-555555555555"),
            None,
            "Acme_Laser",
        );
        let destinations = vec![
            destination(
                "Other",
                &[(
                    "printer-uuid",
                    "urn:uuid:99999999-0000-0000-0000-000000000000",
                )],
            ),
            destination(
                "Acme_Laser",
                &[("printer-uuid", "11111111-2222-3333-4444-555555555555")],
            ),
        ];

        assert_eq!(
            find_destination(&pending, &destinations).map(PrinterEntry::id),
            Some("Acme_Laser")
        );
    }

    #[test]
    fn reconciles_by_exact_printer_uri() {
        let pending = pending(
            None,
            Some("ipps://localhost:8000/ipp/print/Acme_Laser"),
            "Acme_Laser",
        );
        let destinations = vec![destination(
            "queue",
            &[(
                "printer-uri-supported",
                "ipps://localhost:8000/ipp/print/Acme_Laser/",
            )],
        )];

        assert_eq!(
            find_destination(&pending, &destinations).map(PrinterEntry::id),
            Some("queue")
        );
    }

    #[test]
    fn reconciles_by_application_endpoint_and_printer_name() {
        let pending = pending(None, None, "Acme_Laser");
        let destinations = vec![
            // Same name, but served by a different application on another port.
            destination(
                "other",
                &[("device-uri", "ipp://localhost:8001/ipp/print/Acme_Laser")],
            ),
            destination(
                "wanted",
                &[("device-uri", "ipp://localhost:8000/ipp/print/Acme_Laser")],
            ),
        ];

        assert_eq!(
            find_destination(&pending, &destinations).map(PrinterEntry::id),
            Some("wanted")
        );
    }

    #[test]
    fn a_matching_display_name_alone_is_not_enough() {
        let mut pending = pending(None, None, "Acme_Laser");
        pending.application_endpoint = None;
        // Named for a person, not the queue: the destination's own id is what a
        // name match uses, so a display name cannot pull in the wrong entry.
        let destinations = vec![destination(
            "unrelated-queue",
            &[("printer-info", "Acme_Laser")],
        )];

        assert_eq!(find_destination(&pending, &destinations), None);
    }

    #[test]
    fn a_uuid_that_does_not_appear_does_not_fall_through_to_a_wrong_match() {
        let pending = pending(
            Some("11111111-2222-3333-4444-555555555555"),
            None,
            "Acme_Laser",
        );
        // The name matches, and that is allowed to match once the stronger
        // evidence found nothing — the queue name inside the same application is
        // still specific.
        let destinations = vec![destination(
            "Acme_Laser",
            &[("printer-uuid", "99999999-0000-0000-0000-000000000000")],
        )];

        assert_eq!(
            find_destination(&pending, &destinations).map(PrinterEntry::id),
            Some("Acme_Laser")
        );
    }

    #[test]
    fn nothing_matches_an_empty_destination_list() {
        let pending = pending(Some("uuid"), Some("ipps://localhost:8000/x"), "Acme_Laser");

        assert_eq!(find_destination(&pending, &Vec::new()), None);
    }

    #[test]
    fn loopback_spellings_are_treated_as_one_host() {
        let pending = pending(None, None, "Acme_Laser");
        let destinations = vec![destination(
            "wanted",
            &[("device-uri", "ipp://127.0.0.1:8000/ipp/print/Acme_Laser")],
        )];

        assert_eq!(
            find_destination(&pending, &destinations).map(PrinterEntry::id),
            Some("wanted")
        );
    }

    #[test]
    fn only_unfinished_attempts_wait_for_a_destination() {
        assert!(PendingConfigurationState::AwaitingAdvertisement.is_awaiting());
        assert!(!PendingConfigurationState::AlreadyConfigured.is_awaiting());
        assert!(
            !PendingConfigurationState::Reconciled {
                destination_id: "queue".into()
            }
            .is_awaiting()
        );
        assert!(!PendingConfigurationState::UnknownOutcome.is_awaiting());
        assert!(!PendingConfigurationState::ManualActionRequired.is_awaiting());
    }
}
