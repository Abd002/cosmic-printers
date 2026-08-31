//! Groups printer candidates using device UUID, serial number, MAC address, DNS-SD service,
//! normalized device URI, network host and port, manufacturer, and model.

use std::collections::BTreeSet;
use std::net::IpAddr;

/// Which fields support a physical-printer group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdentityConfidence {
    /// At least one device UUID, serial number, or MAC address is present.
    Strong,
    /// No UUID, serial number, or MAC address is present, but a host-and-port endpoint or DNS-SD
    /// service is present.
    Medium,
    /// No UUID, serial number, MAC address, host-and-port endpoint, or DNS-SD service is present.
    Weak,
}

/// A network endpoint normalized for comparison.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NormalizedEndpoint {
    host: String,
    port: u16,
}

impl NormalizedEndpoint {
    /// Normalizes a host and port for comparison, lowercasing the host and
    /// unwrapping a bracketed IPv6 literal so equivalent spellings match.
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: normalize_host(host),
            port,
        }
    }

    /// Returns the normalized host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the port.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Fields extracted from one Printer Application's report about a device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhysicalDeviceEvidence {
    /// A UUID belonging to the printer itself.
    pub device_uuid: Option<String>,
    /// The IEEE-1284 serial number, normalized to uppercase.
    pub serial_number: Option<String>,
    /// The IEEE-1284 manufacturer, normalized to lowercase with collapsed whitespace.
    pub manufacturer: Option<String>,
    /// The IEEE-1284 model, normalized to lowercase with collapsed whitespace.
    pub model: Option<String>,
    /// IEEE-1284 command sets. `can_merge` does not compare command sets.
    pub command_sets: BTreeSet<String>,
    /// USB vendor ID. The aggregate stores USB IDs, but `can_merge` does not compare USB IDs.
    pub usb_vendor_id: Option<u16>,
    /// USB product ID. The aggregate stores USB IDs, but `can_merge` does not compare USB IDs.
    pub usb_product_id: Option<u16>,
    /// Lowercase MAC address with `-` and `.` replaced by `:`, accepted only when it has six
    /// colon-separated components. Component length and hexadecimal syntax are not validated.
    pub mac_address: Option<String>,
    /// Lowercase DNS-SD service instance without trailing whitespace.
    pub dns_sd_service: Option<String>,
    /// Lowercase hostname, or canonical text for an IP address.
    pub network_hostname: Option<String>,
    /// Parsed copy of `network_hostname` when it is an IP address; not currently compared.
    pub network_address: Option<IpAddr>,
    /// Explicit or scheme-default network port.
    pub network_port: Option<u16>,
    /// The IEEE-1284 device ID exactly as reported; not used for grouping.
    pub raw_device_id: Option<String>,
    /// Lowercase comparison copy of the device URI; configuration uses the separate original URI.
    pub normalized_device_uri: Option<String>,
}

impl PhysicalDeviceEvidence {
    /// Builds evidence from a parsed device ID.
    pub fn from_device_id(device_id: &crate::DeviceId) -> Self {
        Self {
            serial_number: device_id.serial_number().map(normalize_serial),
            manufacturer: device_id.manufacturer().map(normalize_name),
            model: device_id.model().map(normalize_name),
            command_sets: device_id.command_sets(),
            raw_device_id: Some(device_id.raw().to_string()),
            ..Self::default()
        }
    }

    /// Records a printer-reported UUID, ignoring an empty value.
    pub fn set_device_uuid(&mut self, uuid: &str) {
        self.device_uuid = normalize_uuid(uuid);
    }

    /// Records a MAC address, ignoring an empty value.
    pub fn set_mac_address(&mut self, mac: &str) {
        self.mac_address = normalize_mac(mac);
    }

    /// Records the DNS-SD service instance the device was advertised as.
    pub fn set_dns_sd_service(&mut self, service: &str) {
        self.dns_sd_service = normalize_optional(service);
    }

