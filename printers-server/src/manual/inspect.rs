//! Asking an address what answers there: a Printer Application, a CUPS server, one IPP printer,
//! or only a raw port.

use cosmic_settings_printers_core::Error;
use cups_rs::{IppAttribute, IppOperation, IppRequest, IppResponse, IppTag, IppValueTag};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use super::target::{Target, parse_target};
use crate::ipp::{IppTimeouts, add_requesting_user, send_to_with_timeouts};

/// Short, because the user is waiting on the answer.
const IPP_TIMEOUTS: IppTimeouts = IppTimeouts {
    connect_ms: 2_000,
    response_seconds: 8.0,
};
const TCP_TIMEOUT: Duration = Duration::from_millis(1_500);

const CUPS_GET_PRINTERS: IppOperation = IppOperation::Other(0x4002);

/// Where a printer is most likely to live when the user gave no path.
const PRINTER_RESOURCES: &[&str] = &["/ipp/print", "/ipp/printer", "/ipp", "/printer", "/"];

/// The attributes asked of every printer in a listing.
const LISTED_ATTRIBUTES: &[&str] = &[
    "printer-name",
    "printer-info",
    "printer-uri-supported",
    "printer-make-and-model",
    "printer-device-id",
    "document-format-supported",
    "ipp-features-supported",
];

/// What kind of service answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EndpointKind {
    /// An IPP system service that can create printers.
    PrinterApplication,
    /// A CUPS scheduler that lists its queues.
    CupsServer,
    /// One IPP printer.
    IppPrinter,
    /// No IPP, but a raw (9100) or LPD (515) port is open.
    RawOnly,
    /// Nothing usable answered.
    Silent,
}

/// One printer found at the address.
#[derive(Clone, Debug)]
pub(super) struct FoundPrinter {
    pub(super) name: String,
    /// The URI to print to (the best of `uris`).
    pub(super) uri: String,
    /// Every URI the printer lists for itself.
    pub(super) uris: Vec<String>,
    pub(super) make_and_model: Option<String>,
    pub(super) info: Option<String>,
    /// IEEE-1284 device ID, which lets a Printer Application pick a driver.
    pub(super) device_id: Option<String>,
    pub(super) formats: Vec<String>,
    /// `ipp-features-supported`, where a printer says it is IPP Everywhere.
    pub(super) ipp_features: Vec<String>,
}

/// What answered at an address.
#[derive(Clone, Debug)]
pub(super) struct EndpointReport {
    /// The address as read.
    pub(super) target: Target,
    pub(super) kind: EndpointKind,
    /// Every printer found, in the order the service gave them.
    pub(super) printers: Vec<FoundPrinter>,
    /// Raw ports that accepted a connection, the one typed first.
    pub(super) raw_ports_open: Vec<RawPort>,
}

/// A port that takes a print job as is, without IPP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RawPort {
    /// AppSocket (JetDirect): 9100, or another port a print server gives each of its printers.
    AppSocket(u16),
    /// LPD: 515.
    Lpd(u16),
}

impl RawPort {
    pub(super) fn port(self) -> u16 {
        match self {
            Self::AppSocket(port) | Self::Lpd(port) => port,
        }
    }
}

