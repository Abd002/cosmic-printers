//! Printers a Printer Application was asked to create, until they show up by themselves.

use cosmic_settings_printers_core::{ConfigurePrinterReply, Error, PrinterConfigurationState};

use super::State;
use crate::printer_app::{PendingConfigurationState, PendingPaConfiguration, reconcile};

impl State {
    /// Records a configuration attempt.
    pub(crate) fn insert_pending_configuration(&self, pending: PendingPaConfiguration) {
        self.locked_model()
            .pending_pa_configurations
            .insert(pending.operation_id.clone(), pending);
        self.emit_printer_configuration_changed();
    }

    /// Returns a configuration attempt.
    pub(crate) fn pending_configuration(
        &self,
        operation_id: &str,
    ) -> Option<PendingPaConfiguration> {
        self.locked_model()
            .pending_pa_configurations
            .get(operation_id)
            .cloned()
    }

    /// Matches waiting configuration attempts against the destinations now known.
    fn reconcile_pending_configurations(&self) -> bool {
        let mut model = self.locked_model();
        if model.pending_pa_configurations.is_empty() {
            return false;
        }

        let destinations = model
            .available_destinations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut changed = false;

        for pending in model.pending_pa_configurations.values_mut() {
            if !pending.state.is_awaiting() {
                continue;
            }

            match reconcile::find_destination(pending, destinations.iter()) {
                Some(destination) => {
                    tracing::debug!(
                        destination_id = destination.id(),
                        attempt = reconcile::describe(pending),
                        "printer configuration reconciled to a destination"
                    );
                    pending.state = PendingConfigurationState::Reconciled {
                        destination_id: destination.id().to_string(),
                    };
                    changed = true;
                }
                // A printer that was created but never advertised would otherwise
                // leave the attempt waiting forever.
                None if pending.created_at.elapsed() >= reconcile::ADVERTISEMENT_TIMEOUT => {
                    tracing::warn!(
                        attempt = reconcile::describe(pending),
                        "printer was created but never advertised; setup needs finishing by hand"
                    );
                    pending.state = PendingConfigurationState::ManualActionRequired;
                    changed = true;
                }
                None => {}
            }
        }

        changed
    }

    /// Reconciles pending attempts and emits their completion separately from destination changes.
    pub(super) fn reconcile_after_destination_change(&self) {
        if self.reconcile_pending_configurations() {
            self.emit_printer_configuration_changed();
        }
    }

    /// Returns the state of an earlier configuration attempt.
    pub(crate) fn printer_configuration(
        &self,
        operation_id: &str,
    ) -> Result<ConfigurePrinterReply, Error> {
        let pending = self.pending_configuration(operation_id).ok_or_else(|| {
            Error::PrinterConfigurationUnknownOutcome {
                application_id: String::new(),
                printer_name: operation_id.to_string(),
            }
        })?;

        let (state, destination_id) = match &pending.state {
            PendingConfigurationState::AwaitingAdvertisement => {
                (PrinterConfigurationState::AwaitingAdvertisement, None)
            }
            PendingConfigurationState::Reconciled { destination_id } => (
                PrinterConfigurationState::Reconciled,
                Some(destination_id.clone()),
            ),
            PendingConfigurationState::AlreadyConfigured => {
                (PrinterConfigurationState::AlreadyConfigured, None)
            }
            PendingConfigurationState::ManualActionRequired => {
                (PrinterConfigurationState::ManualActionRequired, None)
            }
            PendingConfigurationState::UnknownOutcome => {
                (PrinterConfigurationState::UnknownOutcome, None)
            }
        };

        Ok(ConfigurePrinterReply {
            operation_id: pending.operation_id,
            state,
            configured_printer_name: pending.configured_printer_name,
            destination_id,
            web_interface_uri: pending.web_interface_uri,
        })
    }
}
