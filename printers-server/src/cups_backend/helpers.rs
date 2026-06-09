use cosmic_settings_printers_core::{
    DeviceIdentity, Error, PrinterEntry, PrinterStatus, parse_uri_endpoint,
};
use cups_rs::{
    Destination, IppOperation, IppRequest, IppResponse, IppTag, IppValueTag,
    PrinterState as CupsPrinterState, enum_destinations,
};
use std::collections::HashMap;

pub(super) const LOCAL_CUPS_SOCKET: &str = "/run/cups/cups.sock";

/// Lists queues configured in the local CUPS scheduler.
pub(super) fn configured_destinations(
    timeout_ms: i32,
) -> Result<HashMap<String, Destination>, Error> {
    enum_destination_set(
        cups_rs::PRINTER_LOCAL,
        cups_rs::PRINTER_DISCOVERED,
        timeout_ms,
    )
}

/// Discovers network and temporary CUPS destinations.
pub(super) fn discovered_destinations(
    timeout_ms: i32,
) -> Result<HashMap<String, Destination>, Error> {
    enum_destination_set(
        cups_rs::PRINTER_DISCOVERED,
        cups_rs::PRINTER_DISCOVERED,
        timeout_ms,
    )
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
    .map_err(|_| Error::CupsFailed)?;

    Ok(destinations)
}

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

/// Returns the device URI, falling back to the destination's printer URI.
pub(super) fn destination_uri(destination: &Destination) -> Option<&str> {
    destination
        .device_uri()
        .or_else(|| destination.uri())
        .map(String::as_str)
}

/// Checks whether two CUPS destinations refer to the same device.
pub(super) fn destinations_match(left: &Destination, right: &Destination) -> bool {
    if destination_identity(left).matches(&destination_identity(right)) {
        return true;
    }

    cups_browsed_name_matches(left, right)
}

/// Extracts the shared matching identity from a CUPS destination.
fn destination_identity(destination: &Destination) -> DeviceIdentity {
    DeviceIdentity::new(
        destination.options.get("printer-uuid").map(String::as_str),
        destination.device_uri().map(String::as_str),
        destination.uri().map(String::as_str),
    )
}

/// Matches a cups-browsed queue with its DNS-SD destination by queue name.
fn cups_browsed_name_matches(left: &Destination, right: &Destination) -> bool {
    (left.options.contains_key("cups-browsed") || right.options.contains_key("cups-browsed"))
        && left.name.eq_ignore_ascii_case(&right.name)
}

/// To solve the problem of dest when it be offline but still detected as ready.
/// Combines the CUPS queue state with current DNS-SD discovery availability.
/// we should replace this with Avahi
pub(super) fn printer_status_with_discovery<'a>(
    destination: &Destination,
    discovered: impl Iterator<Item = &'a Destination>,
) -> PrinterStatus {
    let cups_status = printer_status(destination);
    let device_uri = destination_uri(destination);
    let Some(device_uri) = device_uri else {
        return cups_status;
    };

    if discovered
        .into_iter()
        .filter(|candidate| is_live_discovered_device(candidate))
        .any(|candidate| destinations_match(destination, candidate))
    {
        return cups_status;
    }

    if is_dns_sd_uri(device_uri) {
        PrinterStatus::Offline
    } else {
        cups_status
    }
}

/// Rejects local scheduler queues from the discovery set.
fn is_live_discovered_device(destination: &Destination) -> bool {
    destination
        .uri()
        .map(|uri| !is_local_scheduler_uri(uri))
        .unwrap_or(true)
}

/// Detects a printer or class URI served by the local CUPS scheduler.
fn is_local_scheduler_uri(uri: &str) -> bool {
    let Some((hostname, _)) = parse_uri_endpoint(uri) else {
        return false;
    };

    matches!(hostname.as_str(), "localhost" | "127.0.0.1" | "[::1]")
        && (uri.contains("/printers/") || uri.contains("/classes/"))
}

/// Detects an IPP URI whose hostname is a DNS-SD service name.
fn is_dns_sd_uri(uri: &str) -> bool {
    let Some((hostname, _)) = parse_uri_endpoint(uri) else {
        return false;
    };
    hostname.ends_with(".local")
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
pub(super) fn destination_to_printer_entry(
    destination: Destination,
    status: PrinterStatus,
) -> PrinterEntry {
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
