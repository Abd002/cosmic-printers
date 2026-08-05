use super::conversion::refresh_printer_endpoint;
use crate::error::{BackendError, BackendResult};
use crate::ipp::{CupsResultExt, ensure_success, printer_attrs_request, send_ipp_request};
use cosmic_settings_printers_core::{EndpointSource, PrinterEntry, is_local_address};
use cups_rs::{ConnectionFlags, Destination, HttpConnection, IppResponse};

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
];

/// Fetches missing attributes through the entry's `printer-uri-supported` URI.
///
/// For configured queues this normally targets the CUPS server. A discovered
/// printer may instead expose its device endpoint as `printer-uri-supported`.
pub(in crate::cups_backend) fn fill_missing_attrs_from_printer_uri(
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> BackendResult<()> {
    let missing = missing_attrs(printer, attrs);

    if missing.is_empty() {
        return Ok(());
    }

    let printer_uri = printer.printer_uri().ok_or_else(|| {
        BackendError::Internal(format!("queue '{}' has no printer URI", printer.id()))
    })?;
    let request = printer_attrs_request(printer_uri, &missing)?;
    let response = send_ipp_request(request, printer_uri)?;

    merge_missing_attrs(printer, &missing, response)
}

/// Fetches missing attributes from the destination's underlying device URI.
pub(in crate::cups_backend) fn fill_missing_attrs_from_device_uri(
    destination: &Destination,
    printer: &mut PrinterEntry,
    attrs: &[&str],
) -> BackendResult<()> {
    let (device_uri, connection) = connect_to_device(destination, printer)?;
    apply_connection_endpoint(printer, &connection);

    let missing = missing_attrs(printer, attrs);
    if missing.is_empty() {
        return Ok(());
    }
    let request =
        printer_attrs_request(&printer_uri_for_request(&device_uri, &connection), &missing)?;
    let response = request
        .send(&connection, connection.resource_path())
        .cups_err()?;

    merge_missing_attrs(printer, &missing, response)?;
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
        device_uri,
        connection.hostname().as_deref(),
        connection.port(),
        connection.resource_path(),
    )
}

fn printer_uri_from_parts(
    device_uri: &str,
    host: Option<&str>,
    port: Option<u16>,
    resource: &str,
) -> String {
    // Nothing was resolved that the device URI does not already say.
    if resource.is_empty() || resource == "/" {
        return device_uri.to_string();
    }

    let (Some(host), Some(port)) = (host, port) else {
        return device_uri.to_string();
    };
    let scheme = if device_uri.starts_with("ipps") {
        "ipps"
    } else {
        "ipp"
    };
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    format!("{scheme}://{host}:{port}{resource}")
}

fn connect_to_device(
    destination: &Destination,
    printer: &mut PrinterEntry,
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

fn missing_attrs<'a>(printer: &PrinterEntry, attrs: &'a [&str]) -> Vec<&'a str> {
    attrs
        .iter()
        .copied()
        .filter(|attr| printer.option(attr).is_none())
        .collect()
}

fn merge_missing_attrs(
    printer: &mut PrinterEntry,
    missing: &[&str],
    response: IppResponse,
) -> BackendResult<()> {
    ensure_success(&response, "Get-Printer-Attributes")?;

    printer.merge_options(merge_response_attrs(&response, missing));
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

#[cfg(test)]
mod tests {
    use super::printer_uri_from_parts;

    /// The case that mattered: a DNS-SD service URI names no printer, so the
    /// resolved path is what says which printer on the application was meant.
    #[test]
    fn a_dns_sd_service_uri_gains_the_printer_it_resolved_to() {
        assert_eq!(
            printer_uri_from_parts(
                "ipps://Acme_Laser._ipps._tcp.local/",
                Some("desktop.local"),
                Some(8001),
                "/ipp/print/Acme_Laser",
            ),
            "ipps://desktop.local:8001/ipp/print/Acme_Laser"
        );
    }

    #[test]
    fn a_plain_scheme_stays_plain() {
        assert_eq!(
            printer_uri_from_parts(
                "ipp://Acme_Laser._ipp._tcp.local/",
                Some("desktop.local"),
                Some(8001),
                "/ipp/print/Acme_Laser",
            ),
            "ipp://desktop.local:8001/ipp/print/Acme_Laser"
        );
    }

    #[test]
    fn an_address_literal_is_bracketed() {
        assert_eq!(
            printer_uri_from_parts(
                "ipps://Acme_Laser._ipps._tcp.local/",
                Some("fe80::1"),
                Some(8001),
                "/ipp/print/Acme_Laser",
            ),
            "ipps://[fe80::1]:8001/ipp/print/Acme_Laser"
        );
    }

    /// Nothing was resolved beyond what the URI already said, so it is left alone
    /// rather than rebuilt into something equivalent but different.
    #[test]
    fn a_uri_that_resolved_to_nothing_more_is_left_alone() {
        for resource in ["", "/"] {
            assert_eq!(
                printer_uri_from_parts(
                    "ipp://printer.local:631/ipp/print",
                    Some("printer.local"),
                    Some(631),
                    resource,
                ),
                "ipp://printer.local:631/ipp/print"
            );
        }
    }

    #[test]
    fn an_unresolved_endpoint_is_left_alone() {
        assert_eq!(
            printer_uri_from_parts(
                "ipps://Acme_Laser._ipps._tcp.local/",
                None,
                Some(8001),
                "/ipp/print/Acme_Laser",
            ),
            "ipps://Acme_Laser._ipps._tcp.local/"
        );
    }
}
