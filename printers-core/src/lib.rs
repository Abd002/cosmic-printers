mod error;
mod grouping;
mod types;

pub use error::Error;
pub use grouping::{DeviceIdentity, group_printers, printers_match};
pub use types::{
    GetJobsReply, GroupedDevice, JobFilter, JobInfo, JobState, ListDiscoveredPrintersReply,
    ListPrinterApplicationsReply, ListPrintersReply, PrintTestPageReply, PrinterApplication,
    PrinterApplicationState, PrinterEntry, PrinterStatus, PrintersEvent, PrintersEventKind,
    SupplyLevel,
};
