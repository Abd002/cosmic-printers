//! Physical printer identity: evidence, conflict-aware merging, and grouping.
//!
//! Add Printer shows one row per physical printer, built from observations that
//! several Printer Applications made independently. Deciding that two
//! observations describe the same hardware is the whole problem, and getting it
//! wrong in the merging direction is worse than getting it wrong in the
//! separating direction: a false merge hides a printer the user owns behind
//! another printer's name, while a false separation merely shows two rows.
//!
//! Evidence is therefore split into three strengths:
//!
//! - **Strong** — an intrinsic identifier of one piece of hardware: a device
//!   UUID, a serial number, or a MAC address. Sharing one is proof of identity;
//!   disagreeing on one is proof of difference.
//! - **Medium** — where the device answers: a network endpoint or hostname.
//!   Sharing one is suggestive, so it merges only alongside a compatible model.
//! - **Weak** — a name, model, command set, or description. Never merges on its
//!   own, because two identical printers on one desk look identical this way.
//!
//! Addresses that identify a *service* rather than the hardware — a DNS-SD
//! service name, or a Printer-Application-specific device URI — count as strong
//! when shared but never as a conflict when they differ, because two Printer
//! Applications reach the same printer by different routes.

use std::collections::BTreeSet;
use std::net::IpAddr;

/// How confidently a physical printer's identity was established.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdentityConfidence {
    /// Backed by an intrinsic identifier such as a serial number or UUID.
    Strong,
    /// Backed by where the device answers, plus a compatible model.
    Medium,
    /// Backed only by descriptive data; the group holds a single observation.
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

/// Identity evidence gathered about one physical printer.
///
/// Values are stored normalized for comparison, not for display: a group's
/// human-readable name comes from the observation that produced it, never from
/// here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhysicalDeviceEvidence {
    /// A UUID belonging to the printer itself, never a Printer Application's
    /// system UUID.
    pub device_uuid: Option<String>,
    pub serial_number: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub command_sets: BTreeSet<String>,
    pub usb_vendor_id: Option<u16>,
    pub usb_product_id: Option<u16>,
    pub mac_address: Option<String>,
    pub dns_sd_service: Option<String>,
    pub network_hostname: Option<String>,
    pub network_address: Option<IpAddr>,
    pub network_port: Option<u16>,
    /// The device ID exactly as reported, kept for diagnostics.
    pub raw_device_id: Option<String>,
    /// A device URI normalized for comparison only. The exact URI a Printer
    /// Application reported is never stored here, because it must be replayed
    /// unchanged and only to the Printer Application that produced it.
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

    /// Returns the endpoint used for medium-strength comparison.
    fn endpoint(&self) -> Option<NormalizedEndpoint> {
        let host = self.network_hostname.as_deref()?;
        Some(NormalizedEndpoint::new(
            host,
            self.network_port.unwrap_or(0),
        ))
    }
}

