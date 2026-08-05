use std::collections::HashMap;
use std::net::IpAddr;

use crate::{GroupedDevice, PrinterApplication, PrinterEntry};
use nix::ifaddrs::getifaddrs;
use nix::sys::socket::SockaddrStorage;

/// Normalized identity evidence used to decide whether queues share a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceIdentity {
    uuid: Option<String>,
    endpoint: Option<(String, u16)>,
    uri: Option<String>,
}

impl DeviceIdentity {
    /// Builds the normalized identity used to compare printer queues.
    fn new(
        uuid: Option<&str>,
        endpoint: Option<(String, u16)>,
        device_uri: Option<&str>,
        fallback_uri: Option<&str>,
    ) -> Self {
        let uri = device_uri.or(fallback_uri);
        Self {
            uuid: normalize_uuid(uuid),
            endpoint: endpoint.map(normalize_endpoint),
            uri: uri.map(uri_identity),
        }
    }

    /// Compares identities by UUID, then prepared endpoint, then normalized URI.
    fn matches(&self, other: &Self) -> bool {
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

    pub(crate) fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    pub(crate) fn hostname(&self) -> Option<&str> {
        self.endpoint
            .as_ref()
            .map(|(hostname, _)| hostname.as_str())
    }

    pub(crate) fn port(&self) -> Option<u16> {
        self.endpoint.as_ref().map(|(_, port)| *port)
    }

    pub(crate) fn uri(&self) -> Option<&str> {
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

    /// Returns the keys that name one service.
    ///
    /// Each of these says *which* service, not merely where one is: an identifier
    /// the device reported, or the exact address it answers on. A Printer
    /// Application has only the last of them.
    fn service_keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(3);

        if let Some(uuid) = &self.uuid {
            keys.push(format!("uuid:{uuid}"));
        }
        if let Some((host, port)) = &self.endpoint {
            keys.push(format!("service:{}:{port}", host.to_ascii_lowercase()));
        }
        if let Some(uri) = &self.uri {
            keys.push(format!("uri:{uri}"));
        }

        keys
    }

    /// Returns the key that says only where a device is, not which service on it.
    ///
    /// A remote host is named without its port, which is what keeps a remote CUPS
    /// server's queues, or a multi-function device's print and fax queues, in one
    /// group. That is too coarse to name a Printer Application, because several of
    /// them share one host, so this may only group devices no application claimed.
    fn location_key(&self) -> Option<String> {
        let (host, port) = self.endpoint.as_ref()?;

        Some(format!("endpoint:{}", endpoint_match_key(host, *port)))
    }
}

fn endpoints_match(left: &(String, u16), right: &(String, u16)) -> bool {
    let (host_left, port_left) = left;
    let (host_right, port_right) = right;

    if !hosts_match(host_left, host_right) {
        return false;
    }

    !host_left.eq_ignore_ascii_case("localhost") || port_left == port_right
}

fn hosts_match(left: &str, right: &str) -> bool {
    match (parse_ip(left), parse_ip(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left.eq_ignore_ascii_case(right),
    }
}

fn host_is_known_local(host: &str) -> bool {
    if let Some(ip) = parse_ip(host) {
        return is_local_address(ip);
    }

    host.eq_ignore_ascii_case("localhost")
}

/// Returns whether a hostname or address literal refers to this machine.
///
/// Accepts `localhost`, a loopback literal, and any address assigned to a local
/// interface. A name that would need resolving to answer is treated as remote,
/// because deciding locality must not block on DNS.
pub fn host_is_local(host: &str) -> bool {
    host_is_known_local(host)
}

fn endpoint_match_key(host: &str, port: u16) -> String {
    if host.eq_ignore_ascii_case("localhost") {
        return format!("local:{port}");
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

fn normalize_endpoint((host, port): (String, u16)) -> (String, u16) {
    (host.to_ascii_lowercase(), port)
}

#[doc(hidden)]
pub fn is_local_address(target: IpAddr) -> bool {
    if target.is_loopback() {
        return true;
    }

    let Ok(addrs) = getifaddrs() else {
        return false;
    };

    addrs
        .filter_map(|ifaddr| ifaddr.address)
        .filter_map(|address| sockaddr_to_ip(&address))
        .any(|address| address == target)
}

fn sockaddr_to_ip(addr: &SockaddrStorage) -> Option<IpAddr> {
    if let Some(addr) = addr.as_sockaddr_in() {
        return Some(IpAddr::V4(addr.ip()));
    }

    if let Some(addr) = addr.as_sockaddr_in6() {
        return Some(IpAddr::V6(addr.ip()));
    }

    None
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

/// Returns, for each item, whether a Printer Application shares its group.
fn claimed_by_an_application(items: &[GroupingItem], sets: &mut DisjointSet) -> Vec<bool> {
    let application_roots = (0..items.len())
        .filter(|index| matches!(items[*index], GroupingItem::Application(_)))
        .map(|index| sets.find(index))
        .collect::<Vec<_>>();

    (0..items.len())
        .map(|index| {
            let root = sets.find(index);
            application_roots.contains(&root)
        })
        .collect()
}

enum GroupingItem {
    Printer(PrinterEntry),
    /// Boxed because a Printer Application carries its probed capabilities and
    /// is much larger than a destination.
    Application(Box<PrinterApplication>),
}

impl GroupingItem {
    fn identity(&self) -> DeviceIdentity {
        match self {
            Self::Printer(printer) => printer_identity(printer),
            Self::Application(application) => application_identity(application),
        }
    }
}

impl GroupedDevice {
    fn new(item: GroupingItem) -> Self {
        let identity = item.identity();
        match item {
            GroupingItem::Printer(printer) => Self {
                identity,
                application: None,
                queues: vec![printer],
            },
            GroupingItem::Application(application) => Self {
                identity,
                application: Some(*application),
                queues: Vec::new(),
            },
        }
    }

    /// Folds another group into this one.
    ///
    /// A group holds at most one Printer Application, and the one kept is the first
    /// in input order — which the server supplies sorted by `(service_name, id)`.
    /// Two distinct applications cannot reach one group by address, so this only
    /// decides between two records of the same application.
    fn absorb(&mut self, other: Self) {
        self.identity.fill_missing_from(other.identity);
        if self.application.is_none() {
            self.application = other.application;
        }
        self.queues.extend(other.queues);
    }
}

struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(item_count: usize) -> Self {
        Self {
            parent: (0..item_count).collect(),
        }
    }

    fn find(&mut self, index: usize) -> usize {
        let parent = self.parent[index];
        if parent == index {
            index
        } else {
            let root = self.find(parent);
            self.parent[index] = root;
            root
        }
    }

    fn union(&mut self, left: usize, right: usize) {
        let left_root = self.find(left);
        let right_root = self.find(right);
        if left_root != right_root {
            self.parent[left_root] = right_root;
        }
    }
}

/// Groups configured queues that appear to belong to the same physical device.
///
/// Evidence is used in two passes, strongest first. Whatever names one service —
/// an identifier, or an exact address — groups first, which is what puts a Printer
/// Application together with the printers it serves and nothing else. Only then may
/// what is left group by host alone, so a bare host can never reach into an
/// application's group and claim a printer another application serves.
pub fn group_printers(
    printers: Vec<PrinterEntry>,
    printer_applications: Vec<PrinterApplication>,
) -> Vec<GroupedDevice> {
    let items = printers
        .into_iter()
        .map(GroupingItem::Printer)
        .chain(
            printer_applications
                .into_iter()
                .map(|application| GroupingItem::Application(Box::new(application))),
        )
        .collect::<Vec<_>>();
    let identities: Vec<DeviceIdentity> = items.iter().map(GroupingItem::identity).collect();
    let item_count = identities.len();
    let mut sets = DisjointSet::new(item_count);

    let mut first_index_by_key = HashMap::<String, usize>::new();
    for (index, identity) in identities.iter().enumerate() {
        for key in identity.service_keys() {
            if let Some(&other) = first_index_by_key.get(&key) {
                sets.union(other, index);
            } else {
                first_index_by_key.insert(key, index);
            }
        }
    }

    // Decided for every item before any of them moves, so that grouping by host
    // cannot chain its way into a group an application had already claimed.
    let claimed = claimed_by_an_application(&items, &mut sets);

    let mut first_unclaimed_by_key = HashMap::<String, usize>::new();
    for (index, identity) in identities.iter().enumerate() {
        if claimed[index] {
            continue;
        }
        let Some(key) = identity.location_key() else {
            continue;
        };
        if let Some(&other) = first_unclaimed_by_key.get(&key) {
            sets.union(other, index);
        } else {
            first_unclaimed_by_key.insert(key, index);
        }
    }

    let mut slot_of_root: HashMap<usize, usize> = HashMap::new();
    let mut devices = Vec::<GroupedDevice>::new();

    for (index, item) in items.into_iter().enumerate() {
        let root = sets.find(index);
        if let Some(&slot) = slot_of_root.get(&root) {
            devices[slot].absorb(GroupedDevice::new(item));
        } else {
            slot_of_root.insert(root, devices.len());
            devices.push(GroupedDevice::new(item));
        }
    }

    for device in &mut devices {
        device
            .queues
            .sort_by(|left, right| left.id().cmp(right.id()));
    }
    devices.sort_by(|left, right| group_sort_key(left).cmp(&group_sort_key(right)));

    devices
}

fn group_sort_key(device: &GroupedDevice) -> (&str, &str) {
    if let Some(application) = &device.application {
        return (&application.service_name, &application.id);
    }

    device
        .queues
        .first()
        .map(|printer| (printer.id(), printer.name()))
        .unwrap_or_default()
}

/// Returns true when two printer entries appear to describe the same physical
/// device or queue.
pub fn printers_match(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    printer_identity(left).matches(&printer_identity(right))
}

fn printer_identity(printer: &PrinterEntry) -> DeviceIdentity {
    DeviceIdentity::new(
        printer.device_uuid().or_else(|| printer.printer_uuid()),
        printer.endpoint(),
        printer.device_uri(),
        printer.printer_uri(),
    )
}

/// Builds the identity of a Printer Application: the address it answers on, and
/// nothing else.
///
/// A UUID cannot serve. The `system-uuid` an application advertises belongs to the
/// PAPPL system on that machine, so every application running there reports the same
/// one, and grouping by it would merge all of them. Its `system_uri` cannot serve
/// either: it restates the host and port while looking like separate evidence.
///
/// A local application is named `localhost`, which [`PrinterEntry::endpoint`] does
/// for its queues as well. Both sides rewriting is what makes them agree on one
/// spelling of this machine, so it is not a tidy-up to remove from either.
fn application_identity(application: &PrinterApplication) -> DeviceIdentity {
    let host = if application.is_local() {
        "localhost".to_string()
    } else {
        application.hostname.clone()
    };

    DeviceIdentity::new(None, Some((host, application.port)), None, None)
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
    use crate::PrinterApplicationState;
    use std::collections::BTreeMap;

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
            endpoint.map(|(host, port)| {
                (
                    if host_is_known_local(host) {
                        "localhost".to_string()
                    } else {
                        host.to_string()
                    },
                    port,
                )
            }),
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
                .and_then(|port| port.parse::<u16>().ok()),
        );

        {
            if let Some((host, port)) = endpoint {
                options.insert("endpoint-hostname".to_string(), host);
                options.insert("endpoint-port".to_string(), port.to_string());
            }
            PrinterEntry::new(id, id, false, options)
        }
    }

    fn typed_printer_application(id: &str, host: &str, port: u16) -> PrinterApplication {
        PrinterApplication {
            id: id.to_string(),
            service_name: id.to_string(),
            service_type: "_ipps-system._tcp".to_string(),
            domain: "local".to_string(),
            hostname: host.to_string(),
            port,
            addresses: vec![host.to_string()],
            system_uri: format!("ipps://{host}:{port}/ipp/system"),
            make_and_model: None,
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: crate::PrinterApplicationCapabilities::from_operations(vec![0x402b]),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Ready,
        }
    }

    fn printer_queue(id: &str, host: &str, port: u16) -> PrinterEntry {
        let mut printer = printer(
            id,
            &format!("ipp://{host}:{port}/ipp/print/{id}"),
            "",
            Some(&format!("{host}:{port}")),
        );
        printer.set_option("endpoint-address", host);
        printer.set_option("endpoint-hostname", host);
        printer.set_option("endpoint-port", port.to_string());
        printer
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
        let groups = group_printers(printers, Vec::new());
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].queues().len(), 2);
    }