    /// Records the network endpoint the device answers on.
    pub fn set_network_endpoint(&mut self, host: &str, port: Option<u16>) {
        self.network_hostname = normalize_optional(host).map(|host| normalize_host(&host));
        self.network_address = self
            .network_hostname
            .as_deref()
            .and_then(|host| host.parse().ok());
        self.network_port = port;
    }

    /// Records a comparison-only normalized form of the device URI.
    pub fn set_normalized_device_uri(&mut self, uri: &str) {
        self.normalized_device_uri = normalize_optional(uri).map(|uri| uri.to_ascii_lowercase());
    }

    /// Returns `(normalized hostname, port)` when both fields are present.
    fn endpoint(&self) -> Option<NormalizedEndpoint> {
        let host = self.network_hostname.as_deref()?;

        Some(NormalizedEndpoint::new(host, self.network_port?))
    }
}

/// All grouping fields collected from every candidate already placed in one row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhysicalIdentityAggregate {
    uuids: BTreeSet<String>,
    serials: BTreeSet<String>,
    mac_addresses: BTreeSet<String>,
    dns_sd_services: BTreeSet<String>,
    device_uris: BTreeSet<String>,
    network_endpoints: BTreeSet<NormalizedEndpoint>,
    hostnames: BTreeSet<String>,
    manufacturers: BTreeSet<String>,
    models: BTreeSet<String>,
    usb_ids: BTreeSet<(u16, u16)>,
}

impl PhysicalIdentityAggregate {
    /// Copies one candidate's grouping fields into sets.
    pub fn from_evidence(evidence: &PhysicalDeviceEvidence) -> Self {
        let mut aggregate = Self::default();

        aggregate.uuids.extend(evidence.device_uuid.clone());
        aggregate.serials.extend(evidence.serial_number.clone());
        aggregate.mac_addresses.extend(evidence.mac_address.clone());
        aggregate
            .dns_sd_services
            .extend(evidence.dns_sd_service.clone());
        aggregate
            .device_uris
            .extend(evidence.normalized_device_uri.clone());
        aggregate.network_endpoints.extend(evidence.endpoint());
        aggregate
            .hostnames
            .extend(evidence.network_hostname.clone());
        aggregate
            .manufacturers
            .extend(evidence.manufacturer.clone());
        aggregate.models.extend(evidence.model.clone());
        if let (Some(vendor), Some(product)) = (evidence.usb_vendor_id, evidence.usb_product_id) {
            aggregate.usb_ids.insert((vendor, product));
        }

        aggregate
    }

    /// Adds every value from another aggregate to the corresponding sets.
    pub fn absorb(&mut self, other: &Self) {
        self.uuids.extend(other.uuids.iter().cloned());
        self.serials.extend(other.serials.iter().cloned());
        self.mac_addresses
            .extend(other.mac_addresses.iter().cloned());
        self.dns_sd_services
            .extend(other.dns_sd_services.iter().cloned());
        self.device_uris.extend(other.device_uris.iter().cloned());
        self.network_endpoints
            .extend(other.network_endpoints.iter().cloned());
        self.hostnames.extend(other.hostnames.iter().cloned());
        self.manufacturers
            .extend(other.manufacturers.iter().cloned());
        self.models.extend(other.models.iter().cloned());
        self.usb_ids.extend(other.usb_ids.iter().copied());
    }

    /// Rejects `other` when `self` and `other` have non-empty UUID, serial-number, or MAC-address
    /// sets with an empty intersection. Otherwise, any shared UUID, serial number, MAC address,
    /// DNS-SD service, or normalized device URI accepts `other`. With none of those matches,
    /// manufacturer and model sets must each be empty on one aggregate or contain an equal or
    /// substring-related pair. The final requirement is equal host and port, or equal host plus
    /// non-empty model sets containing an equal or substring-related pair.
    pub fn can_merge(&self, other: &Self) -> bool {
        if self.conflicts_with(other) {
            return false;
        }

        if self.shares_uuid_serial_mac_service_or_uri(other) {
            return true;
        }

        if !self.model_is_compatible(other) {
            return false;
        }

        // A shared host needs compatible models unless the ports also match.
        intersects(&self.network_endpoints, &other.network_endpoints)
            || (self.shares_hostname(other) && self.models_agree(other))
    }

