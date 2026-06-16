use cosmic_settings_printers_core::{DiscoveredPrinter, Error};
use cups_rs::{Destination, IppOperation, IppRequest, IppTag, IppValueTag};
use std::collections::HashSet;

use super::helpers::{
    CupsResultExt, LOCAL_CUPS_SOCKET, PRINTER_ATTRIBUTES, add_requesting_user,
    configured_destinations, destination_uri, destinations_match, discovered_destinations,
    ensure_success, fill_attrs_from_device, fill_device_attrs_from_device,
};
use super::metadata::{self, QueueMetadata};

pub async fn list_discovered_printers() -> Result<Vec<DiscoveredPrinter>, Error> {
    tokio::task::spawn_blocking(|| {
        let mut configured = configured_destinations(250)?;
        metadata::apply(&mut configured)?;
        let mut discovered = discovered_destinations(250)?;

        for destination in discovered.values_mut() {
            if fill_device_attrs_from_device(destination).is_err() {
                eprintln!(
                    "failed to load device attributes for destination {}",
                    destination.full_name()
                );
            }
        }

        let mut discovered = discovered
            .into_values()
            .filter(|candidate| {
                !configured
                    .values()
                    .any(|queue| destinations_match(queue, candidate))
            })
            .collect::<Vec<_>>();

        for destination in &mut discovered {
            if fill_attrs_from_device(destination, PRINTER_ATTRIBUTES).is_err() {
                eprintln!(
                    "failed to load all attributes for discovered destination {}",
                    destination.full_name()
                );
            }
            // debugging output to verify discovered attributes are loaded correctly
            print_discovered_destination(destination);
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
fn print_discovered_destination(destination: &Destination) {
    eprintln!("discovered destination:");
    eprintln!("  name: {}", destination.name);
    eprintln!(
        "  instance: {}",
        destination.instance.as_deref().unwrap_or("<none>")
    );
    eprintln!("  is-default: {}", destination.is_default);

    let mut attributes = destination.options.iter().collect::<Vec<_>>();
    attributes.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    eprintln!("  attributes:");
    for (name, value) in attributes {
        eprintln!("    {name}: {value}");
    }
}

pub async fn add_discovered_printer(printer_id: &str) -> Result<(), Error> {
    let printer_id = printer_id.to_string();

    tokio::task::spawn_blocking(move || {
        let discovered = discovered_destinations(250)?;
        let mut destination = discovered
            .get(&printer_id)
            .cloned()
            .ok_or(Error::PrinterNotFound)?;
        fill_device_attrs_from_device(&mut destination)?;

        let mut configured = configured_destinations(250)?;
        metadata::apply(&mut configured)?;
        let device_uri = destination_uri(&destination).ok_or_else(|| Error::MissingDeviceUri {
            queue: destination.full_name(),
        })?;
        let queue_name = available_queue_name(&destination.name, configured.values());
        let info = destination
            .info()
            .cloned()
            .unwrap_or_else(|| destination.name.clone());
        let location = destination.location().cloned().unwrap_or_default();
        let device_uuid = destination.options.get("device-uuid").map(String::as_str);
        let printer_more_info = destination
            .options
            .get("printer-more-info")
            .map(String::as_str);

        let previous_server = cups_rs::config::get_server();
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).cups_err()?;

        let result = create_local_printer(&queue_name, device_uri, &info, &location);
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

        cups_rs::config::set_server(Some(&previous_server)).cups_err()?;
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
fn discovered_printer(destination: Destination) -> Option<DiscoveredPrinter> {
    let device_uri = destination_uri(&destination)?.to_string();
    let id = destination.full_name();

    Some(DiscoveredPrinter {
        id: id.clone(),
        name: destination
            .info()
            .filter(|info| !info.is_empty())
            .cloned()
            .unwrap_or(id),
        device_uri,
        location: destination.location().cloned().unwrap_or_default(),
        model: destination.make_and_model().cloned().unwrap_or_default(),
    })
}

/// Produces a valid queue name that does not collide with configured queues.
fn available_queue_name<'a>(
    name: &str,
    configured: impl Iterator<Item = &'a Destination>,
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
    let existing_names = configured
        .map(|destination| destination.name.as_str())
        .collect::<HashSet<_>>();

    let mut candidate = base_name.clone();
    let mut suffix = 2;
    while existing_names.contains(candidate.as_str()) {
        candidate = format!("{base_name}_{suffix}");
        suffix += 1;
    }

    candidate
}