    #[test]
    fn keeps_independent_local_printer_applications_separate() {
        let printers = vec![
            printer("app-a-print", "ipp://localhost:60001/ipp/print", "", None),
            printer("app-b-print", "ipp://localhost:60002/ipp/print", "", None),
        ];
        let groups = group_printers(printers, Vec::new());
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn sorts_groups_by_application_name_or_queue_id() {
        let groups = group_printers(
            vec![
                printer("z-queue", "ipp://192.0.2.2/ipp/print", "", None),
                printer("a-queue", "ipp://192.0.2.1/ipp/print", "", None),
            ],
            Vec::new(),
        );

        assert_eq!(groups[0].queues()[0].id(), "a-queue");
        assert_eq!(groups[1].queues()[0].id(), "z-queue");
    }

    #[test]
    fn moves_printer_application_into_group_metadata() {
        let groups = group_printers(
            vec![printer_queue("SocketLabel", "10.255.255.254", 8000)],
            vec![typed_printer_application("LPrint", "10.255.255.254", 8000)],
        );

        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0]
                .printer_application()
                .map(|printer| printer.id.as_str()),
            Some("LPrint")
        );
        assert_eq!(groups[0].queues().len(), 1);
        assert_eq!(groups[0].queues()[0].id(), "SocketLabel");
    }

    #[test]
    fn returns_app_only_group_without_queue() {
        let groups = group_printers(
            Vec::new(),
            vec![typed_printer_application("LPrint", "10.255.255.254", 8000)],
        );

        assert_eq!(groups.len(), 1);
        assert!(groups[0].printer_application().is_some());
        assert!(groups[0].queues().is_empty());
    }

    #[test]
    fn keeps_printer_applications_on_different_ports_separate() {
        let groups = group_printers(
            Vec::new(),
            vec![
                typed_printer_application("LPrint", "localhost", 8000),
                typed_printer_application("PostScript Printer Application", "localhost", 8001),
            ],
        );

        assert_eq!(groups.len(), 2);
        assert!(
            groups
                .iter()
                .all(|group| group.printer_application().is_some())
        );
        assert!(groups.iter().all(|group| group.queues().is_empty()));
    }

    /// Returns the group holding this application, and the ids of its queues.
    fn group_of(groups: &[GroupedDevice], application_id: &str) -> Vec<String> {
        groups
            .iter()
            .find(|group| {
                group
                    .printer_application()
                    .is_some_and(|application| application.id == application_id)
            })
            .map(|group| {
                group
                    .queues()
                    .iter()
                    .map(|queue| queue.id().to_string())
                    .collect()
            })
            .unwrap_or_else(|| panic!("no group holds '{application_id}'"))
    }

    /// The reported bug. Several Printer Applications on one remote host, each
    /// serving its own printers on its own port: keying by host alone collapsed all
    /// of them into one group, which then kept whichever application came first and
    /// showed every printer under it.
    #[test]
    fn keeps_remote_printer_applications_on_one_host_separate() {
        let groups = group_printers(
            vec![
                printer_queue("label-queue", "printer.lan", 8000),
                printer_queue("postscript-queue", "printer.lan", 8001),
            ],
            vec![
                typed_printer_application("LPrint", "printer.lan", 8000),
                typed_printer_application("PostScript Printer Application", "printer.lan", 8001),
            ],
        );

        assert_eq!(groups.len(), 2);
        assert_eq!(group_of(&groups, "LPrint"), ["label-queue"]);
        assert_eq!(
            group_of(&groups, "PostScript Printer Application"),
            ["postscript-queue"]
        );
    }

    /// Destinations on one remote host with no application discovered keep grouping
    /// by that host, which is how a remote CUPS server's queues stay together.
    #[test]
    fn remote_destinations_without_an_application_stay_one_group() {
        let groups = group_printers(
            vec![
                printer_queue("first", "printer.lan", 8881),
                printer_queue("second", "printer.lan", 8882),
                printer_queue("third", "printer.lan", 8883),
            ],
            Vec::new(),
        );

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].queues().len(), 3);
        assert!(groups[0].printer_application().is_none());
    }

    /// An application takes the printers answering on its own port and leaves the
    /// rest of its host alone, so discovering one application does not rearrange
    /// destinations it has nothing to do with.
    #[test]
    fn an_application_claims_only_the_queue_on_its_own_port() {
        let groups = group_printers(
            vec![
                printer_queue("served", "printer.lan", 8000),
                printer_queue("unrelated-a", "printer.lan", 8881),
                printer_queue("unrelated-b", "printer.lan", 8882),
            ],
            vec![typed_printer_application("LPrint", "printer.lan", 8000)],
        );

        assert_eq!(groups.len(), 2);
        assert_eq!(group_of(&groups, "LPrint"), ["served"]);

        let unrelated = groups
            .iter()
            .find(|group| group.printer_application().is_none())
            .expect("the group of destinations no application claimed");
        assert_eq!(
            unrelated
                .queues()
                .iter()
                .map(|queue| queue.id())
                .collect::<Vec<_>>(),
            ["unrelated-a", "unrelated-b"]
        );
    }

    /// One application advertising under both `_ipp-system._tcp` and
    /// `_ipps-system._tcp` arrives as two records with different ids, because the
    /// service type is part of an application's DNS-SD identity. They answer on one
    /// address, so they are one application and must not become two cards.
    #[test]
    fn one_application_advertised_under_two_service_types_is_one_group() {
        let groups = group_printers(
            Vec::new(),
            vec![
                typed_printer_application("LPrint over ipp", "printer.lan", 8000),
                typed_printer_application("LPrint over ipps", "printer.lan", 8000),
            ],
        );

        assert_eq!(groups.len(), 1);
        assert!(groups[0].printer_application().is_some());
    }

    /// The `system-uuid` an application advertises names the PAPPL system on its
    /// machine, so every application there reports the same one. It must not be
    /// treated as identifying one of them.
    #[test]
    fn applications_sharing_a_system_uuid_stay_separate() {
        let system_uuid = "8f3b1c52-0000-4000-8000-000000000001";
        let with_system_uuid = |id: &str, port: u16| {
            let mut application = typed_printer_application(id, "printer.lan", port);
            application
                .txt
                .insert("system-uuid".to_string(), format!("urn:uuid:{system_uuid}"));
            application
        };

        let groups = group_printers(
            Vec::new(),
            vec![
                with_system_uuid("LPrint", 8000),
                with_system_uuid("PSPA", 8001),
            ],
        );

        assert_eq!(groups.len(), 2);
    }

    /// The path that already worked, kept honest: a local application is named
    /// `localhost` on both sides, so it still finds its own queues.
    #[test]
    fn a_local_application_still_claims_its_own_queues() {
        let groups = group_printers(
            vec![printer_queue("local-queue", "localhost", 8000)],
            vec![
                typed_printer_application("LPrint", "localhost", 8000),
                typed_printer_application("PostScript Printer Application", "localhost", 8001),
            ],
        );

        assert_eq!(groups.len(), 2);
        assert_eq!(group_of(&groups, "LPrint"), ["local-queue"]);
        assert!(
            group_of(&groups, "PostScript Printer Application").is_empty(),
            "an application with no queues of its own must not borrow one"
        );
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

        assert_eq!(group_printers(forward.clone(), Vec::new()).len(), 1);
        assert_eq!(group_printers(reversed.clone(), Vec::new()).len(), 1);

        forward.swap(1, 2);
        assert_eq!(group_printers(forward, Vec::new()).len(), 1);
        reversed.swap(0, 2);
        assert_eq!(group_printers(reversed, Vec::new()).len(), 1);
    }
}
