use cosmic_settings_printers_core::{
    DeviceIdentity, Error, PrinterEntry, PrinterStatus, parse_uri_endpoint,
};
use cups_rs::{
    Destination, HttpConnection, IppOperation, IppRequest, IppResponse, IppStatus, IppTag,
    IppValueTag, PrinterState as CupsPrinterState, enum_destinations,
};
use std::collections::HashMap;

pub(super) const PRINTER_ATTRIBUTES: &[&str] = &[
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

pub(super) const LOCAL_CUPS_SOCKET: &str = "/run/cups/cups.sock";

// CUPS wrapper/binding helpers.

pub(super) struct LocalSocketGuard {
    previous: String,
}

impl LocalSocketGuard {
    pub(super) fn engage() -> Result<Self, Error> {
        let previous = cups_rs::config::get_server();
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).cups_err()?;
        Ok(Self { previous })
    }
}

impl Drop for LocalSocketGuard {
    fn drop(&mut self) {
        let _ = cups_rs::config::set_server(Some(&self.previous));
    }
}

pub(super) trait CupsResultExt<T> {
    fn cups_err(self) -> Result<T, Error>;
}

impl<T, E: std::fmt::Display> CupsResultExt<T> for std::result::Result<T, E> {
    fn cups_err(self) -> Result<T, Error> {
        self.map_err(|error| Error::CupsFailed {
            why: error.to_string(),
        })
    }
}

/// Lists queues configured in the local CUPS scheduler as normalized printer entries.
pub(super) fn configured_printers(timeout_ms: i32) -> Result<HashMap<String, PrinterEntry>, Error> {
    let destinations = enum_destination_set(
        cups_rs::PRINTER_LOCAL,
        cups_rs::PRINTER_DISCOVERED,
        timeout_ms,
    )?;
    Ok(printer_entry_set(destinations))
}

/// Discovers network and temporary CUPS destinations as normalized printer entries.
pub(super) fn discovered_printers(timeout_ms: i32) -> Result<HashMap<String, PrinterEntry>, Error> {
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

// Raw IPP request helpers.

/// Adds the current CUPS user to an IPP request.
pub(super) fn add_requesting_user(request: &mut IppRequest) -> Result<(), Error> {
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Name,
            "requesting-user-name",
            &cups_rs::config::get_user(),
        )
        .cups_err()
}

/// Converts an IPP response status into the backend result.
pub(super) fn ensure_success(response: &IppResponse, operation: &str) -> Result<(), Error> {
    let status = response.status();
    if status.is_successful() {
        Ok(())
    } else {
        match status {
            IppStatus::ErrorNotAuthorized
            | IppStatus::ErrorForbidden
            | IppStatus::ErrorNotAuthenticated => Err(Error::PermissionDenied {
                operation: operation.to_string(),
            }),
            _ => Err(Error::CupsFailed {
                why: format!("{operation} failed with status {status:?}"),
            }),
        }
    }
}

/// Builds a raw Get-Printer-Attributes request for selected attribute names.
fn printer_attrs_request(printer_uri: &str, requested_attrs: &[&str]) -> Result<IppRequest, Error> {
    let mut request = IppRequest::new(IppOperation::GetPrinterAttributes).cups_err()?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            printer_uri,
        )
        .cups_err()?;
    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            requested_attrs,
        )
        .cups_err()?;

    Ok(request)
}

// URI and destination identity helpers.

/// Constructs the local scheduler URI for a queue or printer class.
fn local_printer_uri(destination: &Destination) -> String {
    let path = if is_printer_class(&destination.options) {
        "classes"
    } else {
        "printers"
    };

    format!("ipp://localhost/{path}/{}", destination.name)
}

/// Derives a simple web interface URL from a device URI hostname.
fn web_page_from_device_uri(device_uri: &str) -> Option<String> {
    let (hostname, _) = parse_uri_endpoint(device_uri)?;
    Some(format!("http://{hostname}"))
}

/// Splits a CUPS destination id into its queue name and optional instance.
pub(super) fn split_queue_instance(printer_id: &str) -> (&str, Option<&str>) {
    printer_id
        .split_once('/')
        .map_or((printer_id, None), |(name, instance)| {
            (name, Some(instance))
        })
}

/// Returns the CUPS queue name portion of a printer entry id.
pub(super) fn printer_queue_name(printer: &PrinterEntry) -> &str {
    split_queue_instance(&printer.id).0
}

/// Checks whether two printer entries refer to the same physical device.
pub(super) fn printers_match(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    if printer_identity(left).matches(&printer_identity(right)) {
        return true;
    }

    cups_browsed_name_matches(left, right)
}

/// Extracts the shared matching identity from a printer entry.
fn printer_identity(printer: &PrinterEntry) -> DeviceIdentity {
    DeviceIdentity::new(
        non_empty_option(&printer.options, "device-uuid"),
        non_empty_option(&printer.options, "device-uri"),
        non_empty_option(&printer.options, "printer-uri-supported"),
    )
}

