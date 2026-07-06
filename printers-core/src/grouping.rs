use std::cell::Cell;
use std::collections::HashMap;
use std::net::IpAddr;

use crate::{GroupedDevice, PrinterEntry};

/// Normalized identity evidence used to decide whether queues share a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    uuid: Option<String>,
    endpoint: Option<(String, u16)>,
    uri: Option<String>,
}

impl DeviceIdentity {
    /// Builds the normalized identity used to compare printer queues.
    pub fn new(
        uuid: Option<&str>,
        endpoint: Option<(String, u16)>,
        device_uri: Option<&str>,
        fallback_uri: Option<&str>,
    ) -> Self {
        let uri = device_uri.or(fallback_uri);
        Self {
            uuid: normalize_uuid(uuid),
            endpoint,
            uri: uri.map(uri_identity),
        }
    }

    /// Compares identities by UUID, then prepared endpoint, then normalized URI.
    pub fn matches(&self, other: &Self) -> bool {
        if let (Some(left), Some(right)) = (&self.uuid, &other.uuid)
            && left == right
        {
            return true;
        }

        if let (Some(left), Some(right)) = (&self.endpoint, &other.endpoint)
            && endpoints_match(left, right)
        {
            return true;
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

    fn match_keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(3);

        if let Some(uuid) = &self.uuid {
            keys.push(format!("uuid:{uuid}"));
        }
        if let Some((host, port)) = &self.endpoint {
            keys.push(format!("endpoint:{}", endpoint_match_key(host, *port)));
        }
        if let Some(uri) = &self.uri {
            keys.push(format!("uri:{uri}"));
        }

        keys
    }
}

fn endpoints_match(left: &(String, u16), right: &(String, u16)) -> bool {
    let (host_left, port_left) = left;
    let (host_right, port_right) = right;

    if !hosts_match(host_left, host_right) {
        return false;
    }

    !is_loopback_host(host_left) || port_left == port_right
}

fn hosts_match(left: &str, right: &str) -> bool {
    if is_loopback_host(left) && is_loopback_host(right) {
        return true;
    }

    match (parse_ip(left), parse_ip(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left.eq_ignore_ascii_case(right),
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || parse_ip(host).is_some_and(|ip| ip.is_loopback())
}

fn endpoint_match_key(host: &str, port: u16) -> String {
    if is_loopback_host(host) {
        return format!("loopback:{port}");
    }

    parse_ip(host)
        .map(|ip| format!("ip:{ip}"))
        .unwrap_or_else(|| format!("host:{}", host.to_ascii_lowercase()))
}

fn parse_ip(host: &str) -> Option<IpAddr> {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

fn normalize_uuid(uuid: Option<&str>) -> Option<String> {
    let uuid = uuid.map(str::trim).filter(|value| !value.is_empty())?;
    let lowered = uuid.to_ascii_lowercase();
    Some(
        lowered
            .strip_prefix("urn:uuid:")
            .unwrap_or(&lowered)
            .to_string(),
    )
}

impl GroupedDevice {
    fn new(printer: PrinterEntry) -> Self {
        let identity = printer_identity(&printer);
        Self {
            identity,
            queues: vec![printer],
        }
    }

    fn absorb(&mut self, other: Self) {
        self.identity.fill_missing_from(other.identity);
        self.queues.extend(other.queues);
    }
}

/// Groups configured queues that appear to belong to the same physical device.
pub fn group_printers(printers: Vec<PrinterEntry>) -> Vec<GroupedDevice> {
    let identities: Vec<DeviceIdentity> = printers.iter().map(printer_identity).collect();
    let printer_count = identities.len();
    let parent: Vec<Cell<usize>> = (0..printer_count).map(Cell::new).collect();
    let mut first_index_by_key = HashMap::<String, usize>::new();

    for (index, identity) in identities.iter().enumerate() {
        for key in identity.match_keys() {
            if let Some(&other) = first_index_by_key.get(&key) {
                union(&parent, other, index);
            } else {
                first_index_by_key.insert(key, index);
            }
        }
    }

    let mut slot_of_root: HashMap<usize, usize> = HashMap::new();
    let mut devices = Vec::<GroupedDevice>::new();

    for (index, printer) in printers.into_iter().enumerate() {
        let root = find(&parent, index);
        if let Some(&slot) = slot_of_root.get(&root) {
            devices[slot].absorb(GroupedDevice::new(printer));
        } else {
            slot_of_root.insert(root, devices.len());
            devices.push(GroupedDevice::new(printer));
        }
    }

    for device in &mut devices {
        device.queues.sort_by(|left, right| left.id.cmp(&right.id));
    }

    devices
}

/// Returns true when two printer entries appear to describe the same physical
/// device or queue.
pub fn printers_match(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    printer_identity(left).matches(&printer_identity(right))
}

fn printer_identity(printer: &PrinterEntry) -> DeviceIdentity {
    DeviceIdentity::new(
        non_empty_option(&printer.options, "device-uuid"),
        printer_endpoint(printer),
        non_empty_option(&printer.options, "device-uri"),
        non_empty_option(&printer.options, "printer-uri-supported"),
    )
}

fn printer_endpoint(printer: &PrinterEntry) -> Option<(String, u16)> {
    let host = non_empty_option(&printer.options, "dnssd-address")
        .map(ToString::to_string)
        .or_else(|| printer.hostname.clone())?;

    Some((host, printer.port?))
}

fn non_empty_option<'a>(options: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn find(parent: &[Cell<usize>], index: usize) -> usize {
    let stored = parent[index].get();
    if stored == index {
        index
    } else {
        let root = find(parent, stored);
        parent[index].set(root);
        root
    }
}

fn union(parent: &[Cell<usize>], a: usize, b: usize) {
    let root_a = find(parent, a);
    let root_b = find(parent, b);
    if root_a != root_b {
        parent[root_a].set(root_b);
    }
}

fn uri_prefix(uri: &str) -> String {
    uri.split(['?', '#'])
        .next()
        .unwrap_or(uri)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn uri_identity(uri: &str) -> String {
    let normalized = uri_prefix(uri);
    let Some((scheme, rest)) = normalized.split_once("://") else {
        return normalized;
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = match (scheme, authority.rsplit_once(':')) {
        ("ipp", None) | ("ipps", None) => format!("{authority}:631"),
        ("http", None) => format!("{authority}:80"),
        ("https", None) => format!("{authority}:443"),
        _ => authority.to_string(),
    };

    format!("{scheme}://{authority}/{path}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrinterStatus;

    fn insert_test_endpoint(options: &mut HashMap<String, String>, uri: &str) {
        let Some((host, port)) = parse_uri_endpoint(uri) else {
            return;
        };

        options.insert("test-endpoint-host".to_string(), host);
        options.insert("test-endpoint-port".to_string(), port.to_string());
    }

    fn parse_uri_endpoint(uri: &str) -> Option<(String, u16)> {
        let (scheme, rest) = uri.split_once("://")?;
        let authority = rest.split('/').next()?.rsplit('@').next()?.trim();
        if authority.is_empty() {
            return None;
        }

        let default_port = match scheme.to_ascii_lowercase().as_str() {
            "ipp" | "ipps" => 631,
            "http" => 80,
            "https" => 443,
            _ => return None,
        };

        if authority.starts_with('[') {
            let end = authority.find(']')?;
            let host = &authority[..=end];
            let port = authority
                .get(end + 1..)
                .and_then(|suffix| suffix.strip_prefix(':'))
                .and_then(|port| port.parse::<u16>().ok())
                .unwrap_or(default_port);
            return Some((host.to_ascii_lowercase(), port));
        }

        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse::<u16>().ok()),
            _ => (authority, Some(default_port)),
        };

        Some((host.to_ascii_lowercase(), port?))
    }

    fn identity(
        uuid: Option<&str>,
        endpoint: Option<(&str, u16)>,
        device_uri: Option<&str>,
        fallback_uri: Option<&str>,
    ) -> DeviceIdentity {
        DeviceIdentity::new(
            uuid,
            endpoint.map(|(host, port)| (host.to_string(), port)),
            device_uri,
            fallback_uri,
        )
    }

    #[test]
    fn same_remote_host_different_ports_match() {
        let a = identity(None, Some(("192.168.1.50", 631)), None, None);
        let b = identity(None, Some(("192.168.1.50", 8000)), None, None);
        assert!(a.matches(&b));
        assert!(b.matches(&a));
    }

    #[test]
    fn different_remote_hosts_do_not_match() {
        let a = identity(None, Some(("192.168.1.50", 631)), None, None);
        let b = identity(None, Some(("192.168.1.51", 631)), None, None);
        assert!(!a.matches(&b));
    }

    #[test]
    fn same_localhost_different_ports_do_not_match() {
        let a = identity(None, Some(("localhost", 60001)), None, None);
        let b = identity(None, Some(("localhost", 60002)), None, None);
        assert!(!a.matches(&b));
    }

    #[test]
    fn same_localhost_same_port_matches() {
        let a = identity(None, Some(("localhost", 60000)), None, None);
        let b = identity(None, Some(("localhost", 60000)), None, None);
        assert!(a.matches(&b));
    }

    #[test]
    fn loopback_ip_literal_behaves_like_localhost() {
        let a = identity(None, Some(("127.0.0.1", 60001)), None, None);
        let b = identity(None, Some(("127.0.0.1", 60002)), None, None);
        assert!(!a.matches(&b));

        let c = identity(None, Some(("127.0.0.1", 60001)), None, None);
        let d = identity(None, Some(("localhost", 60001)), None, None);
        assert!(c.matches(&d));
    }

    #[test]
    fn ipv6_loopback_requires_matching_port() {
        let a = identity(None, Some(("[::1]", 60001)), None, None);
        let b = identity(None, Some(("[::1]", 60002)), None, None);
        assert!(!a.matches(&b));
    }

    #[test]
    fn ipv6_equivalent_forms_match_as_same_address() {
        let a = identity(None, Some(("[2001:db8::1]", 631)), None, None);
        let b = identity(
            None,
            Some(("[2001:0db8:0000:0000:0000:0000:0000:0001]", 631)),
            None,
            None,
        );
        assert!(a.matches(&b));
    }

    #[test]
    fn same_uuid_different_hosts_match() {
        let a = identity(
            Some("4509a323-cc83-2540-0000-000000000000"),
            Some(("192.168.1.50", 631)),
            None,
            None,
        );
        let b = identity(
            Some("urn:uuid:4509A323-CC83-2540-0000-000000000000"),
            Some(("printer.lan", 631)),
            None,
            None,
        );
        assert!(a.matches(&b));
    }

    #[test]
    fn different_uuids_same_host_still_match_via_host() {
        let print = identity(
            Some("uuid-print-service"),
            Some(("192.168.1.20", 631)),
            None,
            None,
        );
        let fax = identity(
            Some("uuid-fax-service"),
            Some(("192.168.1.20", 631)),
            None,
            None,
        );
        assert!(print.matches(&fax));
    }

    #[test]
    fn uuid_present_on_only_one_side_does_not_block_host_match() {
        let ipp_faxout = identity(None, Some(("192.168.1.50", 8000)), None, None);
        let ipp_destination = identity(Some("some-uuid"), Some(("192.168.1.50", 631)), None, None);
        assert!(ipp_faxout.matches(&ipp_destination));
    }

    #[test]
    fn different_uuids_different_hosts_do_not_match() {
        let a = identity(Some("uuid-a"), Some(("192.168.1.20", 631)), None, None);
        let b = identity(Some("uuid-b"), Some(("192.168.1.21", 631)), None, None);
        assert!(!a.matches(&b));
    }

    #[test]
    fn pairwise_matches_alone_is_not_transitive() {
        let a = identity(
            Some("shared-uuid"),
            None,
            None,
            Some("ipp://localhost:631/printers/local-queue"),
        );
        let b = identity(
            Some("shared-uuid"),
            Some(("10.0.0.5", 631)),
            Some("ipp://10.0.0.5:631/ipp/print"),
            None,
        );
        let c = identity(
            None,
            Some(("10.0.0.5", 8000)),
            Some("ipp://10.0.0.5:8000/ipp/faxout"),
            None,
        );

        assert!(a.matches(&b));
        assert!(b.matches(&c));
        assert!(!a.matches(&c));
    }

    fn printer(id: &str, device_uri: &str, fallback_uri: &str, uuid: Option<&str>) -> PrinterEntry {
        let mut options = HashMap::new();
        if !device_uri.is_empty() {
            options.insert("device-uri".to_string(), device_uri.to_string());
            insert_test_endpoint(&mut options, device_uri);
        }
        if !fallback_uri.is_empty() {
            options.insert(
                "printer-uri-supported".to_string(),
                fallback_uri.to_string(),
            );
        }
        if let Some(uuid) = uuid {
            options.insert("device-uuid".to_string(), uuid.to_string());
        }
        let endpoint = options.get("test-endpoint-host").cloned().zip(
            options
                .get("test-endpoint-port")
                .and_then(|port| port.parse().ok()),
        );

        PrinterEntry {
            id: id.to_string(),
            name: id.to_string(),
            is_default: false,
            printer_local_uri: fallback_uri.to_string(),
            status: PrinterStatus::Ready,
            queue_status: String::new(),
            location: String::new(),
            model: String::new(),
            device_uri: device_uri.to_string(),
            hostname: endpoint.as_ref().map(|(host, _)| host.clone()),
            port: endpoint.map(|(_, port)| port),
            web_page: None,
            driver_version: String::new(),
            paper_size_idx: 0,
            print_sides_idx: 0,
            options,
            supplies: Vec::new(),
            paper_sizes: Vec::new(),
            print_sides: Vec::new(),
        }
    }

    #[test]
    fn groups_print_and_fax_from_same_multi_function_device() {
        let printers = vec![
            printer(
                "hp-print",
                "ipp://192.168.1.20:631/ipp/print",
                "",
                Some("uuid-print"),
            ),
            printer(
                "hp-fax",
                "ipp://192.168.1.20:631/ipp/faxout",
                "",
                Some("uuid-fax"),
            ),
        ];
        let groups = group_printers(printers);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].queues().len(), 2);
    }

    #[test]
    fn keeps_independent_local_printer_applications_separate() {
        let printers = vec![
            printer("app-a-print", "ipp://localhost:60001/ipp/print", "", None),
            printer("app-b-print", "ipp://localhost:60002/ipp/print", "", None),
        ];
        let groups = group_printers(printers);
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn transitively_groups_across_mixed_evidence_regardless_of_order() {
        let make = || {
            vec![
                printer(
                    "a-queue",
                    "",
                    "ipp://localhost:631/printers/local-queue",
                    Some("shared-uuid"),
                ),
                printer(
                    "b-ipp",
                    "ipp://10.0.0.5:631/ipp/print",
                    "",
                    Some("shared-uuid"),
                ),
                printer("c-faxout", "ipp://10.0.0.5:8000/ipp/faxout", "", None),
            ]
        };

        let mut forward = make();
        let mut reversed = make();
        reversed.reverse();

        assert_eq!(group_printers(forward.clone()).len(), 1);
        assert_eq!(group_printers(reversed.clone()).len(), 1);

        forward.swap(1, 2);
        assert_eq!(group_printers(forward).len(), 1);
        reversed.swap(0, 2);
        assert_eq!(group_printers(reversed).len(), 1);
    }
}