/// The accumulated identity of a group of observations.
///
/// A group can hold several values for the same kind of evidence, because
/// different Printer Applications reach a printer differently. Merging is
/// decided against the whole accumulation, not against one member, so a device
/// that conflicts with any member is kept out.
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
    /// Builds an aggregate holding a single observation's evidence.
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

    /// Folds another aggregate into this one.
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

    /// Decides whether two aggregates describe the same physical printer.
    ///
    /// A strong conflict rejects the merge outright, even when other evidence
    /// agrees. Otherwise shared strong evidence accepts it, and failing that the
    /// merge needs both a shared location and a compatible model.
    pub fn can_merge(&self, other: &Self) -> bool {
        if self.conflicts_with(other) {
            return false;
        }

        if self.shares_strong_identity(other) {
            return true;
        }

        self.shares_medium_identity(other) && self.model_is_compatible(other)
    }

    /// Returns true when an intrinsic identifier disagrees.
    ///
    /// Only identifiers intrinsic to the hardware count. A DNS-SD service name
    /// or device URI that differs proves nothing, because that is just another
    /// route to the same printer.
    ///
    /// A printer with both wired and wireless interfaces reports two MAC
    /// addresses and will be treated as two devices. That is the safe direction
    /// of the trade: two rows for one printer, rather than one row hiding two.
    fn conflicts_with(&self, other: &Self) -> bool {
        disjoint_and_present(&self.uuids, &other.uuids)
            || disjoint_and_present(&self.serials, &other.serials)
            || disjoint_and_present(&self.mac_addresses, &other.mac_addresses)
    }

    /// Returns true when an identifier unique to one endpoint or one piece of
    /// hardware agrees, and none disagrees.
    ///
    /// Stronger than [`Self::can_merge`], which also accepts a shared location
    /// plus a compatible model. Two identical printers side by side agree that
    /// way, so a caller that must not confuse them asks for this instead.
    pub fn agrees_strongly_with(&self, other: &Self) -> bool {
        !self.conflicts_with(other) && self.shares_strong_identity(other)
    }

    fn shares_strong_identity(&self, other: &Self) -> bool {
        intersects(&self.uuids, &other.uuids)
            || intersects(&self.serials, &other.serials)
            || intersects(&self.mac_addresses, &other.mac_addresses)
            || intersects(&self.dns_sd_services, &other.dns_sd_services)
            || intersects(&self.device_uris, &other.device_uris)
    }

    fn shares_medium_identity(&self, other: &Self) -> bool {
        intersects(&self.network_endpoints, &other.network_endpoints)
            || intersects(&self.hostnames, &other.hostnames)
    }

    /// Returns true when the make and model do not contradict each other.
    ///
    /// Absent data is compatible with anything, since many Printer Applications
    /// report no model at all for a raw socket device. Present data matches
    /// exactly or by containment, so `test laser 9000` and
    /// `acme test laser 9000` agree.
    fn model_is_compatible(&self, other: &Self) -> bool {
        names_compatible(&self.models, &other.models)
            && names_compatible(&self.manufacturers, &other.manufacturers)
    }

    /// Returns how strongly this group's identity is established.
    pub fn confidence(&self) -> IdentityConfidence {
        if !self.uuids.is_empty() || !self.serials.is_empty() || !self.mac_addresses.is_empty() {
            IdentityConfidence::Strong
        } else if !self.network_endpoints.is_empty() || !self.dns_sd_services.is_empty() {
            IdentityConfidence::Medium
        } else {
            IdentityConfidence::Weak
        }
    }

    /// Returns a stable key identifying this group across recomputations.
    ///
    /// The strongest available evidence wins, so adding a Printer Application's
    /// observation to an existing group does not rename the group.
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

/// An observation that can be grouped by physical device.
pub trait PhysicalDeviceObservation {
    /// Returns the identity evidence for this observation.
    fn physical_evidence(&self) -> &PhysicalDeviceEvidence;

    /// Returns a key that orders observations deterministically.
    ///
    /// Grouping is greedy, so the order observations are considered in decides
    /// the outcome in ambiguous cases. Sorting on this key first makes the
    /// result depend on the set of observations rather than on the order Printer
    /// Applications happened to answer in.
    fn grouping_sort_key(&self) -> String;
}

/// A set of observations judged to describe one physical printer.
#[derive(Clone, Debug)]
pub struct PhysicalDeviceGroup<T> {
    /// The accumulated identity of every member.
    pub identity: PhysicalIdentityAggregate,
    /// The observations in this group, in deterministic order.
    pub members: Vec<T>,
}

