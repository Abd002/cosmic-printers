//! Transport-independent printer change events.

use serde::{Deserialize, Serialize};

/// Identifies the cache a client must re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrintersEventKind {
    /// The set or attributes of available destinations changed.
    AvailableDestinationsChanged,
    /// The set or state of discovered Printer Applications changed.
    PrinterApplicationsChanged,
    /// An Add Printer discovery generation produced new results.
    AddPrinterDiscoveryChanged,
    /// A printer configuration attempt changed state.
    PrinterConfigurationChanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrintersEvent {
    pub kind: PrintersEventKind,
    /// Printer affected by this event, when the event is destination-specific.
    pub printer_id: Option<String>,
}