/// Finds out what is at an address and lists every printer it exposes.
///
/// A bare host is tried on port 631. IPP is tried over plain HTTP and over TLS, because the
/// port alone does not say which. In order, the address is asked whether it is an IPP system
/// service, a CUPS server, or a single IPP printer; the raw ports 9100 and 515 are checked last.
pub(super) fn inspect(input: &str) -> Result<EndpointReport, Error> {
    let target = parse_target(input)?;
    let mut report = EndpointReport {
        target: target.clone(),
        kind: EndpointKind::Silent,
        printers: Vec::new(),
        raw_ports_open: Vec::new(),
    };

    let raw_only = matches!(target.scheme.as_deref(), Some("socket" | "lpd"));
    let port = target.port.unwrap_or(631);
    let host_part = host_in_uri(&target.host);

    let ipp_port_open = !raw_only && tcp_open(&target.host, port);
    if !raw_only {
        tracing::debug!(
            host = target.host,
            port,
            open = ipp_port_open,
            "checked ipp port"
        );
    }

    if ipp_port_open {
        let schemes: &[&str] = match target.scheme.as_deref() {
            Some("ipps" | "https") => &["ipps", "ipp"],
            _ => &["ipp", "ipps"],
        };
        for scheme in schemes {
            if try_ipp(&mut report, &target, scheme, &host_part, port) {
                break;
            }
        }
    }

    let ipp_answered = report.kind != EndpointKind::Silent;
    for raw in raw_ports(&target, ipp_port_open && !ipp_answered) {
        // The IPP port was already found open, so it is not asked twice.
        let already_known_open = !raw_only && port == raw.port() && ipp_port_open;
        if already_known_open || tcp_open(&target.host, raw.port()) {
            report.raw_ports_open.push(raw);
        }
    }
    tracing::debug!(raw_ports_open = ?report.raw_ports_open, "checked raw ports");

    if report.kind == EndpointKind::Silent && !report.raw_ports_open.is_empty() {
        report.kind = EndpointKind::RawOnly;
    }

    Ok(report)
}

/// The raw ports worth asking, the typed one first.
///
/// A typed port that is open but does not speak IPP is taken as AppSocket, the way a print server
/// gives each of its printers a port of its own (9100, 9101, ...).
fn raw_ports(target: &Target, typed_port_is_raw: bool) -> Vec<RawPort> {
    match (target.scheme.as_deref(), target.port) {
        (Some("socket"), Some(port)) => return vec![RawPort::AppSocket(port)],
        (Some("lpd"), Some(port)) => return vec![RawPort::Lpd(port)],
        _ => {}
    }

    let mut ports = Vec::with_capacity(3);
    if typed_port_is_raw
        && let Some(port) = target.port
        && !matches!(port, 9100 | 515)
    {
        ports.push(RawPort::AppSocket(port));
    }
    ports.extend([RawPort::AppSocket(9100), RawPort::Lpd(515)]);

    ports
}

