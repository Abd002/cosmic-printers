use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::{Destination, enum_destinations};
use std::collections::HashMap;

use super::conversion::destination_to_printer_entry;

/// Lists queues configured in the local CUPS scheduler as normalized printer entries.
pub(in crate::cups_backend) fn configured_printers(
    timeout_ms: i32,
) -> Result<HashMap<String, PrinterEntry>, Error> {
    let destinations = enum_destination_set(
        cups_rs::PRINTER_LOCAL,
        cups_rs::PRINTER_DISCOVERED,
        timeout_ms,
    )?;
    Ok(printer_entry_set(destinations))
}

/// Discovers network and temporary CUPS destinations as normalized printer entries.
pub(in crate::cups_backend) fn discovered_printers(
    timeout_ms: i32,
) -> Result<HashMap<String, PrinterEntry>, Error> {
    let destinations = enum_destination_set(
        cups_rs::PRINTER_DISCOVERED,
        cups_rs::PRINTER_DISCOVERED,
        timeout_ms,
    )?;
    Ok(printer_entry_set(destinations))
}

/// Collects `cupsEnumDests` callbacks into a map keyed by destination full name.
fn enum_destination_set(
    printer_type: u32,
    printer_mask: u32,
    timeout: i32,
) -> Result<HashMap<String, Destination>, Error> {
    let mut destinations = HashMap::<String, Destination>::new();

    enum_destinations(
        cups_rs::DEST_FLAGS_NONE,
        timeout,
        None,
        printer_type,
        printer_mask,
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
    .map_err(|error| Error::FailedToGetPrinters {
        why: error.to_string(),
    })?;

    Ok(destinations)
}

/// Normalizes raw CUPS destinations immediately after enumeration.
fn printer_entry_set(destinations: HashMap<String, Destination>) -> HashMap<String, PrinterEntry> {
    destinations
        .into_iter()
        .map(|(id, destination)| (id, destination_to_printer_entry(destination)))
        .collect()
}