/// Matches a cups-browsed queue with its DNS-SD destination by queue name.
fn cups_browsed_name_matches(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    (left.options.contains_key("cups-browsed") || right.options.contains_key("cups-browsed"))
        && left.id.eq_ignore_ascii_case(&right.id)
}

// Raw IPP attribute loading.

/// Loads identity and web interface attributes from an IPP/IPPS device.
pub(super) fn fill_device_attrs_from_device(printer: &mut PrinterEntry) -> Result<(), Error> {
    if printer.device_uri.is_empty() {
        return Ok(());
    }

    let is_ipp = printer
        .device_uri
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .is_some_and(|scheme| {
            scheme.eq_ignore_ascii_case("ipp") || scheme.eq_ignore_ascii_case("ipps")
        });
    if !is_ipp {
        return Ok(());
    }

    fill_attrs_from_device(printer, &["device-uuid", "printer-more-info"])
}

/// Fetches requested IPP attributes that are absent from a scheduler printer entry.
pub(super) fn fill_missing_attrs(printer: &mut PrinterEntry, attrs: &[&str]) -> Result<(), Error> {
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
pub(super) fn fill_attrs_from_device(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> Result<(), Error> {
    if printer.device_uri.is_empty() {
        return Err(Error::MissingDeviceUri {
            queue: printer.id.clone(),
        });
    }

    fill_attrs_from_device_uri(
        &printer.id,
        &printer.device_uri,
        &mut printer.options,
        attrs,
    )?;
    refresh_printer_entry(printer);
    Ok(())
}

/// Sends the raw IPP request to an already-selected device URI.
fn fill_attrs_from_device_uri(
    queue_name: &str,
    device_uri: &str,
    options: &mut HashMap<String, String>,
    attrs: &[&str],
) -> Result<(), Error> {
    let (connection, printer_uri) =
        HttpConnection::connect_uri(device_uri, Some(250)).map_err(|error| {
            Error::DeviceUnreachable {
                why: format!("{queue_name}: {error}"),
            }
        })?;
    let request = printer_attrs_request(&printer_uri, attrs)?;
    let response = request
        .send(&connection, connection.resource_path())
        .map_err(|error| Error::DeviceUnreachable {
            why: format!("{queue_name}: {error}"),
        })?;
    ensure_success(&response, "Get-Printer-Attributes")?;

    merge_response_attrs(options, &response, attrs);
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

// CUPS option conversion helpers.

/// Checks the CUPS printer-type bitmask for the class flag.
fn is_printer_class(options: &HashMap<String, String>) -> bool {
    options
        .get("printer-type")
        .and_then(|printer_type| printer_type.parse::<u32>().ok())
        .is_some_and(|printer_type| printer_type & cups_rs::PRINTER_CLASS != 0)
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

/// Reads a trimmed option and treats missing or empty values as absent.
fn non_empty_option<'a>(options: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

// Public API model conversion.

/// Converts a cups-rs destination into the type exposed by the printer API.
fn destination_to_printer_entry(mut destination: Destination) -> PrinterEntry {
    let queue_status = destination.state().to_string();
    let printer_local_uri = destination
        .uri()
        .cloned()
        .unwrap_or_else(|| local_printer_uri(&destination));
    let device_uri = destination.device_uri().cloned().unwrap_or_default();
    destination
        .options
        .entry("printer-uri-supported".to_string())
        .or_insert_with(|| printer_local_uri.clone());
    destination
        .options
        .entry("device-uri".to_string())
        .or_insert_with(|| device_uri.clone());
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
        printer_local_uri,
        status: printer_status(&destination),
        queue_status,
        location: destination.location().cloned().unwrap_or_default(),
        model: destination.make_and_model().cloned().unwrap_or_default(),
        device_uri,
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

/// Recomputes derived public fields after new IPP attributes are merged.
fn refresh_printer_entry(printer: &mut PrinterEntry) {
    printer.device_uri = printer
        .options
        .get("device-uri")
        .cloned()
        .unwrap_or_default();
    printer.location = printer
        .options
        .get("printer-location")
        .cloned()
        .unwrap_or_default();
    printer.model = printer
        .options
        .get("printer-make-and-model")
        .cloned()
        .unwrap_or_default();
    printer.web_page = printer
        .options
        .get("printer-more-info")
        .filter(|url| !url.trim().is_empty())
        .cloned()
        .or_else(|| web_page_from_device_uri(&printer.device_uri));
    printer.paper_sizes = option_values(&printer.options, "media-supported");
    printer.print_sides = option_values(&printer.options, "sides-supported");
    printer.status = printer_status_from_options(printer);
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

/// Maps normalized printer options to the UI printer status after IPP refreshes.
fn printer_status_from_options(printer: &PrinterEntry) -> PrinterStatus {
    if option_values(&printer.options, "printer-state-reasons")
        .iter()
        .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
    {
        return PrinterStatus::LowToner;
    }

    match printer.options.get("printer-state").map(String::as_str) {
        Some("5") => PrinterStatus::Offline,
        Some("3" | "4") => PrinterStatus::Ready,
        _ => printer.status.clone(),
    }
}
