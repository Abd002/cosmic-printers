//! Saying that something changed, without saying what.

use cosmic_settings_printers_core::{PrintersEvent, PrintersEventKind};
use tokio::sync::broadcast;

use super::State;

impl State {
    pub(crate) fn subscribe_events(&self) -> broadcast::Receiver<PrintersEvent> {
        self.events.subscribe()
    }

    pub(super) fn emit_printer_applications_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::PrinterApplicationsChanged,
            printer_id: None,
        });
    }

    pub(crate) fn emit_available_destinations_changed(&self, printer_id: &str) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::AvailableDestinationsChanged,
            printer_id: Some(printer_id.to_string()),
        });
    }

    pub(super) fn emit_add_printer_discovery_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::AddPrinterDiscoveryChanged,
            printer_id: None,
        });
    }

    pub(super) fn emit_printer_configuration_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::PrinterConfigurationChanged,
            printer_id: None,
        });
    }

    pub(super) fn emit_refresh_available_destinations(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::RefreshAvailableDestinations,
            printer_id: None,
        });
    }
}
