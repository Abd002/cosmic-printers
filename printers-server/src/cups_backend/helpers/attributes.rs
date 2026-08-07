use super::conversion::refresh_printer_endpoint;
use crate::error::{BackendError, BackendResult};
use crate::ipp::{CupsResultExt, ensure_success, printer_attrs_request, send_ipp_request};
use cosmic_settings_printers_core::{
    EndpointSource, PrinterEntry, SupplyLevel, is_local_address, parse_printer_supplies,
};
use cups_rs::{ConnectionFlags, Destination, HttpConnection, IppResponse};
use std::collections::HashMap;

pub(in crate::cups_backend) const PRINTER_ATTRIBUTES: &[&str] = &[
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
    "media-default",
    "media-supported",
    "sides-default",
    "sides-supported",
    "printer-uuid",
    "device-uuid",
    // Which of its own values a service will let us change. Worth carrying, because a server
    // that will not take one does not necessarily say so: a Printer Application answers
    // `successful-ok` — "Printer attributes set." — and ignores an attribute outside this list,
    // so without it a change that did nothing is indistinguishable from one that worked.
    //
    // Both spellings, because the two eras disagree: a Printer Application says
    // `printer-settable-attributes` and the scheduler says the same with `-supported`.
    "printer-settable-attributes",
    "printer-settable-attributes-supported",
];

/// Re-reads attributes through the entry's `printer-uri-supported` URI.
///
/// For configured queues this normally targets the CUPS server. A discovered
/// printer may instead expose its device endpoint as `printer-uri-supported`.
///
/// Every requested attribute is asked for, not only the ones the entry is missing. Most of
/// these change while a printer is in use — where it says it is, what it says about itself,
/// whether it is taking work — so skipping the ones already held would make a refresh unable
/// to refresh: a value changed on the printer would be written, take effect, and never be
/// shown again until the daemon restarted.
pub(in crate::cups_backend) fn reload_attrs_from_printer_uri(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> BackendResult<()> {
    let printer_uri = printer.printer_uri().ok_or_else(|| {
        BackendError::Internal(format!("queue '{}' has no printer URI", printer.id()))
    })?;
    let request = printer_attrs_request(printer_uri, attrs)?;
    let response = send_ipp_request(request, printer_uri)?;

    merge_attrs(printer, attrs, response)
}

/// What a printer is asked for when reading its supplies.
///
/// `printer-supply` is what a printer says about its own supplies. The `marker-*`
/// attributes are what CUPS synthesises for a queue, asked for here because a printer
/// driven through a CUPS backend answers with those instead.
const SUPPLY_ATTRIBUTES: &[&str] = &[
    "printer-supply",
    "printer-supply-description",
    "marker-names",
    "marker-levels",
    "marker-colors",
    "marker-high-levels",
    "marker-low-levels",
];

/// Asks the printer itself what supplies it has.
///
/// Not the queue: CUPS only carries `marker-*` for a queue once that queue has printed
/// something, so a printer set up and never used would report nothing at all.
pub(in crate::cups_backend) fn supplies_from_device(
    destination: &Destination,
    printer: &PrinterEntry,
) -> BackendResult<Vec<SupplyLevel>> {
    let (device_uri, connection) = connect_to_device(destination, printer)?;
    let request = printer_attrs_request(
        &printer_uri_for_request(&device_uri, &connection),
        SUPPLY_ATTRIBUTES,
    )?;
    let response = request
        .send(&connection, connection.resource_path())
        .cups_err()?;
    ensure_success(&response, "Get-Printer-Attributes")?;

    Ok(supplies_from_response(&response))
}

/// Reads the supplies out of whichever form the printer answered in.
fn supplies_from_response(response: &IppResponse) -> Vec<SupplyLevel> {
    let supplies = supply_records(response);
    if !supplies.is_empty() {
        let descriptions = attr_strings(response, "printer-supply-description");
        let supplies = supplies.iter().map(String::as_str).collect::<Vec<_>>();
        let descriptions = descriptions.iter().map(String::as_str).collect::<Vec<_>>();

        return parse_printer_supplies(&supplies, &descriptions);
    }

    // A CUPS-driven printer answers with the marker attributes instead, which are
    // read the same way a queue's are.
    let mut reported = PrinterEntry::new("", "", false, HashMap::new());
    reported.merge_options(merge_response_attrs(response, SUPPLY_ATTRIBUTES));

    reported.supplies()
}

/// Reads `printer-supply`, which a printer sends as an octetString rather than as text.
///
/// CUPS will not read a value of one syntax as another, so asking for these as strings
/// answers nothing at all — which is why a printer that reports its supplies perfectly
/// well looked like one that reports none.
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

/// Re-reads attributes from the destination's underlying device URI.
///
/// As above, everything requested is asked for rather than only what is absent, because a
/// refresh exists to notice what changed.
pub(in crate::cups_backend) fn reload_attrs_from_device_uri(
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
    apply_connection_endpoint(printer, &connection);
    Ok(())
}

/// Returns the URI to name in a request sent over this connection.
///
/// A Printer Application decides which of its printers a request is about from
/// `printer-uri`, not from the path the request was sent to — verified by sending
/// one printer's path together with another printer's URI and getting the second
/// one back. A DNS-SD device URI names no printer at all, only a service
/// (`ipps://Name._ipps._tcp.local/`), so an application answers for whichever
/// printer is its default and every printer on it ends up wearing the default
/// printer's name, UUID and state. Sharing one UUID then merges printers that are
/// not the same printer.
///
/// `cupsConnectDest` has already resolved which printer that service meant, and
/// says so in the connection's resource path. This puts it back into the URI.
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
///
/// Answers `None` when the parts name no printer: either the endpoint went
/// unresolved, or it resolved to no more than the service that was asked for.
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
    let connection = destination
        .connect(ConnectionFlags::Device, Some(5000), None)
        .map_err(|source| BackendError::DeviceUnreachable {
            uri: device_uri.clone(),
            source,
        })?;
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
///
/// The value's own tag decides how to read it. Trying the text form first and falling back to
/// the numeric one when nothing came of it looks equivalent, but is not: an empty text value
/// yields no string, and asking a text attribute for an integer answers zero — so a printer
/// with its location cleared reported a location of `0`.
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

    /// The case that mattered: a DNS-SD service URI names no printer, so the
    /// resolved path is what says which printer on the application was meant.
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

    /// Nothing was resolved beyond the service that was asked for, so there is no
    /// printer here to name.
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
