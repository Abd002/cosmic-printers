//! Grouping for configured destinations and Add Printer discovery candidates.

mod add_printer_list;
mod device_id;
mod evidence;
mod printers_list;

pub use add_printer_list::{GroupedDevice, PhysicalDeviceObservation, group_by_physical_device};
pub use device_id::DeviceId;
pub use evidence::{
    IdentityConfidence, NormalizedEndpoint, PhysicalDeviceEvidence, PhysicalIdentityAggregate,
};
pub use printers_list::{GroupedDestination, group_printers, printers_match};