/// Tries every kind of IPP service on one scheme. Returns whether one answered.
fn try_ipp(
    report: &mut EndpointReport,
    target: &Target,
    scheme: &str,
    host_part: &str,
    port: u16,
) -> bool {
    let base = format!("{scheme}://{host_part}:{port}");

    // 1. A Printer Application's system service.
    let system_uri = format!("{base}/ipp/system");
    match call_ok(&system_uri, IppOperation::GetSystemAttributes, |request| {
        add_uri(request, "system-uri", &system_uri)?;
        add_keywords(
            request,
            "requested-attributes",
            &[
                "system-name",
                "system-make-and-model",
                "operations-supported",
                "smi55357-device-uri-schemes-supported",
            ],
        )
    }) {
        Ok(_) => {
            report.kind = EndpointKind::PrinterApplication;
            match call_ok(&system_uri, IppOperation::GetPrinters, |request| {
                add_uri(request, "system-uri", &system_uri)?;
                add_keywords(request, "requested-attributes", LISTED_ATTRIBUTES)
            }) {
                Ok(response) => report.printers = parse_printer_list(&response, scheme),
                Err(why) => tracing::debug!(system_uri, why, "Get-Printers failed"),
            }
            return true;
        }
        Err(why) => tracing::debug!(system_uri, why, "no printer application answered"),
    }

    // 2. A CUPS server listing its queues.
    let root = format!("{base}/");
    match call(&root, CUPS_GET_PRINTERS, |request| {
        add_keywords(request, "requested-attributes", LISTED_ATTRIBUTES)
    }) {
        // CUPS says "not found" rather than returning an empty list when it holds no queues
        // that this client may see. Only a CUPS scheduler answers that way to this operation.
        Ok(response) if response.status() == cups_rs::IppStatus::ErrorNotFound => {
            report.kind = EndpointKind::CupsServer;
            return true;
        }
        Ok(response) if response.status().is_successful() => {
            report.kind = EndpointKind::CupsServer;
            report.printers = parse_printer_list(&response, scheme);
            return true;
        }
        Ok(response) => {
            tracing::debug!(root, status = ?response.status(), "CUPS-Get-Printers refused");
        }
        Err(why) => tracing::debug!(root, why, "no CUPS server answered"),
    }

    // 3. A single IPP printer, at the path given or the usual ones.
    let mut resources: Vec<String> = Vec::new();
    if let Some(path) = &target.path {
        resources.push(path.clone());
    }
    resources.extend(PRINTER_RESOURCES.iter().map(ToString::to_string));
    resources.dedup();

    for resource in resources {
        let uri = format!("{base}{resource}");
        match call_ok(&uri, IppOperation::GetPrinterAttributes, |request| {
            add_uri(request, "printer-uri", &uri)?;
            add_keywords(request, "requested-attributes", LISTED_ATTRIBUTES)
        }) {
            Ok(response) => {
                report.kind = EndpointKind::IppPrinter;
                report.printers.push(FoundPrinter {
                    name: first(&response, "printer-name")
                        .or_else(|| first(&response, "printer-info"))
                        .or_else(|| first(&response, "printer-make-and-model"))
                        .unwrap_or_else(|| uri.clone()),
                    uri,
                    uris: strings(&response, "printer-uri-supported"),
                    make_and_model: first(&response, "printer-make-and-model"),
                    info: first(&response, "printer-info"),
                    device_id: first(&response, "printer-device-id"),
                    formats: strings(&response, "document-format-supported"),
                    ipp_features: strings(&response, "ipp-features-supported"),
                });
                return true;
            }
            Err(why) => tracing::debug!(uri, why, "no ipp printer answered"),
        }
    }

    false
}

/// Splits a listing into printers. A nameless attribute separates them.
fn parse_printer_list(response: &IppResponse, scheme: &str) -> Vec<FoundPrinter> {
    let mut printers = Vec::new();
    let mut current: Vec<(String, Vec<String>)> = Vec::new();

    let mut finish = |current: &mut Vec<(String, Vec<String>)>| {
        let take = |name: &str| -> Vec<String> {
            current
                .iter()
                .find(|(attribute, _)| attribute == name)
                .map(|(_, values)| values.clone())
                .unwrap_or_default()
        };
        let one = |name: &str| take(name).into_iter().next();

        if let Some(name) = one("printer-name") {
            // Prefer the URI on the scheme that answered.
            let uris = take("printer-uri-supported");
            let uri = uris
                .iter()
                .find(|uri| uri.starts_with(&format!("{scheme}://")))
                .or_else(|| uris.first())
                .cloned()
                .unwrap_or_default();
            printers.push(FoundPrinter {
                name,
                uri,
                uris,
                make_and_model: one("printer-make-and-model"),
                info: one("printer-info"),
                device_id: one("printer-device-id"),
                formats: take("document-format-supported"),
                ipp_features: take("ipp-features-supported"),
            });
        }
        current.clear();
    };

    for attribute in response.attributes() {
        let Some(name) = attribute.name() else {
            finish(&mut current);
            continue;
        };
        if attribute.group_tag() != Some(IppTag::Printer) {
            continue;
        }
        current.push((name, values(&attribute)));
    }
    finish(&mut current);

    printers
}

fn tcp_open(host: &str, port: u16) -> bool {
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };

    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, TCP_TIMEOUT).is_ok())
}

