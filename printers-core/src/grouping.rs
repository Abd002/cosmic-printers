use std::collections::HashMap;

use crate::{GroupedDevice, PrinterEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    uuid: Option<String>,
    endpoint: Option<(String, u16)>,
    uri: Option<String>,
}

impl DeviceIdentity {
    /// Builds identity from a UUID, device URI, and queue URI fallback.
    pub fn new(
        uuid: Option<&str>,
        device_uri: Option<&str>,
        fallback_uri: Option<&str>,
    ) -> Self {
        let uri = device_uri.or(fallback_uri);

        Self {
            uuid: uuid
                .map(str::trim)
                .filter(|uuid| !uuid.is_empty())
                .map(str::to_ascii_lowercase),
            endpoint: device_uri.and_then(parse_uri_endpoint),
            uri: uri.map(uri_identity),
        }
    }

    /// Compares UUID first, then host and port, then the normalized full URI.
    pub fn matches(&self, other: &Self) -> bool {
        if self.uuid.is_some() || other.uuid.is_some() {
            return self.uuid == other.uuid;
        }

        if let (Some(left_endpoint), Some(right_endpoint)) = (&self.endpoint, &other.endpoint) {
            return left_endpoint == right_endpoint;
        }

        self.uri.is_some() && self.uri == other.uri
    }

    pub fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    pub fn hostname(&self) -> Option<&str> {
        self.endpoint
            .as_ref()
            .map(|(hostname, _)| hostname.as_str())
    }

    pub fn port(&self) -> Option<u16> {
        self.endpoint.as_ref().map(|(_, port)| *port)
    }

    pub fn uri(&self) -> Option<&str> {
        self.uri.as_deref()
    }

    fn fill_missing_from(&mut self, other: Self) {
        if self.uuid.is_none() {
            self.uuid = other.uuid;
        }

        if self.endpoint.is_none() {
            self.endpoint = other.endpoint;
        }

        if self.uri.is_none() {
            self.uri = other.uri;
        }
    }
}

impl GroupedDevice {
    /// Starts a physical-device group with one printer queue.
    fn new(printer: PrinterEntry) -> Self {
        let identity = printer_identity(&printer);

        Self {
            identity,
            queues: vec![printer],
        }
    }

    /// Tests a queue against the identity stored from its original URI.
    fn matches(&self, printer: &PrinterEntry) -> bool {
        self.identity.matches(&printer_identity(printer))
    }

    /// Adds a matching queue and fills identity fields missing from the group.
    fn add(&mut self, printer: PrinterEntry) {
        let identity = printer_identity(&printer);

        self.identity.fill_missing_from(identity);
        self.queues.push(printer);
    }
}

/// Groups configured queues that appear to belong to the same physical device.
pub fn group_printers(printers: Vec<PrinterEntry>) -> Vec<GroupedDevice> {
    let mut devices = Vec::<GroupedDevice>::new();

    for printer in printers {
        if let Some(device) = devices.iter_mut().find(|device| device.matches(&printer)) {
            device.add(printer);
        } else {
            devices.push(GroupedDevice::new(printer));
        }
    }

    for device in &mut devices {
        device.queues.sort_by(|left, right| left.id.cmp(&right.id));
    }

    devices
}

/// Extracts the shared matching identity from a printer entry.
fn printer_identity(printer: &PrinterEntry) -> DeviceIdentity {
    DeviceIdentity::new(
        non_empty_option(&printer.options, "printer-uuid"),
        non_empty_option(&printer.options, "device-uri"),
        non_empty_option(&printer.options, "printer-uri-supported"),
    )
}

/// Reads a trimmed option and treats missing or empty values as absent.
fn non_empty_option<'a>(options: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Parses a URI into a lowercase hostname and explicit or default port.
pub fn parse_uri_endpoint(uri: &str) -> Option<(String, u16)> {
    let (scheme, rest) = uri.split_once("://")?;
    let authority = rest.split('/').next()?.rsplit('@').next()?.trim();
    if authority.is_empty() {
        return None;
    }

    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ipp" | "http" => Some(631),
        "ipps" => Some(631),
        "https" => Some(443),
        "socket" => Some(9100),
        "lpd" => Some(515),
        _ => None,
    };

    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let port = authority
            .get(end + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .and_then(|port| port.parse::<u16>().ok())
            .or(default_port)?;
        return Some((host.to_ascii_lowercase(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse::<u16>().ok()),
        _ => (authority, default_port),
    };

    Some((host.to_ascii_lowercase(), port?))
}

/// Removes query, fragment, and trailing slash data and lowercases a URI.
pub fn uri_prefix(uri: &str) -> String {
    uri.split(['?', '#'])
        .next()
        .unwrap_or(uri)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// Normalizes a URI for identity comparison and supplies IPP's default port.
fn uri_identity(uri: &str) -> String {
    let normalized = uri_prefix(uri);
    let Some((scheme, rest)) = normalized.split_once("://") else {
        return normalized;
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = match (scheme, authority.rsplit_once(':')) {
        ("ipp", None) | ("ipps", None) => format!("{authority}:631"),
        _ => authority.to_string(),
    };

    format!("{scheme}://{authority}/{path}")
}
