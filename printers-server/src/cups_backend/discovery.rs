use cosmic_settings_printers_core::{DiscoveredPrinter, Error, PrinterEntry};
use cups_rs::{IppOperation, IppRequest, IppTag, IppValueTag};
use std::collections::HashSet;

use super::helpers::{
    CupsResultExt, LocalSocketGuard, PRINTER_ATTRIBUTES, add_requesting_user, configured_printers,
    discovered_printers, ensure_success, fill_attrs_from_device, fill_device_attrs_from_device,
    printer_queue_name, printers_match,
};
use super::metadata::{self, QueueMetadata};

pub async fn list_discovered_printers() -> Result<Vec<DiscoveredPrinter>, Error> {
    tokio::task::spawn_blocking(|| {
        let mut configured = configured_printers(250)?;
        metadata::apply(&mut configured)?;
        let mut discovered = discovered_printers(250)?;

        for printer in discovered.values_mut() {
            if fill_device_attrs_from_device(printer).is_err() {
                eprintln!(
                    "failed to load device attributes for destination {}",
                    printer.id
                );
            }
        }

        let mut discovered = discovered
            .into_values()
            .filter(|candidate| {
                !configured
                    .values()
                    .any(|queue| printers_match(queue, candidate))
            })
            .collect::<Vec<_>>();

        for printer in &mut discovered {
            if fill_attrs_from_device(printer, PRINTER_ATTRIBUTES).is_err() {
                eprintln!(
                    "failed to load all attributes for discovered destination {}",
                    printer.id
                );
            }
            // debugging output to verify discovered attributes are loaded correctly
            print_discovered_destination(printer);
        }

        let mut printers = discovered
            .into_iter()
            .filter_map(discovered_printer)
            .collect::<Vec<_>>();
        printers.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(printers)
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

/// Prints every attribute returned by CUPS for a discovered destination.
fn print_discovered_destination(printer: &PrinterEntry) {
    eprintln!("discovered destination:");
    eprintln!("  id: {}", printer.id);
    eprintln!("  name: {}", printer.name);
    eprintln!("  is-default: {}", printer.is_default);

    let mut attributes = printer.options.iter().collect::<Vec<_>>();
    attributes.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    eprintln!("  attributes:");
    for (name, value) in attributes {
        eprintln!("    {name}: {value}");
    }
}

pub async fn add_discovered_printer(printer_id: &str) -> Result<(), Error> {
    let printer_id = printer_id.to_string();

    tokio::task::spawn_blocking(move || {
        let discovered = discovered_printers(250)?;
        let mut printer = discovered
            .get(&printer_id)
            .cloned()
            .ok_or(Error::PrinterNotFound)?;
        fill_device_attrs_from_device(&mut printer)?;

        let mut configured = configured_printers(250)?;
        metadata::apply(&mut configured)?;
        let device_uri = (!printer.device_uri.is_empty())
            .then(|| printer.device_uri.clone())
            .ok_or_else(|| Error::MissingDeviceUri {
                queue: printer.id.clone(),
            })?;
        let queue_name = available_queue_name(printer_queue_name(&printer), configured.values());
        let info = printer.name.clone();
        let location = printer.location.clone();
        let device_uuid = printer.options.get("device-uuid").map(String::as_str);
        let printer_more_info = printer.options.get("printer-more-info").map(String::as_str);

        let _guard = LocalSocketGuard::engage()?;
        let result = create_local_printer(&queue_name, &device_uri, &info, &location);
        if result.is_ok() {
            metadata::save(
                &queue_name,
                QueueMetadata {
                    device_uuid: device_uuid.map(ToString::to_string),
                    printer_more_info: printer_more_info.map(ToString::to_string),
                },
            )?;
        }
        // if result.is_ok() {
        //     result = create_permanent_printer(&queue_name);
        // }

        result
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

/// Creates a temporary local queue for a discovered driverless device.
fn create_local_printer(
    queue_name: &str,
    device_uri: &str,
    info: &str,
    location: &str,
) -> Result<(), Error> {
    let mut request = IppRequest::new(IppOperation::CupsCreateLocalPrinter).cups_err()?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            "ipp://localhost/",
        )
        .cups_err()?;
    add_requesting_user(&mut request)?;
    request
        .add_string(
            IppTag::Printer,
            IppValueTag::Name,
            "printer-name",
            queue_name,
        )
        .cups_err()?;
    add_printer_attributes(&mut request, device_uri, info, location)?;

    let response = request.send_default("/").cups_err()?;
    ensure_success(&response, "CUPS-Create-Local-Printer")
}

/// Adds the device URI, description, and optional location to an IPP request.
fn add_printer_attributes(
    request: &mut IppRequest,
    device_uri: &str,
    info: &str,
    location: &str,
) -> Result<(), Error> {
    request
        .add_string(IppTag::Printer, IppValueTag::Uri, "device-uri", device_uri)
        .cups_err()?;
    request
        .add_string(IppTag::Printer, IppValueTag::Text, "printer-info", info)
        .cups_err()?;
    if !location.is_empty() {
        request
            .add_string(
                IppTag::Printer,
                IppValueTag::Text,
                "printer-location",
                location,
            )
            .cups_err()?;
    }

    Ok(())
}

/// Converts a discovered CUPS destination into the lightweight discovery API type.
fn discovered_printer(printer: PrinterEntry) -> Option<DiscoveredPrinter> {
    if printer.device_uri.is_empty() {
        return None;
    }

    Some(DiscoveredPrinter {
        id: printer.id,
        name: printer.name,
        device_uri: printer.device_uri,
        location: printer.location,
        model: printer.model,
    })
}

/// Produces a valid queue name that does not collide with configured queues.
fn available_queue_name<'a>(
    name: &str,
    configured: impl Iterator<Item = &'a PrinterEntry>,
) -> String {
    let sanitized_name = name
        .chars()
        .map(|character| match character {
            character if character.is_ascii_alphanumeric() => character,
            '-' | '_' | '.' => character,
            _ => '_',
        })
        .collect::<String>();
    let base_name = if sanitized_name.is_empty() {
        "printer".to_string()
    } else {
        sanitized_name
    };
    let existing_names = configured.map(printer_queue_name).collect::<HashSet<_>>();

    let mut candidate = base_name.clone();
    let mut suffix = 2;
    while existing_names.contains(candidate.as_str()) {
        candidate = format!("{base_name}_{suffix}");
        suffix += 1;
    }

    candidate
}