    /// Returns `true` when the `self` and `other` UUID, serial-number, or MAC-address sets are both
    /// non-empty and have an empty intersection. DNS-SD services and device URIs cannot conflict.
    fn conflicts_with(&self, other: &Self) -> bool {
        disjoint_and_present(&self.uuids, &other.uuids)
            || disjoint_and_present(&self.serials, &other.serials)
            || disjoint_and_present(&self.mac_addresses, &other.mac_addresses)
    }

    /// Returns `true` when UUID, serial number, MAC address, DNS-SD service, or normalized device
    /// URI intersects and no non-empty UUID, serial-number, or MAC-address sets are disjoint.
    pub fn agrees_strongly_with(&self, other: &Self) -> bool {
        !self.conflicts_with(other) && self.shares_uuid_serial_mac_service_or_uri(other)
    }

    fn shares_uuid_serial_mac_service_or_uri(&self, other: &Self) -> bool {
        intersects(&self.uuids, &other.uuids)
            || intersects(&self.serials, &other.serials)
            || intersects(&self.mac_addresses, &other.mac_addresses)
            || intersects(&self.dns_sd_services, &other.dns_sd_services)
            || intersects(&self.device_uris, &other.device_uris)
    }

    /// Returns `true` when the normalized-hostname sets share a value; ports are ignored.
    fn shares_hostname(&self, other: &Self) -> bool {
        intersects(&self.hostnames, &other.hostnames)
    }

    /// Returns `true` when both model sets are non-empty and both model and manufacturer sets are
    /// empty on one aggregate or contain an equal or substring-related pair.
    fn models_agree(&self, other: &Self) -> bool {
        !self.models.is_empty() && !other.models.is_empty() && self.model_is_compatible(other)
    }

    /// Returns `true` when the model sets and manufacturer sets are each empty on one aggregate or
    /// contain a pair where the strings are equal or one string contains the other.
    fn model_is_compatible(&self, other: &Self) -> bool {
        names_compatible(&self.models, &other.models)
            && names_compatible(&self.manufacturers, &other.manufacturers)
    }

    /// Returns `Strong` for any UUID, serial number, or MAC address; otherwise `Medium` for any
    /// host-and-port endpoint or DNS-SD service; otherwise `Weak`.
    pub fn confidence(&self) -> IdentityConfidence {
        if !self.uuids.is_empty() || !self.serials.is_empty() || !self.mac_addresses.is_empty() {
            IdentityConfidence::Strong
        } else if !self.network_endpoints.is_empty() || !self.dns_sd_services.is_empty() {
            IdentityConfidence::Medium
        } else {
            IdentityConfidence::Weak
        }
    }

    /// Returns the first available value in this order: UUID, serial number, MAC address, DNS-SD
    /// service, host-and-port endpoint, then normalized device URI.
    pub fn stable_key(&self) -> Option<String> {
        if let Some(uuid) = self.uuids.first() {
            return Some(format!("uuid:{uuid}"));
        }
        if let Some(serial) = self.serials.first() {
            return Some(format!("serial:{serial}"));
        }
        if let Some(mac) = self.mac_addresses.first() {
            return Some(format!("mac:{mac}"));
        }
        if let Some(service) = self.dns_sd_services.first() {
            return Some(format!("service:{service}"));
        }
        if let Some(endpoint) = self.network_endpoints.first() {
            return Some(format!("endpoint:{}:{}", endpoint.host, endpoint.port));
        }
        self.device_uris.first().map(|uri| format!("uri:{uri}"))
    }
}

fn intersects<T: Ord>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> bool {
    left.intersection(right).next().is_some()
}

