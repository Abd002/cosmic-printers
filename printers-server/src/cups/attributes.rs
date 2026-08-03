use super::conversion::{refresh_printer_endpoint, refresh_printer_web_page};
use super::scheduler;
use crate::error::{BackendError, BackendResult};
use crate::ipp::{CupsResultExt, ensure_success, printer_attrs_request};
use cosmic_settings_printers_core::{
    EndpointSource, PrinterEntry, SupplyLevel, is_local_address, parse_printer_supplies,
};
use cups_rs::{ConnectionFlags, Destination, HttpConnection, IppResponse};

pub(in crate::cups) const PRINTER_ATTRIBUTES: &[&str] = &[
    "printer-uri-supported",
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
    "marker-high-levels",
    "marker-low-levels",
    // Devices may report `printer-supply` where queues report `marker-*` attributes.
    "printer-supply",
    "printer-supply-description",
    "media-default",
    "media-supported",
    "sides-default",
    "sides-supported",
    "printer-uuid",
    "device-uuid",
    // Some services silently ignore unsupported changes, so retain both historical spellings of
    // the attributes they accept.
    "printer-settable-attributes",
    "printer-settable-attributes-supported",
];

/// Reads attributes through the queue or device `printer-uri-supported` endpoint.
pub(in crate::cups) fn reload_attrs_from_printer_uri(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> BackendResult<()> {
    let printer_uri = printer.printer_uri().ok_or_else(|| {
        BackendError::Internal(format!("queue '{}' has no printer URI", printer.id()))
    })?;
    let request = printer_attrs_request(printer_uri, attrs)?;
    let response = scheduler::send(request, printer_uri)?;

    merge_attrs(printer, attrs, response)?;
    refresh_printer_web_page(printer);
    Ok(())
}

/// Reads `printer-supply`, or returns empty when only `marker-*` data exists.
fn supplies_the_printer_described(response: &IppResponse) -> Vec<SupplyLevel> {
    let supplies = supply_records(response);
    if supplies.is_empty() {
        return Vec::new();
    }

    let descriptions = attr_strings(response, "printer-supply-description");
    let supplies = supplies.iter().map(String::as_str).collect::<Vec<_>>();
    let descriptions = descriptions.iter().map(String::as_str).collect::<Vec<_>>();

    parse_printer_supplies(&supplies, &descriptions)
}

/// Reads `printer-supply`, which a printer sends as an octetString rather than as text.
fn supply_records(response: &IppResponse) -> Vec<String> {
    let Some(attr) = response.find_attribute("printer-supply", None) else {
        return Vec::new();
    };

    (0..attr.count())
        .filter_map(|index| attr.get_octet_string(index))
        .map(|record| String::from_utf8_lossy(&record).into_owned())
        .filter(|record| !record.trim().is_empty())
        .collect()
}

fn attr_strings(response: &IppResponse, name: &str) -> Vec<String> {
    response
        .find_attribute(name, None)
        .map(|attr| attr_values(name, attr))
        .unwrap_or_default()
}

/// Re-reads all requested attributes from the underlying device URI.
pub(in crate::cups) fn reload_attrs_from_device_uri(
    destination: &Destination,
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> BackendResult<()> {
    let (device_uri, connection) = connect_to_device(destination, printer)?;
    apply_connection_endpoint(printer, &connection);

    let request = printer_attrs_request(&printer_uri_for_request(&device_uri, &connection), attrs)?;
    let response = request
        .send(&connection, connection.resource_path())
        .cups_err()?;

    merge_attrs(printer, attrs, response)?;
    // After the endpoint, because that is where the address the page is offered at comes from.
    apply_connection_endpoint(printer, &connection);
    refresh_printer_web_page(printer);
    Ok(())
}

/// Returns the URI to name in a request sent over this connection.
fn printer_uri_for_request(device_uri: &str, connection: &HttpConnection) -> String {
    printer_uri_from_parts(
        request_scheme(device_uri),
        connection.hostname().as_deref(),
        connection.port(),
        connection.resource_path(),
    )
    // Nothing was resolved that the device URI does not already say.
    .unwrap_or_else(|| device_uri.to_string())
}

/// Builds the URI of one printer on a resolved endpoint.
fn printer_uri_from_parts(
    scheme: &str,
    host: Option<&str>,
    port: Option<u16>,
    resource: &str,
) -> Option<String> {
    if resource.is_empty() || resource == "/" {
        return None;
    }

    let (host, port) = (host?, port?);
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    Some(format!("{scheme}://{host}:{port}{resource}"))
}

/// Returns the scheme to reach a printer advertised under `uri` on.
fn request_scheme(uri: &str) -> &'static str {
    if uri.starts_with("ipps") {
        "ipps"
    } else {
        "ipp"
    }
}

/// Maximum silence during one read or write, not a deadline for the full exchange.
/// Connection timeouts do not cover a peer that connects and then stops responding.
const DEVICE_SILENCE_TIMEOUT_SECONDS: f64 = 10.0;

fn connect_to_device(
    destination: &Destination,
    printer: &PrinterEntry,
) -> BackendResult<(String, HttpConnection)> {
    let device_uri = printer
        .device_uri()
        .ok_or_else(|| BackendError::MissingDeviceUri {
            queue: printer.id().to_string(),
        })?
        .to_owned();
    let mut connection = destination
        .connect(ConnectionFlags::Device, Some(5000), None)
        .map_err(|source| BackendError::DeviceUnreachable {
            uri: device_uri.clone(),
            source,
        })?;
    connection.set_timeout(DEVICE_SILENCE_TIMEOUT_SECONDS);

    Ok((device_uri, connection))
}

fn apply_connection_endpoint(printer: &mut PrinterEntry, connection: &HttpConnection) {
    if let Some(hostname) = connection.hostname() {
        printer.set_option("endpoint-hostname", hostname);
    }
    if let Some(port) = connection.port() {
        printer.set_option("endpoint-port", port.to_string());
    }
    if let Some(address) = connection.address() {
        printer.set_option("endpoint-address", address.to_string());
        printer.set_option("endpoint-is-local", is_local_address(address).to_string());
    }
    printer.set_endpoint_source(EndpointSource::Connected);
}

fn merge_attrs(
    printer: &mut PrinterEntry,
    attrs: &[&str],
    response: IppResponse,
) -> BackendResult<()> {
    ensure_success(&response, "Get-Printer-Attributes")?;

    printer.merge_options(merge_response_attrs(&response, attrs));
    // Convert octet-string `printer-supply` data into option-map-compatible marker attributes.
    printer.set_supplies(&supplies_the_printer_described(&response));
    refresh_printer_endpoint(printer);
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
    use cups_rs::IppValueTag::{Boolean, Enum, Integer};

    let values = 0..attr.count();

    // A boolean reads as `false`/`true` rather than `0`/`1`, since it is shown and compared as a
    // word elsewhere. The name is still honoured because a server may send this one as an integer.
    if attr.value_tag() == Boolean || name == "printer-is-accepting-jobs" {
        return values.map(|at| attr.get_boolean(at).to_string()).collect();
    }

    if matches!(attr.value_tag(), Integer | Enum) {
        return values.map(|at| attr.get_integer(at).to_string()).collect();
    }

    values
        .filter_map(|at| attr.get_string(at))
        // An empty value carries nothing to show, and callers treat a present-but-empty option
        // differently from an absent one.
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{printer_uri_from_parts, request_scheme};

    #[test]
    fn a_dns_sd_service_uri_gains_the_printer_it_resolved_to() {
        assert_eq!(
            printer_uri_from_parts("ipps", Some("desktop.local"), Some(8001), "/ipp/print/Acme")
                .as_deref(),
            Some("ipps://desktop.local:8001/ipp/print/Acme")
        );
    }

    #[test]
    fn an_address_literal_is_bracketed() {
        assert_eq!(
            printer_uri_from_parts("ipps", Some("fe80::1"), Some(8001), "/ipp/print/Acme")
                .as_deref(),
            Some("ipps://[fe80::1]:8001/ipp/print/Acme")
        );
    }

    #[test]
    fn a_service_that_resolved_to_nothing_more_names_no_printer() {
        for resource in ["", "/"] {
            assert_eq!(
                printer_uri_from_parts("ipp", Some("printer.local"), Some(631), resource),
                None
            );
        }
    }

    #[test]
    fn an_unresolved_endpoint_names_no_printer() {
        assert_eq!(
            printer_uri_from_parts("ipps", None, Some(8001), "/ipp/print/Acme"),
            None
        );
        assert_eq!(
            printer_uri_from_parts("ipps", Some("desktop.local"), None, "/ipp/print/Acme"),
            None
        );
    }

    #[test]
    fn a_secure_advertisement_keeps_its_scheme() {
        assert_eq!(request_scheme("ipps://Acme._ipps._tcp.local/"), "ipps");
        assert_eq!(request_scheme("ipp://Acme._ipp._tcp.local/"), "ipp");
        assert_eq!(request_scheme("dnssd://Acme._ipps._tcp.local/"), "ipp");
    }
}
