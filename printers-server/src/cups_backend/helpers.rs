use cosmic_settings_printers_core::{Error, PrinterEntry, PrinterStatus, parse_uri_endpoint};
use cups_rs::{
    Destination, IppOperation, IppRequest, IppResponse, IppTag, IppValueTag,
    PrinterState as CupsPrinterState,
};
use std::collections::HashMap;

pub(super) const LOCAL_CUPS_SOCKET: &str = "/run/cups/cups.sock";

/// Adds the current CUPS user to an IPP request.
pub(super) fn add_requesting_user(request: &mut IppRequest) -> Result<(), Error> {
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Name,
            "requesting-user-name",
            &cups_rs::config::get_user(),
        )
        .map_err(|_| Error::CupsFailed)
}

/// Converts an IPP response status into the backend result.
pub(super) fn ensure_success(response: IppResponse, operation: &str) -> Result<(), Error> {
    let status = response.status();
    if status.is_successful() {
        Ok(())
    } else {
        eprintln!("{operation} failed with status {status:?}");
        Err(Error::CupsFailed)
    }
}

/// Fetches requested IPP attributes that are absent from a destination.
pub(super) fn fill_missing_attrs(
    destination: &mut Destination,
    attrs: &[&str],
) -> Result<(), Error> {
    let missing = attrs
        .iter()
        .copied()
        .filter(|attr| !destination.options.contains_key(*attr))
        .collect::<Vec<_>>();

    if missing.is_empty() {
        return Ok(());
    }

    let printer_uri = destination
        .uri()
        .cloned()
        .unwrap_or_else(|| local_printer_uri(destination));

    let request = printer_attrs_request(&printer_uri, &missing)?;
    let response = request.send_default("/").map_err(|_| Error::CupsFailed)?;

    if !response.status().is_successful() {
        return Err(Error::CupsFailed);
    }

    for name in missing {
        let Some(attr) = response.find_attribute(name, None) else {
            continue;
        };
        let values = attr_values(name, attr);
        if !values.is_empty() {
            destination
                .options
                .insert(name.to_string(), values.join(","));
        }
    }

    Ok(())
}

/// Builds a Get-Printer-Attributes request for selected attribute names.
fn printer_attrs_request(printer_uri: &str, requested_attrs: &[&str]) -> Result<IppRequest, Error> {
    let mut request =
        IppRequest::new(IppOperation::GetPrinterAttributes).map_err(|_| Error::CupsFailed)?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            printer_uri,
        )
        .map_err(|_| Error::CupsFailed)?;
    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            requested_attrs,
        )
        .map_err(|_| Error::CupsFailed)?;

    Ok(request)
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

/// Constructs the local scheduler URI for a queue or printer class.
fn local_printer_uri(destination: &Destination) -> String {
    let path = if is_printer_class(&destination.options) {
        "classes"
    } else {
        "printers"
    };

    format!("ipp://localhost/{path}/{}", destination.name)
}

/// Checks the CUPS printer-type bitmask for the class flag.
fn is_printer_class(options: &HashMap<String, String>) -> bool {
    options
        .get("printer-type")
        .and_then(|printer_type| printer_type.parse::<u32>().ok())
        .is_some_and(|printer_type| printer_type & cups_rs::PRINTER_CLASS != 0)
}

/// Derives a simple web interface URL from a device URI hostname.
fn web_page_from_device_uri(device_uri: &str) -> Option<String> {
    let (hostname, _) = parse_uri_endpoint(device_uri)?;
    Some(format!("http://{hostname}"))
}

/// Converts a cups-rs destination into the type exposed by the printer API.
pub(super) fn destination_to_printer_entry(destination: Destination) -> PrinterEntry {
    let status = printer_status(&destination);
    let queue_status = destination.state().to_string();
    let printer_uri = destination
        .uri()
        .cloned()
        .unwrap_or_else(|| local_printer_uri(&destination));
    let id = destination.full_name();
    let name = destination
        .info()
        .filter(|info| !info.is_empty())
        .cloned()
        .unwrap_or_else(|| id.clone());
    let paper_sizes = option_values(&destination.options, "media-supported");
    let print_sides = option_values(&destination.options, "sides-supported");
    let web_page = destination
        .options
        .get("printer-more-info")
        .filter(|url| !url.trim().is_empty())
        .cloned()
        .or_else(|| {
            destination
                .options
                .get("device-uri")
                .and_then(|device_uri| web_page_from_device_uri(device_uri))
        });

    PrinterEntry {
        id,
        name,
        is_default: destination.is_default,
        printer_uri,
        status,
        queue_status,
        location: destination.location().cloned().unwrap_or_default(),
        model: destination.make_and_model().cloned().unwrap_or_default(),
        device_name: destination.device_uri().cloned().unwrap_or_default(),
        web_page,
        driver_version: String::new(),
        paper_size_idx: 0,
        print_sides_idx: 0,
        options: destination.options,
        supplies: Vec::new(),
        paper_sizes,
        print_sides,
    }
}

/// Splits a comma-separated CUPS option into trimmed values.
fn option_values(options: &HashMap<String, String>, name: &str) -> Vec<String> {
    options
        .get(name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Maps CUPS state and toner reasons to the UI printer status.
fn printer_status(destination: &Destination) -> PrinterStatus {
    if destination
        .state_reasons()
        .iter()
        .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
    {
        return PrinterStatus::LowToner;
    }

    match destination.state() {
        CupsPrinterState::Idle | CupsPrinterState::Processing => PrinterStatus::Ready,
        CupsPrinterState::Stopped | CupsPrinterState::Unknown => PrinterStatus::Offline,
    }
}
