//! One round of Add Printer discovery, and which generation it is.

use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, PrinterApplication, PrinterApplicationScanState,
    PrinterApplicationState,
};

use super::State;
use crate::printer_app::{DiscoveryGeneration, PaConfigurationCandidate, ResolveError};

impl State {
    /// Starts a new Add Printer discovery generation.
    pub(crate) fn start_add_printer_discovery(&self) -> DiscoveryGeneration {
        let mut model = self.locked_model();
        let applications = model
            .printer_applications
            .values()
            .filter(|application| is_worth_asking(application))
            .map(|application| (application.id.clone(), application_name(application)))
            .collect::<Vec<_>>();
        let generation = model.add_printer_discovery.start(applications);
        drop(model);

        self.emit_add_printer_discovery_changed();

        generation
    }

    /// Adds an eligible new application to the active round and returns its generation.
    pub(crate) fn join_add_printer_round(
        &self,
        application_id: &str,
    ) -> Option<DiscoveryGeneration> {
        let mut model = self.locked_model();
        let application = model.printer_applications.get(application_id)?;
        if !is_worth_asking(application) {
            return None;
        }
        let name = application_name(application);
        let generation = model
            .add_printer_discovery
            .join(application_id.to_string(), name)?;
        drop(model);

        self.emit_add_printer_discovery_changed();

        Some(generation)
    }

    /// Returns the applications to ask in a generation, with their system URIs.
    pub(crate) fn add_printer_scan_targets(&self) -> Vec<PrinterApplication> {
        let model = self.locked_model();
        let wanted = model
            .add_printer_discovery
            .reply()
            .printer_application_scans
            .into_iter()
            .map(|status| status.printer_application_id)
            .collect::<Vec<_>>();

        wanted
            .into_iter()
            .filter_map(|id| model.printer_applications.get(&id).cloned())
            .collect()
    }

    /// Records that an application's scan has begun.
    pub(crate) fn mark_printer_application_searching(
        &self,
        generation: DiscoveryGeneration,
        application_id: &str,
    ) {
        let changed = self
            .locked_model()
            .add_printer_discovery
            .mark_searching(generation, application_id);

        if changed {
            self.emit_add_printer_discovery_changed();
        }
    }

    /// Replaces one application's results, emitting at most one event.
    pub(crate) fn replace_printer_application_snapshot(
        &self,
        generation: DiscoveryGeneration,
        application_id: &str,
        state: PrinterApplicationScanState,
        candidates: Vec<PaConfigurationCandidate>,
        quarantined: usize,
    ) {
        let changed = self.locked_model().add_printer_discovery.replace_snapshot(
            generation,
            application_id,
            state,
            candidates,
            quarantined,
        );

        if changed {
            self.emit_add_printer_discovery_changed();
        }
    }

    /// Returns the current Add Printer discovery state.
    pub(crate) fn add_printer_discovery_reply(&self) -> AddPrinterDiscoveryReply {
        self.locked_model().add_printer_discovery.reply()
    }

    /// Returns the current generation without building a discovery reply.
    pub(crate) fn add_printer_generation(&self) -> DiscoveryGeneration {
        self.locked_model().add_printer_discovery.generation()
    }

    /// Resolves a client's selection to the candidate the server recorded.
    pub(crate) fn resolve_add_printer_candidate(
        &self,
        generation: DiscoveryGeneration,
        physical_printer_id: &str,
        candidate_id: &str,
    ) -> Result<PaConfigurationCandidate, ResolveError> {
        self.locked_model()
            .add_printer_discovery
            .resolve(generation, physical_printer_id, candidate_id)
            .cloned()
    }

    /// Returns whether a candidate is still reported by its application.
    pub(crate) fn add_printer_candidate_is_current(&self, candidate_id: &str) -> bool {
        self.locked_model()
            .add_printer_discovery
            .candidate_is_current(candidate_id)
    }
}

/// Returns whether an application belongs in a discovery round.
/// Include unprobed applications because DNS-SD re-announcements temporarily reset their state.
fn is_worth_asking(application: &PrinterApplication) -> bool {
    application.capabilities.find_devices
        || application.is_local()
        || matches!(
            application.state,
            PrinterApplicationState::Discovered | PrinterApplicationState::Probing
        )
}

/// Returns the name to show for a Printer Application.
fn application_name(application: &PrinterApplication) -> String {
    application
        .make_and_model
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| application.service_name.clone())
}