/// Sends one request and returns the response, whatever its status.
fn call(
    uri: &str,
    operation: IppOperation,
    build: impl FnOnce(&mut IppRequest) -> Result<(), cups_rs::Error>,
) -> Result<IppResponse, String> {
    let mut request = IppRequest::new(operation).map_err(|error| error.to_string())?;
    build(&mut request).map_err(|error| error.to_string())?;
    add_requesting_user(&mut request).map_err(|error| error.to_string())?;

    send_to_with_timeouts(request, uri, IPP_TIMEOUTS).map_err(|error| error.to_string())
}

/// Like `call`, but a non-success status is an error carrying the status.
fn call_ok(
    uri: &str,
    operation: IppOperation,
    build: impl FnOnce(&mut IppRequest) -> Result<(), cups_rs::Error>,
) -> Result<IppResponse, String> {
    let response = call(uri, operation, build)?;
    if response.status().is_successful() {
        Ok(response)
    } else {
        Err(format!("{:?}", response.status()))
    }
}

fn add_uri(request: &mut IppRequest, name: &str, value: &str) -> Result<(), cups_rs::Error> {
    request.add_string(IppTag::Operation, IppValueTag::Uri, name, value)
}

fn add_keywords(
    request: &mut IppRequest,
    name: &str,
    values: &[&str],
) -> Result<(), cups_rs::Error> {
    request.add_strings(IppTag::Operation, IppValueTag::Keyword, name, values)
}

fn values(attribute: &IppAttribute) -> Vec<String> {
    (0..attribute.count())
        .map(|index| match attribute.value_tag() {
            IppValueTag::Integer | IppValueTag::Enum => attribute.get_integer(index).to_string(),
            IppValueTag::Boolean => attribute.get_boolean(index).to_string(),
            _ => attribute.get_string(index).unwrap_or_default(),
        })
        .filter(|value| !value.is_empty())
        .collect()
}

fn strings(response: &IppResponse, name: &str) -> Vec<String> {
    response
        .find_attribute(name, None)
        .map(|attribute| values(&attribute))
        .unwrap_or_default()
}

fn first(response: &IppResponse, name: &str) -> Option<String> {
    strings(response, name).into_iter().next()
}

/// Brackets an IPv6 literal so it can carry a port in a URI.
pub(super) fn host_in_uri(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(input: &str) -> Target {
        parse_target(input).expect("a target")
    }

    #[test]
    fn a_bare_host_asks_appsocket_and_lpd() {
        assert_eq!(
            raw_ports(&target("192.0.2.10"), false),
            [RawPort::AppSocket(9100), RawPort::Lpd(515)]
        );
    }

    #[test]
    fn a_typed_port_without_ipp_is_asked_as_appsocket_first() {
        assert_eq!(
            raw_ports(&target("192.0.2.10:9101"), true),
            [
                RawPort::AppSocket(9101),
                RawPort::AppSocket(9100),
                RawPort::Lpd(515)
            ]
        );
    }

    #[test]
    fn a_typed_port_that_speaks_ipp_is_not_appsocket() {
        assert_eq!(
            raw_ports(&target("192.0.2.10:8631"), false),
            [RawPort::AppSocket(9100), RawPort::Lpd(515)]
        );
    }

    #[test]
    fn the_well_known_raw_ports_are_not_asked_twice() {
        assert_eq!(
            raw_ports(&target("192.0.2.10:515"), true),
            [RawPort::AppSocket(9100), RawPort::Lpd(515)]
        );
    }

    #[test]
    fn a_typed_raw_uri_asks_only_its_own_port() {
        assert_eq!(
            raw_ports(&target("socket://192.0.2.10:19100"), false),
            [RawPort::AppSocket(19100)]
        );
        assert_eq!(
            raw_ports(&target("lpd://192.0.2.10/queue"), false),
            [RawPort::Lpd(515)]
        );
    }

    #[test]
    fn an_ipv6_host_is_bracketed_in_a_uri() {
        assert_eq!(host_in_uri("fe80::1"), "[fe80::1]");
        assert_eq!(host_in_uri("printer.local"), "printer.local");
    }
}