/// Groups observations by physical printer.
///
/// The result is deterministic for a given set of observations: input is sorted
/// by [`PhysicalDeviceObservation::grouping_sort_key`] first, so two Printer
/// Applications answering in a different order produce identical groups.
///
/// An observation joins the first group it can merge with. Because a conflict
/// with any accumulated member blocks the merge, this cannot chain two
/// contradicting devices together through a shared address.
pub fn group_by_physical_device<T: PhysicalDeviceObservation>(
    observations: Vec<T>,
) -> Vec<PhysicalDeviceGroup<T>> {
    let mut sorted = observations;
    sorted.sort_by_key(|observation| observation.grouping_sort_key());

    let mut groups: Vec<PhysicalDeviceGroup<T>> = Vec::new();

    for observation in sorted {
        let candidate = PhysicalIdentityAggregate::from_evidence(observation.physical_evidence());
        match groups
            .iter_mut()
            .find(|group| group.identity.can_merge(&candidate))
        {
            Some(group) => {
                group.identity.absorb(&candidate);
                group.members.push(observation);
            }
            None => groups.push(PhysicalDeviceGroup {
                identity: candidate,
                members: vec![observation],
            }),
        }
    }

    groups
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

fn normalize_serial(serial: &str) -> String {
    serial.trim().to_ascii_uppercase()
}

fn normalize_mac(mac: &str) -> Option<String> {
    let normalized = normalize_optional(mac)?
        .to_ascii_lowercase()
        .replace(['-', '.'], ":");

    // A MAC with no separators or an unusable length is not treated as
    // identity evidence at all, rather than compared in a form that could
    // accidentally match.
    (normalized.split(':').count() == 6).then_some(normalized)
}

fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Observation {
        id: &'static str,
        evidence: PhysicalDeviceEvidence,
    }

    impl PhysicalDeviceObservation for Observation {
        fn physical_evidence(&self) -> &PhysicalDeviceEvidence {
            &self.evidence
        }

        fn grouping_sort_key(&self) -> String {
            self.id.to_string()
        }
    }

    fn observation(id: &'static str, evidence: PhysicalDeviceEvidence) -> Observation {
        Observation { id, evidence }
    }

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
    fn transitive_conflict_does_not_over_merge() {
        let mut b = with_serial("1234");
        b.set_network_endpoint("192.0.2.50", Some(9100));
        let mut c = with_serial("9999");
        c.set_network_endpoint("192.0.2.50", Some(9100));

        let groups = group_by_physical_device(vec![
            observation("a", with_serial("1234")),
            observation("b", b),
            observation("c", c),
        ]);

        assert_eq!(groups.len(), 2);
        let mut sizes = groups
            .iter()
            .map(|group| group.members.len())
            .collect::<Vec<_>>();
        sizes.sort_unstable();
        assert_eq!(sizes, vec![1, 2]);
    }

    #[test]
    fn grouping_is_independent_of_observation_order() {
        let build = || {
            let mut b = with_serial("1234");
            b.set_network_endpoint("192.0.2.50", Some(9100));
            let mut c = with_serial("9999");
            c.set_network_endpoint("192.0.2.50", Some(9100));
            vec![
                observation("a", with_serial("1234")),
                observation("b", b),
                observation("c", c),
            ]
        };

        let forward = group_by_physical_device(build());
        let mut reversed_input = build();
        reversed_input.reverse();
        let reversed = group_by_physical_device(reversed_input);

        let keys = |groups: &[PhysicalDeviceGroup<Observation>]| {
            groups
                .iter()
                .map(|group| {
                    let mut ids = group
                        .members
                        .iter()
                        .map(|member| member.id)
                        .collect::<Vec<_>>();
                    ids.sort_unstable();
                    ids
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(keys(&forward), keys(&reversed));
    }

    #[test]
    fn two_identical_models_without_serials_stay_separate() {
        let groups = group_by_physical_device(vec![
            observation("a", with_model("Test Laser 9000")),
            observation("b", with_model("Test Laser 9000")),
        ]);

        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn removing_an_observation_cannot_create_a_new_merge() {
        let mut bridge = with_serial("1234");
        bridge.set_network_endpoint("192.0.2.50", Some(9100));

        let all = || {
            vec![
                observation("a", with_serial("1234")),
                observation("b", bridge.clone()),
                observation("c", with_serial("9999")),
            ]
        };

        let full = group_by_physical_device(all());
        let without_bridge = group_by_physical_device(
            all()
                .into_iter()
                .filter(|observation| observation.id != "b")
                .collect(),
        );

        assert_eq!(full.len(), 2);
        assert_eq!(without_bridge.len(), 2);
    }

    #[test]
    fn stable_key_prefers_the_strongest_evidence() {
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
