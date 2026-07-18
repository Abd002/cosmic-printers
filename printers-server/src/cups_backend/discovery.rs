use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::{IppOperation, IppRequest, IppTag, IppValueTag};
use std::collections::HashSet;

use super::helpers::{
    CupsResultExt, LocalSocketGuard, PRINTER_ATTRIBUTES, add_requesting_user, configured_printers,
    ensure_success, fill_attrs_from_device, printer_queue_name, queue_name_from_printer_uri,
};
use super::metadata::{self, QueueMetadata};
use super::polkit_helper;
use crate::avahi::{discovered_printer_id, discovered_printers_match};
use crate::context::Context;

pub async fn list_discovered_printers(context: Context) -> Result<Vec<PrinterEntry>, Error> {
    start_discovery(context.clone()).await;

    Ok(context.discovered_printers().await)
}

pub(crate) async fn start_discovery(context: Context) {
    let task_context = context.clone();
    if context.start_discovery_if_idle().await {
        tokio::spawn(async move {
            crate::avahi::discover_printers_into_cache(task_context.clone()).await;
            fill_cached_discovered_attrs(task_context.clone()).await;
            task_context.finish_discovery().await;
        });
    }
}

pub async fn add_discovered_printer(mut printer: PrinterEntry) -> Result<String, Error> {
    let actual_queue_name = tokio::task::spawn_blocking(move || {
        if !printer.device_uri.is_empty() && !printer.options.contains_key("printer-make-and-model")
        {
            fill_attrs_from_device(&mut printer, PRINTER_ATTRIBUTES)?;
        }

        let configured = configured_printers(250)?;
        let device_uri = (!printer.device_uri.is_empty())
            .then(|| printer.device_uri.clone())
            .ok_or_else(|| Error::MissingDeviceUri {
                queue: printer.id.clone(),
            })?;
        let queue_name = available_queue_name(&printer, configured.values());
        let info = printer.name.clone();
        let location = printer.location.clone();
        let metadata = QueueMetadata::from_discovered_printer(&printer);

        let _guard = LocalSocketGuard::engage()?;
        let actual_queue_name = create_local_printer(&queue_name, &device_uri, &info, &location)?;
        metadata::save(&actual_queue_name, metadata)?;
        Ok::<_, Error>(actual_queue_name)
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })??;

    make_printer_permanent(&actual_queue_name).await?;
    Ok(actual_queue_name)
}

pub(crate) async fn auto_add_discovered_printer(context: Context, printer: PrinterEntry) {
    if printer.device_uri.is_empty() {
        return;
    }

    let Some(printer_id) = discovered_printer_id(&printer) else {
        return;
    };

    let already_configured = match metadata::contains_discovered_printer_id(&printer_id) {
        Ok(already_configured) => already_configured,
        Err(error) => {
            eprintln!("failed to load discovered printer metadata: {error:?}");
            false
        }
    };
    if already_configured {
        if let Err(error) = metadata::refresh_discovered_printer(&printer_id, &printer) {
            eprintln!("failed to refresh discovered printer metadata: {error:?}");
        }
        return;
    }

    if !context.start_auto_add(printer_id.clone()).await {
        return;
    }

    tokio::spawn(async move {
        match add_discovered_printer(printer).await {
            Ok(actual_queue_name) => {
                context
                    .update_discovered_printer(&printer_id, |printer| {
                        printer.id = actual_queue_name;
                    })
                    .await;
            }
            Err(error) => {
                eprintln!("failed to auto-add discovered printer {printer_id}: {error:?}");
            }
        }

        context.finish_auto_add(&printer_id).await;
    });
}

pub(crate) async fn delete_stale_discovered_printers(active_printer_ids: HashSet<String>) {
    let stale_queue_names = match metadata::stale_discovered_queue_names(&active_printer_ids) {
        Ok(queue_names) => queue_names,
        Err(error) => {
            eprintln!("failed to load discovered printer metadata: {error:?}");
            return;
        }
    };

    for queue_name in stale_queue_names {
        match polkit_helper::delete_printer(&queue_name).await {
            Ok(()) => {
                if let Err(error) = metadata::remove(&queue_name) {
                    eprintln!("failed to remove discovered printer metadata: {error:?}");
                }
            }
            Err(error) => {
                eprintln!("failed to delete stale discovered printer {queue_name}: {error:?}");
            }
        }
    }
}

async fn fill_cached_discovered_attrs(context: Context) {
    let printers = context.discovered_printers().await;

    let Ok(printers) = tokio::task::spawn_blocking(move || {
        printers
            .into_iter()
            .map(|mut printer| {
                if !printer.device_uri.is_empty()
                    && fill_attrs_from_device(&mut printer, PRINTER_ATTRIBUTES).is_ok()
                {
                    printer.options.insert(
                        "cosmic-discovery-detail-state".to_string(),
                        "enriched".to_string(),
                    );
                }
                printer
            })
            .collect::<Vec<_>>()
    })
    .await
    else {
        return;
    };

    context
        .merge_discovered_printers_by(printers, discovered_printers_match)
        .await;
}

/// Converts a temporary local queue created by CUPS into a persistent queue.
async fn make_printer_permanent(queue_name: &str) -> Result<(), Error> {
    polkit_helper::set_printer_shared(queue_name, true).await?;
    polkit_helper::set_printer_shared(queue_name, false).await
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
        candidate = format!("{base_name}_{suffix}");
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
