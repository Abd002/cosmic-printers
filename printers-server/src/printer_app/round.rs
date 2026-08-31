//! Add Printer discovery, organized into generations.

use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, AddPrinterDiscoveryState, DiscoveredPhysicalPrinter,
    IdentityConfidenceKind, PaCandidateState, PrinterApplicationCandidateSummary,
    PrinterApplicationScanState, PrinterApplicationScanStatus,
};
use std::collections::BTreeMap;
use std::time::Instant;

use super::drivers::PaDriverMatch;
use super::identity::{PaConfigurationCandidate, PhysicalPrinter, group_candidates};

/// Identifies one round of discovery.
pub(crate) type DiscoveryGeneration = u64;

/// Distinguishes failed scans from valid empty or unsupported answers.
fn scan_broke(state: PrinterApplicationScanState) -> bool {
    matches!(
        state,
        PrinterApplicationScanState::Unreachable | PrinterApplicationScanState::Failed
    )
}

/// Preserves a row identifier while any of its candidates remain in that row.
fn keep_established_identifiers(established: &[PhysicalPrinter], rebuilt: &mut [PhysicalPrinter]) {
    let taken = rebuilt
        .iter()
        .map(|printer| printer.id.clone())
        .collect::<std::collections::HashSet<_>>();

    for printer in rebuilt.iter_mut() {
        let already = established.iter().find(|before| {
            before.id != printer.id
                && before
                    .candidates
                    .iter()
                    .any(|candidate| printer.candidates.iter().any(|now| now.id == candidate.id))
        });

        if let Some(already) = already
            && !taken.contains(&already.id)
        {
            printer.id = already.id.clone();
        }
    }
}

/// What one Printer Application reported in one generation.
#[derive(Debug)]
pub(crate) struct PrinterApplicationDeviceSnapshot {
    pub(crate) printer_application_id: String,
    pub(crate) printer_application_name: String,
    pub(crate) state: PrinterApplicationScanState,
    pub(crate) candidates: Vec<PaConfigurationCandidate>,
    /// How many device collections were unusable, so a misbehaving application
    /// can be diagnosed rather than appearing to have found fewer printers.
    pub(crate) quarantined: usize,
}

impl PrinterApplicationDeviceSnapshot {
    /// Creates a snapshot for an application that has not been asked yet.
    pub(crate) fn pending(
        printer_application_id: String,
        printer_application_name: String,
    ) -> Self {
        Self {
            printer_application_id,
            printer_application_name,
            state: PrinterApplicationScanState::Pending,
            candidates: Vec::new(),
            quarantined: 0,
        }
    }

    fn is_finished(&self) -> bool {
        !matches!(
            self.state,
            PrinterApplicationScanState::Pending | PrinterApplicationScanState::Searching
        )
    }

    fn failed(&self) -> bool {
        matches!(
            self.state,
            PrinterApplicationScanState::AuthenticationRequired
                | PrinterApplicationScanState::Unreachable
                | PrinterApplicationScanState::Unsupported
                | PrinterApplicationScanState::Failed
        )
    }

    fn status(&self) -> PrinterApplicationScanStatus {
        PrinterApplicationScanStatus {
            printer_application_id: self.printer_application_id.clone(),
            printer_application_name: self.printer_application_name.clone(),
            state: self.state,
        }
    }
}

/// The state of Add Printer discovery.
#[derive(Debug)]
pub(crate) struct AddPrinterDiscovery {
    generation: DiscoveryGeneration,
    state: AddPrinterDiscoveryState,
    snapshots: BTreeMap<String, PrinterApplicationDeviceSnapshot>,
    physical_printers: Vec<PhysicalPrinter>,
    /// Rows from the previous generation, shown as context while a new round runs.
    previous_physical_printers: Vec<PhysicalPrinter>,
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
}

impl Default for AddPrinterDiscovery {
    fn default() -> Self {
        Self {
            generation: 0,
            state: AddPrinterDiscoveryState::Idle,
            snapshots: BTreeMap::new(),
            physical_printers: Vec::new(),
            previous_physical_printers: Vec::new(),
            started_at: None,
            completed_at: None,
        }
    }
}

impl AddPrinterDiscovery {
    /// Returns the current generation.
    pub(crate) fn generation(&self) -> DiscoveryGeneration {
        self.generation
    }