fn disjoint_and_present<T: Ord>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> bool {
    !left.is_empty() && !right.is_empty() && !intersects(left, right)
}

fn names_compatible(left: &BTreeSet<String>, right: &BTreeSet<String>) -> bool {
    if left.is_empty() || right.is_empty() {
        return true;
    }

    left.iter().any(|left| {
        right
            .iter()
            .any(|right| left == right || left.contains(right.as_str()) || right.contains(left))
    })
}

fn normalize_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();

    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.');
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);

    // Reformatting a parsed address collapses equivalent IPv6 spellings.
    match bare.parse::<IpAddr>() {
        Ok(address) => address.to_string(),
        Err(_) => bare.to_ascii_lowercase(),
    }
}

fn normalize_uuid(uuid: &str) -> Option<String> {
    let lowered = normalize_optional(uuid)?.to_ascii_lowercase();

    Some(
        lowered
            .strip_prefix("urn:uuid:")
            .unwrap_or(&lowered)
            .to_string(),
    )
}

pub(super) fn normalize_serial(serial: &str) -> String {
    serial.trim().to_ascii_uppercase()
}

fn normalize_mac(mac: &str) -> Option<String> {
    let normalized = normalize_optional(mac)?
        .to_ascii_lowercase()
        .replace(['-', '.'], ":");

    // Require six components so an unseparated or structurally incomplete value cannot match.
    // Component syntax stays permissive because devices report several nonstandard forms.
    (normalized.split(':').count() == 6).then_some(normalized)
}

