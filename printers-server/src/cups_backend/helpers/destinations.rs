use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::{Destination, enum_destinations};
use std::collections::HashMap;

use super::conversion::destination_to_printer_entry;
use crate::error::{BackendError, BackendResult};

/// Lists queues configured in the local CUPS scheduler as normalized printer entries.
pub(in crate::cups_backend) fn available_destinations(
    timeout_ms: i32,
) -> BackendResult<HashMap<String, PrinterEntry>> {
    let mut destinations = HashMap::<String, Destination>::new();

    enum_destinations(
        cups_rs::DEST_FLAGS_NONE,
        timeout_ms,
        None,
        0,
        0,
        &mut |flags, destination, destinations: &mut HashMap<String, Destination>| {
            let id = destination.full_name();

            if flags & cups_rs::DEST_FLAGS_REMOVED != 0 {
                destinations.remove(&id);
            } else {
                destinations.insert(id, destination.clone());
            }

            true
        },
        &mut destinations,
    )
    .map_err(BackendError::FailedToGetPrinters)?;

    Ok(printer_entry_set(destinations))
}

/// Normalizes raw CUPS destinations immediately after enumeration.
fn printer_entry_set(destinations: HashMap<String, Destination>) -> HashMap<String, PrinterEntry> {
    destinations
        .into_iter()
        .map(|(id, destination)| (id, destination_to_printer_entry(destination)))
        .collect()
}
