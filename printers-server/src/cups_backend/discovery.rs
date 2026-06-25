use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::{IppOperation, IppRequest, IppTag, IppValueTag};
use std::collections::HashSet;

use super::helpers::{
    CupsResultExt, LocalSocketGuard, PRINTER_ATTRIBUTES, add_requesting_user, configured_printers,
    discovered_printers, ensure_success, fill_attrs_from_device, printer_queue_name,
    printers_match, queue_name_from_printer_uri,
};
use super::metadata::{self, QueueMetadata};

pub async fn list_discovered_printers() -> Result<Vec<PrinterEntry>, Error> {
    tokio::task::spawn_blocking(|| {
        let mut configured = configured_printers(250)?;
        metadata::apply(&mut configured)?;
        let mut discovered = discovered_printers(250)?;

        fill_discovered_printer_attrs(discovered.values_mut());

        let discovered = discovered
            .into_values()
            .filter(|candidate| {
                !configured
                    .values()
                    .any(|queue| printers_match(queue, candidate))
            })
            .collect::<Vec<_>>();

        for printer in &discovered {
            // debugging output to verify discovered attributes are loaded correctly
            print_discovered_destination(printer);
        }

        let mut printers = discovered;
        printers.retain(|printer| !printer.device_uri.is_empty());
        printers.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(printers)
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

fn fill_discovered_printer_attrs<'a>(printers: impl Iterator<Item = &'a mut PrinterEntry>) {
    std::thread::scope(|scope| {
        for printer in printers {
            scope.spawn(move || {
                if fill_attrs_from_device(printer, PRINTER_ATTRIBUTES).is_err() {
                    eprintln!(
                        "failed to load all attributes for discovered destination {}",
                        printer.id
                    );
                }
            });
        }
    });
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
        fill_attrs_from_device(&mut printer, PRINTER_ATTRIBUTES)?;

        let mut configured = configured_printers(250)?;
        metadata::apply(&mut configured)?;
        let device_uri = (!printer.device_uri.is_empty())
            .then(|| printer.device_uri.clone())
            .ok_or_else(|| Error::MissingDeviceUri {
                queue: printer.id.clone(),
            })?;
        let queue_name = available_queue_name(&printer, configured.values());
        let info = printer.name.clone();
        let location = printer.location.clone();
        let device_uuid = printer.options.get("device-uuid").map(String::as_str);
        let printer_more_info = printer.options.get("printer-more-info").map(String::as_str);

        let _guard = LocalSocketGuard::engage()?;
        let actual_queue_name = create_local_printer(&queue_name, &device_uri, &info, &location)?;
        metadata::save(
            &actual_queue_name,
            QueueMetadata {
                device_uuid: device_uuid.map(ToString::to_string),
                printer_more_info: printer_more_info.map(ToString::to_string),
            },
        )?;
        Ok(())
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
) -> Result<String, Error> {
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
    ensure_success(&response, "CUPS-Create-Local-Printer")?;

    let printer_uri = response
        .find_attribute("printer-uri-supported", None)
        .and_then(|attr| attr.get_string(0))
        .ok_or_else(|| Error::Internal {
            why: "CUPS-Create-Local-Printer response missing printer-uri-supported".to_string(),
        })?;

    queue_name_from_printer_uri(&printer_uri).ok_or_else(|| Error::Internal {
        why: format!("invalid printer-uri-supported returned by CUPS: {printer_uri}"),
    })
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

/// Produces a valid queue name that does not collide with configured queues.
fn available_queue_name<'a>(
    printer: &PrinterEntry,
    configured: impl Iterator<Item = &'a PrinterEntry>,
) -> String {
    let base_name = queue_name(printer).unwrap_or_else(|| "printer".to_string());
    let existing_names = configured.map(printer_queue_name).collect::<HashSet<_>>();

    let mut candidate = base_name.clone();
    let mut suffix = 2;
    while existing_names.contains(candidate.as_str()) {
        candidate = format!("{base_name}-{suffix}");
        suffix += 1;
    }

    candidate
}

fn queue_name(printer: &PrinterEntry) -> Option<String> {
    let mut name = queue_name_base(printer)?;

    name = name.trim().to_string();
    name = name
        .chars()
        .map(|character| match character {
            character if character.is_ascii_alphanumeric() => character,
            '-' | '_' => character,
            _ => '-',
        })
        .collect();

    const SUFFIXES: &[&str] = &[
        "-foomatic",
        "-hpijs",
        "-hpcups",
        "-cups",
        "-gutenprint",
        "-series",
        "-label-printer",
        "-dot-matrix",
        "-ps3",
        "-ps2",
        "-br-script",
        "-kpdl",
        "-pcl3",
        "-pcl",
        "-zxs",
        "-pxl",
    ];

    // Remove common driver suffixes from generated queue names.
    for suffix in SUFFIXES {
        if let Some(index) = name.to_ascii_lowercase().rfind(suffix) {
            name.truncate(index);
        }
    }

    // Normalize separators after replacing invalid characters.
    name = name.trim_matches('-').to_string();
    while name.contains("--") {
        name = name.replace("--", "-");
    }

    (!name.is_empty()).then_some(name)
}

fn queue_name_base(printer: &PrinterEntry) -> Option<String> {
    device_id_tag(printer, "mdl")
        .or_else(|| device_id_tag(printer, "model"))
        .or_else(|| non_empty_string(&printer.model))
        .or_else(|| non_empty_string(printer_queue_name(printer)))
        .or_else(|| non_empty_string(&printer.name))
}

fn device_id_tag(printer: &PrinterEntry, tag: &str) -> Option<String> {
    let device_id = printer.options.get("device-id")?;

    device_id.split(';').find_map(|field| {
        let (key, value) = field.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(tag)
            .then(|| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}