pub(super) fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_serial(serial: &str) -> PhysicalDeviceEvidence {
        PhysicalDeviceEvidence {
            serial_number: Some(normalize_serial(serial)),
            ..PhysicalDeviceEvidence::default()
        }
    }

    fn with_endpoint(host: &str, port: u16) -> PhysicalDeviceEvidence {
        let mut evidence = PhysicalDeviceEvidence::default();
        evidence.set_network_endpoint(host, Some(port));
        evidence
    }

    fn with_model(model: &str) -> PhysicalDeviceEvidence {
        PhysicalDeviceEvidence {
            model: Some(normalize_name(model)),
            ..PhysicalDeviceEvidence::default()
        }
    }

    fn aggregate(evidence: &PhysicalDeviceEvidence) -> PhysicalIdentityAggregate {
        PhysicalIdentityAggregate::from_evidence(evidence)
    }

    #[test]
    fn shared_serial_merges() {
        let left = aggregate(&with_serial("abc123"));
        let right = aggregate(&with_serial("ABC123"));

        assert!(left.can_merge(&right));
        assert!(right.can_merge(&left));
    }

    #[test]
    fn conflicting_serial_blocks_a_merge_even_with_a_shared_endpoint() {
        let mut left = with_serial("1234");
        left.set_network_endpoint("192.0.2.50", Some(9100));
        let mut right = with_serial("9999");
        right.set_network_endpoint("192.0.2.50", Some(9100));

        assert!(!aggregate(&left).can_merge(&aggregate(&right)));
    }

    #[test]
    fn shared_endpoint_merges_only_with_a_compatible_model() {
        let mut left = with_endpoint("192.0.2.50", 9100);
        left.model = Some(normalize_name("Test Laser 9000"));
        let mut compatible = with_endpoint("192.0.2.50", 9100);
        compatible.model = Some(normalize_name("Acme Test Laser 9000"));
        let mut conflicting = with_endpoint("192.0.2.50", 9100);
        conflicting.model = Some(normalize_name("Other Label Printer"));

        assert!(aggregate(&left).can_merge(&aggregate(&compatible)));
        assert!(!aggregate(&left).can_merge(&aggregate(&conflicting)));
    }

    #[test]
    fn model_alone_never_merges() {
        let left = aggregate(&with_model("Test Laser 9000"));
        let right = aggregate(&with_model("Test Laser 9000"));

        assert!(!left.can_merge(&right));
    }

    #[test]
    fn ipv6_spellings_compare_equal() {
        let left = with_endpoint("[2001:db8::1]", 631);
        let right = with_endpoint("2001:0db8:0000:0000:0000:0000:0000:0001", 631);

        assert!(aggregate(&left).can_merge(&aggregate(&right)));
    }

    #[test]
    fn differing_service_names_are_not_a_conflict() {
        let mut left = with_serial("1234");
        left.set_dns_sd_service("Printer A._pdl-datastream._tcp.local");
        let mut right = with_serial("1234");
        right.set_dns_sd_service("Printer B._pdl-datastream._tcp.local");

        assert!(aggregate(&left).can_merge(&aggregate(&right)));
    }

    #[test]
    fn shared_device_uri_merges_across_applications() {
        let mut left = PhysicalDeviceEvidence::default();
        left.set_normalized_device_uri("socket://192.0.2.50:9100");
        let mut right = PhysicalDeviceEvidence::default();
        right.set_normalized_device_uri("SOCKET://192.0.2.50:9100");

        assert!(aggregate(&left).can_merge(&aggregate(&right)));
    }

    #[test]
    fn stable_key_prefers_uuid_then_serial_then_mac() {
        let mut evidence = with_serial("1234");
        evidence.set_network_endpoint("192.0.2.50", Some(9100));
        let mut identity = aggregate(&evidence);

        assert_eq!(identity.stable_key().as_deref(), Some("serial:1234"));

        let mut later = PhysicalDeviceEvidence::default();
        later.set_network_endpoint("192.0.2.51", Some(9100));
        identity.absorb(&aggregate(&later));

        assert_eq!(identity.stable_key().as_deref(), Some("serial:1234"));
    }

    #[test]
    fn confidence_reflects_the_evidence_available() {
        assert_eq!(
            aggregate(&with_serial("1234")).confidence(),
            IdentityConfidence::Strong
        );
        assert_eq!(
            aggregate(&with_endpoint("192.0.2.50", 9100)).confidence(),
            IdentityConfidence::Medium
        );
        assert_eq!(
            aggregate(&with_model("Test Laser")).confidence(),
            IdentityConfidence::Weak
        );
    }

    #[test]
    fn unusable_mac_addresses_are_not_treated_as_identity() {
        let mut evidence = PhysicalDeviceEvidence::default();
        evidence.set_mac_address("not-a-mac");
        assert_eq!(evidence.mac_address, None);

        evidence.set_mac_address("02-00-00-00-00-01");
        assert_eq!(evidence.mac_address.as_deref(), Some("02:00:00:00:00:01"));
    }

    #[test]
    fn device_uuid_ignores_the_urn_prefix_and_case() {
        let mut left = PhysicalDeviceEvidence::default();
        left.set_device_uuid("urn:uuid:11111111-2222-3333-4444-555555555555");
        let mut right = PhysicalDeviceEvidence::default();
        right.set_device_uuid("11111111-2222-3333-4444-555555555555");

        assert_eq!(left.device_uuid, right.device_uuid);
        assert!(aggregate(&left).can_merge(&aggregate(&right)));
    }

    #[test]
    fn evidence_from_a_device_id_carries_serial_and_model() {
        let device_id = crate::DeviceId::parse("MFG:Acme;MDL:Test Laser 9000;SN:abc123;CMD:PCL;");
        let evidence = PhysicalDeviceEvidence::from_device_id(&device_id);

        assert_eq!(evidence.serial_number.as_deref(), Some("ABC123"));
        assert_eq!(evidence.model.as_deref(), Some("test laser 9000"));
        assert_eq!(evidence.manufacturer.as_deref(), Some("acme"));
        assert_eq!(evidence.command_sets, BTreeSet::from(["PCL".to_string()]));
        assert_eq!(
            evidence.raw_device_id.as_deref(),
            Some("MFG:Acme;MDL:Test Laser 9000;SN:abc123;CMD:PCL;")
        );
    }
}
