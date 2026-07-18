use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::IppResponse;
use std::collections::HashMap;

use super::conversion::refresh_printer_entry;
use crate::ipp::{CupsResultExt, ensure_success, printer_attrs_request, send_ipp_request};

pub(in crate::cups_backend) const PRINTER_ATTRIBUTES: &[&str] = &[
    "printer-more-info",
    "printer-state",
    "printer-state-message",
    "printer-state-reasons",
    "printer-is-accepting-jobs",
    "printer-type",
    "printer-location",
    "printer-info",
    "printer-make-and-model",
    "device-uri",
    "marker-colors",
    "marker-levels",
    "marker-names",
    "marker-types",
    "media-default",
    "media-supported",
    "sides-default",
    "sides-supported",
    "printer-uuid",
    "device-uuid",
];

/// Fetches requested IPP attributes that are absent from a scheduler printer entry.
pub(in crate::cups_backend) fn fill_missing_attrs(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> Result<(), Error> {
    let missing = attrs
        .iter()
        .copied()
        .filter(|attr| !printer.options.contains_key(*attr))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(());
    }

    let request = printer_attrs_request(&printer.printer_local_uri, &missing)?;
    let response = request.send_default("/").cups_err()?;
    ensure_success(&response, "Get-Printer-Attributes")?;

    merge_response_attrs(&mut printer.options, &response, &missing);
    refresh_printer_entry(printer);
    Ok(())
}

/// Fetches and merges every IPP attribute exposed by a direct device printer.
pub(in crate::cups_backend) fn fill_attrs_from_device(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> Result<(), Error> {
    if printer.device_uri.is_empty() {
        return Err(Error::MissingDeviceUri {
            queue: printer.id.clone(),
        });
    }

    fill_attrs_from_device_uri(printer, attrs)
}

/// Sends the raw IPP request to an already-selected device URI.
fn fill_attrs_from_device_uri(printer: &mut PrinterEntry, attrs: &[&str]) -> Result<(), Error> {
    let queue_name = printer.id.clone();
    let device_uri = printer.device_uri.clone();

    let printer_uri = device_uri;
    let request = printer_attrs_request(&printer_uri, attrs)?;
    let response = send_ipp_request(request, &printer_uri).map_err(|error| match error {
        Error::DeviceUnreachable { why } => Error::DeviceUnreachable {
            why: format!("{queue_name}: {why}"),
        },
        error => error,
    })?;
    ensure_success(&response, "Get-Printer-Attributes")?;

    merge_response_attrs(&mut printer.options, &response, attrs);
    refresh_printer_entry(printer);
    Ok(())
}

/// Copies requested response attributes into the destination option map.
fn merge_response_attrs(
    options: &mut HashMap<String, String>,
    response: &IppResponse,
    attrs: &[&str],
) {
    for name in attrs {
        let Some(attr) = response.find_attribute(name, None) else {
            continue;
        };
        let values = attr_values(name, attr);
        if !values.is_empty() {
            options.insert((*name).to_string(), values.join(","));
        }
    }
}

/// Converts all values of an IPP attribute into strings.
fn attr_values(name: &str, attr: cups_rs::IppAttribute) -> Vec<String> {
    if name == "printer-is-accepting-jobs" {
        return (0..attr.count())
            .map(|index| attr.get_boolean(index).to_string())
            .collect();
    }

    let values = (0..attr.count())
        .filter_map(|index| attr.get_string(index))
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect::<Vec<_>>();

    if values.is_empty() {
        (0..attr.count())
            .map(|index| attr.get_integer(index).to_string())
            .collect()
    } else {
        values
    }
}
