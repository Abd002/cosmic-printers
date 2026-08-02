use super::conversion::refresh_printer_entry;
use crate::error::{BackendError, BackendResult};
use crate::ipp::{CupsResultExt, ensure_success, printer_attrs_request, send_ipp_request};
use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::IppResponse;

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
) -> BackendResult<()> {
    let missing = attrs
        .iter()
        .copied()
        .filter(|attr| printer.option(attr).is_none())
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(());
    }

    let printer_uri = printer
        .printer_uri()
        .ok_or_else(|| BackendError::MissingDeviceUri {
            queue: printer.id().to_string(),
        })?;
    let request = printer_attrs_request(printer_uri, &missing)?;
    let response = request.send_default("/").cups_err()?;
    ensure_success(&response, "Get-Printer-Attributes")?;

    printer.merge_options(merge_response_attrs(&response, &missing));
    refresh_printer_entry(printer);
    Ok(())
}

/// Fetches and merges every IPP attribute exposed by a direct device printer.
pub(in crate::cups_backend) fn fill_attrs_from_device(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> BackendResult<()> {
    let Some(device_uri) = printer.device_uri().map(str::to_owned) else {
        return Err(BackendError::MissingDeviceUri {
            queue: printer.id().to_string(),
        });
    };

    fill_attrs_from_device_uri(printer, &device_uri, attrs)
}

/// Sends the raw IPP request to an already-selected device URI.
fn fill_attrs_from_device_uri(
    printer: &mut PrinterEntry,
    device_uri: &str,
    attrs: &[&str],
) -> BackendResult<()> {
    let printer_uri = device_uri;
    let request = printer_attrs_request(printer_uri, attrs)?;
    let response = send_ipp_request(request, printer_uri)?;
    ensure_success(&response, "Get-Printer-Attributes")?;

    printer.merge_options(merge_response_attrs(&response, attrs));
    refresh_printer_entry(printer);
    Ok(())
}

/// Copies requested response attributes into the destination option map.
fn merge_response_attrs(response: &IppResponse, attrs: &[&str]) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for name in attrs {
        let Some(attr) = response.find_attribute(name, None) else {
            continue;
        };
        let attr_values = attr_values(name, attr);
        if !attr_values.is_empty() {
            values.push(((*name).to_string(), attr_values.join(",")));
        }
    }
    values
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