    /// Starts a generation and immediately makes previous results unselectable.
    pub(crate) fn start(&mut self, applications: Vec<(String, String)>) -> DiscoveryGeneration {
        self.generation += 1;
        self.state = if applications.is_empty() {
            AddPrinterDiscoveryState::Complete
        } else {
            AddPrinterDiscoveryState::Searching
        };
        // Keep prior rows as cached until each application supplies a replacement.
        let mut carried = std::mem::take(&mut self.snapshots);
        self.snapshots = applications
            .into_iter()
            .map(|(id, name)| {
                let mut snapshot = PrinterApplicationDeviceSnapshot::pending(id.clone(), name);
                if let Some(before) = carried.remove(&id) {
                    snapshot.candidates = before.candidates;
                }
                (id, snapshot)
            })
            .collect();
        self.previous_physical_printers = std::mem::take(&mut self.physical_printers);
        self.started_at = Some(Instant::now());
        self.completed_at = if self.state == AddPrinterDiscoveryState::Complete {
            Some(Instant::now())
        } else {
            None
        };

        self.generation
    }

    /// Adds a late-resolving application to the active round.
    /// Otherwise DNS-SD timing would make it invisible until the next round.
    pub(crate) fn join(
        &mut self,
        application_id: String,
        name: String,
    ) -> Option<DiscoveryGeneration> {
        if self.state == AddPrinterDiscoveryState::Idle
            || self.snapshots.contains_key(&application_id)
        {
            return None;
        }

        self.snapshots.insert(
            application_id.clone(),
            PrinterApplicationDeviceSnapshot::pending(application_id, name),
        );
        self.recompute();

        Some(self.generation)
    }

    /// Records a scan only for the current generation.
    pub(crate) fn mark_searching(
        &mut self,
        generation: DiscoveryGeneration,
        application_id: &str,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(snapshot) = self.snapshots.get_mut(application_id) else {
            return false;
        };
        if snapshot.state == PrinterApplicationScanState::Searching {
            return false;
        }
        snapshot.state = PrinterApplicationScanState::Searching;

        true
    }

    /// Replaces one application's results.
    pub(crate) fn replace_snapshot(
        &mut self,
        generation: DiscoveryGeneration,
        application_id: &str,
        state: PrinterApplicationScanState,
        candidates: Vec<PaConfigurationCandidate>,
        quarantined: usize,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(snapshot) = self.snapshots.get_mut(application_id) else {
            return false;
        };

        // A failed scan keeps the application's prior cached observations.
        let candidates = if candidates.is_empty() && scan_broke(state) {
            if !snapshot.candidates.is_empty() {
                tracing::debug!(
                    application_id,
                    ?state,
                    devices = snapshot.candidates.len(),
                    "keeping what this application last reported, since the scan broke"
                );
            }
            std::mem::take(&mut snapshot.candidates)
        } else {
            candidates
        };

        let unchanged = snapshot.state == state
            && snapshot.quarantined == quarantined
            && snapshot.candidates.len() == candidates.len()
            && snapshot
                .candidates
                .iter()
                .zip(&candidates)
                .all(|(existing, incoming)| {
                    existing.id == incoming.id
                        && existing.driver_match == incoming.driver_match
                        && existing.endpoints.len() == incoming.endpoints.len()
                });
        if unchanged {
            return false;
        }

        snapshot.state = state;
        snapshot.candidates = candidates;
        snapshot.quarantined = quarantined;
        self.recompute();

        true
    }

    /// Drops an application that is no longer advertised.
    pub(crate) fn remove_application(&mut self, application_id: &str) -> bool {
        if self.snapshots.remove(application_id).is_none() {
            return false;
        }
        self.recompute();

        true
    }

    /// Rebuilds the physical printer rows from every application's candidates.
    fn recompute(&mut self) {
        let candidates = self
            .snapshots
            .values()
            .flat_map(|snapshot| snapshot.candidates.iter().cloned())
            .collect::<Vec<_>>();
        let mut printers = group_candidates(candidates);
        keep_established_identifiers(&self.physical_printers, &mut printers);
        self.physical_printers = printers;

        let finished = self
            .snapshots
            .values()
            .all(PrinterApplicationDeviceSnapshot::is_finished);
        let any_failed = self
            .snapshots
            .values()
            .any(PrinterApplicationDeviceSnapshot::failed);

        self.state = match (finished, any_failed) {
            (false, _) => AddPrinterDiscoveryState::Searching,
            (true, false) => AddPrinterDiscoveryState::Complete,
            (true, true) => AddPrinterDiscoveryState::CompleteWithErrors,
        };
        self.completed_at = finished.then(Instant::now);
    }

