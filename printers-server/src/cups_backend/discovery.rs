use cosmic_settings_printers_core::{DiscoveredPrinter, Error};
use cups_rs::{Destination, IppOperation, IppRequest, IppTag, IppValueTag};
use std::collections::HashSet;

use super::helpers::{
    LOCAL_CUPS_SOCKET, add_requesting_user, configured_destinations, destination_uri,
    destinations_match, discovered_destinations, ensure_success,
};

pub async fn list_discovered_printers() -> Result<Vec<DiscoveredPrinter>, Error> {
    tokio::task::spawn_blocking(|| {
        let configured = configured_destinations(250)?;
        let discovered = discovered_destinations(250)?;

        let mut printers = discovered
            .into_values()
            .filter(|candidate| {
                !configured
                    .values()
                    .any(|queue| destinations_match(queue, candidate))
            })
            .filter_map(discovered_printer)
            .collect::<Vec<_>>();
        printers.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(printers)
    })
    .await
    .map_err(|_| Error::CupsFailed)?
}

pub async fn add_discovered_printer(printer_id: &str) -> Result<(), Error> {
    let printer_id = printer_id.to_string();

    tokio::task::spawn_blocking(move || {
        let discovered = discovered_destinations(250)?;
        let destination = discovered.get(&printer_id).ok_or(Error::PrinterNotFound)?;
        let configured = configured_destinations(250)?;
        let device_uri = destination_uri(destination).ok_or(Error::CupsFailed)?;
        let queue_name = available_queue_name(&destination.name, configured.values());
        let info = destination
            .info()
            .cloned()
            .unwrap_or_else(|| destination.name.clone());
        let location = destination.location().cloned().unwrap_or_default();

        let previous_server = cups_rs::config::get_server();
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).map_err(|_| Error::CupsFailed)?;

        let mut result = create_local_printer(&queue_name, device_uri, &info, &location);
        if result.is_ok() {
            result = create_permanent_printer(&queue_name);
        }

        cups_rs::config::set_server(Some(&previous_server)).map_err(|_| Error::CupsFailed)?;
        result
    })
    .await
    .map_err(|_| Error::CupsFailed)?
}

/// Creates a temporary local queue for a discovered driverless device.
fn create_local_printer(
    queue_name: &str,
    device_uri: &str,
    info: &str,
    location: &str,
) -> Result<(), Error> {
    let mut request =
        IppRequest::new(IppOperation::CupsCreateLocalPrinter).map_err(|_| Error::CupsFailed)?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            "ipp://localhost/",
        )
        .map_err(|_| Error::CupsFailed)?;
    add_requesting_user(&mut request)?;
    request
        .add_string(
            IppTag::Printer,
            IppValueTag::Name,
            "printer-name",
            queue_name,
        )
        .map_err(|_| Error::CupsFailed)?;
    add_printer_attributes(&mut request, device_uri, info, location)?;

    let response = request.send_default("/").map_err(|_| Error::CupsFailed)?;
    ensure_success(response, "CUPS-Create-Local-Printer")
}

/// Promotes a temporary queue to permanent while leaving sharing disabled.
fn create_permanent_printer(queue_name: &str) -> Result<(), Error> {
    let mut request =
        IppRequest::new(IppOperation::CupsAddModifyPrinter).map_err(|_| Error::CupsFailed)?;
    let printer_uri = format!("ipp://localhost/printers/{queue_name}");

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            &printer_uri,
        )
        .map_err(|_| Error::CupsFailed)?;
    add_requesting_user(&mut request)?;
    request
        .add_boolean(IppTag::Printer, "printer-is-shared", true)
        .map_err(|_| Error::CupsFailed)?;

    let response = request
        .send_default("/admin/")
        .map_err(|_| Error::CupsFailed)?;
    ensure_success(response, "CUPS-Add-Modify-Printer sharing update")
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
        .map_err(|_| Error::CupsFailed)?;
    request
        .add_string(IppTag::Printer, IppValueTag::Text, "printer-info", info)
        .map_err(|_| Error::CupsFailed)?;
    if !location.is_empty() {
        request
            .add_string(
                IppTag::Printer,
                IppValueTag::Text,
                "printer-location",
                location,
            )
            .map_err(|_| Error::CupsFailed)?;
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