    /// Builds a reply that may display, but never configure, cached rows.
    pub(crate) fn reply(&self) -> AddPrinterDiscoveryReply {
        let cached =
            self.physical_printers.is_empty() && !self.previous_physical_printers.is_empty();
        let printers = if cached {
            &self.previous_physical_printers
        } else {
            &self.physical_printers
        };

        AddPrinterDiscoveryReply {
            generation: self.generation,
            state: self.state,
            physical_printers: printers.iter().map(|printer| self.row(printer)).collect(),
            completed_printer_application_scans: self
                .snapshots
                .values()
                .filter(|snapshot| snapshot.is_finished())
                .count() as u32,
            total_printer_application_scans: self.snapshots.len() as u32,
            any_printer_application_failed: self
                .snapshots
                .values()
                .any(PrinterApplicationDeviceSnapshot::failed),
            printer_application_scans: self
                .snapshots
                .values()
                .map(PrinterApplicationDeviceSnapshot::status)
                .collect(),
            cached,
        }
    }

    fn row(&self, printer: &PhysicalPrinter) -> DiscoveredPhysicalPrinter {
        DiscoveredPhysicalPrinter {
            id: printer.id.clone(),
            display_name: printer.display_name.clone(),
            make_and_model: printer.make_and_model.clone(),
            candidates: printer
                .candidates
                .iter()
                .map(|candidate| PrinterApplicationCandidateSummary {
                    id: candidate.id.clone(),
                    printer_application_id: candidate.printer_application_id.clone(),
                    printer_application_name: self
                        .snapshots
                        .get(&candidate.printer_application_id)
                        .map(|snapshot| snapshot.printer_application_name.clone())
                        .unwrap_or_else(|| candidate.printer_application_id.clone()),
                    state: candidate_state(&candidate.driver_match),
                })
                .collect(),
            identity_confidence: IdentityConfidenceKind::from(printer.identity.confidence()),
        }
    }

    /// Finds a candidate only in its named row and current generation.
    pub(crate) fn resolve<'a>(
        &'a self,
        generation: DiscoveryGeneration,
        physical_printer_id: &str,
        candidate_id: &str,
    ) -> Result<&'a PaConfigurationCandidate, ResolveError> {
        if self.state == AddPrinterDiscoveryState::Idle {
            return Err(ResolveError::NotStarted);
        }
        if generation != self.generation {
            return Err(ResolveError::Expired {
                generation: self.generation,
            });
        }

        let printer = self
            .physical_printers
            .iter()
            .find(|printer| printer.id == physical_printer_id)
            .ok_or(ResolveError::PrinterNotFound)?;

        printer
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .ok_or(ResolveError::CandidateNotFound)
    }

    /// Returns whether a device is still reported by the application that owns a
    /// candidate.
    pub(crate) fn candidate_is_current(&self, candidate_id: &str) -> bool {
        self.snapshots.values().any(|snapshot| {
            snapshot
                .candidates
                .iter()
                .any(|candidate| candidate.id == candidate_id)
        })
    }
}

/// Why a selection could not be resolved.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResolveError {
    NotStarted,
    Expired { generation: DiscoveryGeneration },
    PrinterNotFound,
    CandidateNotFound,
}

/// Maps a driver match to what the user is told about a candidate.
fn candidate_state(driver_match: &PaDriverMatch) -> PaCandidateState {
    match driver_match {
        PaDriverMatch::Supported { .. } => PaCandidateState::Ready,
        PaDriverMatch::AlreadyConfigured { .. } => PaCandidateState::AlreadyConfigured,
        PaDriverMatch::Unsupported => PaCandidateState::Unsupported,
        PaDriverMatch::AuthenticationRequired => PaCandidateState::AuthenticationRequired,
        PaDriverMatch::Unavailable => PaCandidateState::Unavailable,
        PaDriverMatch::Unchecked | PaDriverMatch::MalformedResponse => {
            PaCandidateState::DriverUnknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer_app::devices::DeviceTransport;
    use crate::printer_app::drivers::PaDriver;
    use crate::printer_app::identity::PaDeviceEndpoint;
    use cosmic_settings_printers_core::{DeviceId, PhysicalDeviceEvidence};

    fn candidate(
        application: &str,
        serial: &str,
        driver_match: PaDriverMatch,
    ) -> PaConfigurationCandidate {
        let device_id = DeviceId::parse(&format!("MFG:Acme;MDL:Test Laser;SN:{serial};"));
        let identity = PhysicalDeviceEvidence::from_device_id(&device_id);

        PaConfigurationCandidate {
            id: format!("{application}:1:0"),
            printer_application_id: application.to_string(),
            endpoints: vec![PaDeviceEndpoint {
                device_uri: format!("socket://192.0.2.10:9100/{serial}"),
                transport: DeviceTransport::Socket,
                preference: DeviceTransport::Socket.preference(),
            }],
            identity,
            display_name: "Acme Test Laser".into(),
            make_and_model: Some("Acme Test Laser".into()),
            device_id: Some(device_id.raw().to_string()),
            driver_match,
        }
    }

    fn supported() -> PaDriverMatch {
        PaDriverMatch::Supported {
            drivers: vec![PaDriver {
                id: "acme-laser".into(),
                display_name: "Acme Laser".into(),
                supported_device_id: None,
            }],
        }
    }

    fn discovery_with_two_applications() -> AddPrinterDiscovery {
        let mut discovery = AddPrinterDiscovery::default();
        discovery.start(vec![
            ("pa-a".to_string(), "LPrint".to_string()),
            ("pa-b".to_string(), "PostScript".to_string()),
        ]);
        discovery
    }

    #[test]
    fn each_round_gets_a_new_generation() {
        let mut discovery = AddPrinterDiscovery::default();

        assert_eq!(discovery.generation(), 0);
        assert_eq!(discovery.start(vec![("pa-a".into(), "LPrint".into())]), 1);
        assert_eq!(discovery.start(vec![("pa-a".into(), "LPrint".into())]), 2);
    }

    #[test]
    fn a_round_with_no_applications_is_complete_immediately() {
        let mut discovery = AddPrinterDiscovery::default();
        discovery.start(Vec::new());

        let reply = discovery.reply();
        assert_eq!(reply.state, AddPrinterDiscoveryState::Complete);
        assert_eq!(reply.total_printer_application_scans, 0);
        assert!(reply.physical_printers.is_empty());
    }

    #[test]
    fn an_application_arriving_late_joins_the_round_that_found_nothing() {
        let mut discovery = AddPrinterDiscovery::default();
        let generation = discovery.start(Vec::new());

        assert_eq!(
            discovery.join("pa-a".into(), "LPrint".into()),
            Some(generation)
        );

        let reply = discovery.reply();
        assert_eq!(reply.generation, generation);
        assert_eq!(reply.state, AddPrinterDiscoveryState::Searching);
        assert_eq!(reply.total_printer_application_scans, 1);
    }

    #[test]
    fn joining_keeps_the_generation_and_the_rows_already_found() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();
        discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        );

        assert_eq!(
            discovery.join("pa-c".into(), "Gutenprint".into()),
            Some(generation)
        );

        let reply = discovery.reply();
        assert_eq!(reply.generation, generation);
        assert_eq!(reply.physical_printers.len(), 1);
        assert_eq!(reply.total_printer_application_scans, 3);
    }

    #[test]
    fn an_application_already_in_the_round_does_not_join_twice() {
        let mut discovery = discovery_with_two_applications();

        assert_eq!(discovery.join("pa-a".into(), "LPrint".into()), None);
        assert_eq!(discovery.reply().total_printer_application_scans, 2);
    }

    #[test]
    fn an_application_does_not_join_before_any_round_has_run() {
        let mut discovery = AddPrinterDiscovery::default();

        assert_eq!(discovery.join("pa-a".into(), "LPrint".into()), None);
        assert_eq!(discovery.reply().state, AddPrinterDiscoveryState::Idle);
    }

    #[test]
    fn results_appear_as_each_application_answers() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();

        assert!(discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        ));

        let reply = discovery.reply();
        assert_eq!(reply.state, AddPrinterDiscoveryState::Searching);
        assert_eq!(reply.completed_printer_application_scans, 1);
        assert_eq!(reply.total_printer_application_scans, 2);
        assert_eq!(reply.physical_printers.len(), 1);
        assert!(!reply.cached);
    }

    #[test]
    fn two_applications_finding_one_printer_produce_one_row_with_two_candidates() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();

        discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        );
        discovery.replace_snapshot(
            generation,
            "pa-b",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-b", "ABC123", supported())],
            0,
        );

        let reply = discovery.reply();
        assert_eq!(reply.state, AddPrinterDiscoveryState::Complete);
        assert_eq!(reply.physical_printers.len(), 1);
        assert_eq!(reply.physical_printers[0].candidates.len(), 2);
        assert_eq!(
            reply.physical_printers[0].candidates[0].printer_application_name,
            "LPrint"
        );
    }

    #[test]
    fn one_failure_does_not_hide_the_other_results() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();

        discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        );
        discovery.replace_snapshot(
            generation,
            "pa-b",
            PrinterApplicationScanState::Unreachable,
            Vec::new(),
            0,
        );

        let reply = discovery.reply();
        assert_eq!(reply.state, AddPrinterDiscoveryState::CompleteWithErrors);
        assert!(reply.any_printer_application_failed);
        assert_eq!(reply.physical_printers.len(), 1);
    }

    #[test]
    fn an_identical_result_emits_no_further_change() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();
        let candidates = vec![candidate("pa-a", "ABC123", supported())];

        assert!(discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            candidates.clone(),
            0,
        ));
        assert!(!discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            candidates,
            0,
        ));
    }

    #[test]
    fn a_task_from_an_abandoned_round_cannot_write_results() {
        let mut discovery = discovery_with_two_applications();
        let stale = discovery.generation();
        discovery.start(vec![("pa-a".into(), "LPrint".into())]);

        assert!(!discovery.replace_snapshot(
            stale,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        ));
        assert!(!discovery.mark_searching(stale, "pa-a"));
    }

    #[test]
    fn previous_rows_are_shown_as_cached_until_fresh_ones_arrive() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();
        discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        );

        discovery.start(vec![("pa-a".into(), "LPrint".into())]);

        let reply = discovery.reply();
        assert!(reply.cached);
        assert_eq!(reply.physical_printers.len(), 1);
    }

    #[test]
    fn a_cached_row_cannot_be_selected() {
        let mut discovery = discovery_with_two_applications();
        let first = discovery.generation();
        discovery.replace_snapshot(
            first,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        );
        let row = discovery.reply().physical_printers[0].clone();

        let second = discovery.start(vec![("pa-a".into(), "LPrint".into())]);

        assert_eq!(
            discovery
                .resolve(second, &row.id, &row.candidates[0].id)
                .err(),
            Some(ResolveError::PrinterNotFound)
        );
    }

    #[test]
    fn a_stale_generation_is_refused() {
        let mut discovery = discovery_with_two_applications();
        let stale = discovery.generation();
        discovery.replace_snapshot(
            stale,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "ABC123", supported())],
            0,
        );
        let row = discovery.reply().physical_printers[0].clone();
        let current = discovery.start(vec![("pa-a".into(), "LPrint".into())]);

        assert_eq!(
            discovery
                .resolve(stale, &row.id, &row.candidates[0].id)
                .err(),
            Some(ResolveError::Expired {
                generation: current
            })
        );
    }

    #[test]
    fn a_candidate_from_another_row_is_refused() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();
        discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![
                candidate("pa-a", "FIRST", supported()),
                candidate("pa-b", "SECOND", supported()),
            ],
            0,
        );

        let reply = discovery.reply();
        assert_eq!(reply.physical_printers.len(), 2);
        let first = &reply.physical_printers[0];
        let second = &reply.physical_printers[1];

        assert_eq!(
            discovery
                .resolve(generation, &first.id, &second.candidates[0].id)
                .err(),
            Some(ResolveError::CandidateNotFound)
        );
    }

    #[test]
    fn resolving_before_discovery_starts_says_so() {
        let discovery = AddPrinterDiscovery::default();

        assert_eq!(
            discovery
                .resolve(0, "physical:serial:ABC123", "pa-a:1:0")
                .err(),
            Some(ResolveError::NotStarted)
        );
    }

    #[test]
    fn an_application_that_disappears_takes_only_its_own_candidates() {
        let mut discovery = discovery_with_two_applications();
        let generation = discovery.generation();
        discovery.replace_snapshot(
            generation,
            "pa-a",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-a", "FIRST", supported())],
            0,
        );
        discovery.replace_snapshot(
            generation,
            "pa-b",
            PrinterApplicationScanState::Complete,
            vec![candidate("pa-b", "SECOND", supported())],
            0,
        );
        assert_eq!(discovery.reply().physical_printers.len(), 2);

        assert!(discovery.remove_application("pa-a"));

        let reply = discovery.reply();
        assert_eq!(reply.physical_printers.len(), 1);
        assert_eq!(reply.total_printer_application_scans, 1);
        assert!(discovery.candidate_is_current("pa-b:1:0"));
        assert!(!discovery.candidate_is_current("pa-a:1:0"));
    }

    #[test]
    fn candidate_states_reflect_driver_support() {
        assert_eq!(candidate_state(&supported()), PaCandidateState::Ready);
        assert_eq!(
            candidate_state(&PaDriverMatch::Unsupported),
            PaCandidateState::Unsupported
        );
        assert_eq!(
            candidate_state(&PaDriverMatch::MalformedResponse),
            PaCandidateState::DriverUnknown
        );
    }
}
