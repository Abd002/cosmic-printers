//! `PAPPL-Find-Devices`: asking one Printer Application what it can see.

use cosmic_settings_printers_core::{DeviceId, PhysicalDeviceEvidence};
use cups_rs::{IppCollection, IppOperation, IppTag, IppValueTag};

use super::client::{MAX_COLLECTIONS, OperationCost, PaError, PaRequest, bounded, check_status};

/// How a device is attached, which decides how endpoints are preferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeviceTransport {
    /// Attached directly to this machine over USB.
    Usb,
    /// Found by DNS-SD.
    DnsSd,
    /// A raw TCP socket, usually AppSocket on port 9100.
    Socket,
    /// Found by SNMP.
    Snmp,
    /// Reached over IPP or IPPS.
    Ipp,
    /// A local attachment that is not USB, such as a parallel or serial port.
    OtherLocal,
    /// A scheme specific to one Printer Application's own backend.
    Vendor,
}

impl DeviceTransport {
    /// Classifies a device URI by its scheme.
    pub(crate) fn from_uri(uri: &str) -> Self {
        let scheme = uri
            .split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .unwrap_or_default();

        match scheme.as_str() {
            "usb" => Self::Usb,
            "dnssd" => Self::DnsSd,
            "socket" => Self::Socket,
            "snmp" => Self::Snmp,
            "ipp" | "ipps" => Self::Ipp,
            "file" | "parallel" | "serial" => Self::OtherLocal,
            _ => Self::Vendor,
        }
    }

    /// Returns whether a URI of this transport names a host.
    pub(crate) fn addresses_a_host(self) -> bool {
        !matches!(self, Self::Usb | Self::OtherLocal)
    }

    /// Orders endpoints by directness and required vendor backends.
    pub(crate) fn preference(self) -> i32 {
        match self {
            Self::Usb => 0,
            Self::Ipp => 10,
            Self::Vendor => 20,
            Self::DnsSd => 30,
            Self::Socket => 40,
            Self::Snmp => 50,
            Self::OtherLocal => 60,
        }
    }
}

/// One device, exactly as one Printer Application reported it.
#[derive(Clone, Debug)]
pub(crate) struct PaDeviceObservation {
    /// Identifier scoped to the owning application and discovery generation.
    pub(crate) id: String,
    /// The application that reported this device, and the only one that may be
    /// given [`PaDeviceObservation::device_uri`].
    pub(crate) printer_application_id: String,
    /// The exact URI the application returned. Never rewritten, never given to
    /// another application.
    pub(crate) device_uri: String,
    /// The device ID, when the application reported a usable one.
    pub(crate) device_id: Option<DeviceId>,
    /// What to show the user for this device, preferring the description the
    /// application itself reported.
    pub(crate) display_name: String,
    pub(crate) identity: PhysicalDeviceEvidence,
    pub(crate) transport: DeviceTransport,
}

impl PaDeviceObservation {
    /// Returns the device ID string to send back when matching drivers.
    pub(crate) fn raw_device_id(&self) -> Option<&str> {
        self.device_id.as_ref().map(DeviceId::raw)
    }
}

/// Asks for every transport at once, which is what a healthy application answers
/// fastest: it scans once instead of once per transport.
const EVERY_DEVICE_TYPE: &[&str] = &["all"];

/// Individual fallback scans prevent one broken transport from hiding all results.
const EACH_DEVICE_TYPE: &[&str] = &["dns-sd", "usb", "snmp", "other-local", "other-network"];

/// Returns usable observations and counts malformed collections separately.
pub(crate) fn find_devices(
    application_id: &str,
    system_uri: &str,
    generation: u64,
) -> Result<FindDevicesResult, PaError> {
    let combined = scan_devices(application_id, system_uri, generation, EVERY_DEVICE_TYPE);
    let Err(error) = combined else {
        return combined;
    };

    tracing::debug!(
        application_id,
        ?error,
        "a combined device scan failed, asking for each transport separately"
    );

    let mut merged = FindDevicesResult {
        observations: Vec::new(),
        quarantined: 0,
    };
    let mut answered = false;

    for device_type in EACH_DEVICE_TYPE {
        match scan_devices(
            application_id,
            system_uri,
            generation,
            std::slice::from_ref(device_type),
        ) {
            Ok(result) => {
                answered = true;
                merged.quarantined += result.quarantined;
                for mut observation in result.observations {
                    if merged
                        .observations
                        .iter()
                        .any(|seen| seen.device_uri == observation.device_uri)
                    {
                        continue;
                    }
                    // Identifiers count within a scan, so they are renumbered
                    // across the merged one to stay unique.
                    observation.id =
                        observation_id(application_id, generation, merged.observations.len());
                    merged.observations.push(observation);
                }
            }
            Err(error) => {
                tracing::debug!(
                    application_id,
                    device_type,
                    ?error,
                    "a printer application could not scan one transport"
                );
            }
        }
    }

    if answered { Ok(merged) } else { Err(error) }
}

fn scan_devices(
    application_id: &str,
    system_uri: &str,
    generation: u64,
    device_types: &[&str],
) -> Result<FindDevicesResult, PaError> {
    let response = PaRequest::new(IppOperation::PAPPL_FIND_DEVICES, system_uri)?
        .keywords("smi55357-device-type", device_types)?
        .send_allowing_failure(system_uri, OperationCost::DeviceScan)?;

    // PAPPL `not-found` is a successful empty scan.
    if response.status() == cups_rs::IppStatus::ErrorNotFound {
        return Ok(FindDevicesResult {
            observations: Vec::new(),
            quarantined: 0,
        });
    }
    check_status(&response)?;

    let mut observations = Vec::new();
    let mut quarantined = 0usize;
    let mut seen_uris = Vec::new();

    // Read every repeated device collection and deduplicate PAPPL's occasional repeats.
    for attribute in response.attributes_named("smi55357-device-col") {
        if attribute.group_tag() != Some(IppTag::System) {
            quarantined += 1;
            continue;
        }
        if attribute.value_tag() != IppValueTag::BeginCollection {
            quarantined += 1;
            continue;
        }

        for collection in attribute.collections().into_iter().take(MAX_COLLECTIONS) {
            match observation(application_id, generation, &collection, observations.len()) {
                Some(observation) => {
                    if seen_uris.contains(&observation.device_uri) {
                        continue;
                    }
                    seen_uris.push(observation.device_uri.clone());
                    observations.push(observation);
                }
                None => quarantined += 1,
            }

            if observations.len() >= MAX_COLLECTIONS {
                break;
            }
        }
    }

    Ok(FindDevicesResult {
        observations,
        quarantined,
    })
}

/// What one device scan produced.
pub(crate) struct FindDevicesResult {
    pub(crate) observations: Vec<PaDeviceObservation>,
    /// Number of malformed device collections.
    pub(crate) quarantined: usize,
}

/// Builds an observation, rejecting collections without a usable device URI.
fn observation(
    application_id: &str,
    generation: u64,
    collection: &IppCollection<'_>,
    index: usize,
) -> Option<PaDeviceObservation> {
    let device_uri = collection.text("smi55357-device-uri").map(bounded)?;
    if device_uri.is_empty() || !device_uri.contains(':') {
        return None;
    }

    let device_id = collection
        .text("smi55357-device-id")
        .map(bounded)
        .map(|raw| DeviceId::parse(&raw))
        .filter(|device_id| !device_id.is_empty());
    let device_info = collection.text("smi55357-device-info").map(bounded);
    let transport = DeviceTransport::from_uri(&device_uri);
    let identity = evidence(device_id.as_ref(), &device_uri);
    let display_name = display_name(device_id.as_ref(), device_info.as_deref(), &device_uri);

    Some(PaDeviceObservation {
        id: observation_id(application_id, generation, index),
        printer_application_id: application_id.to_string(),
        device_uri,
        device_id,
        display_name,
        identity,
        transport,
    })
}

/// Names an observation within one application's round.
fn observation_id(application_id: &str, generation: u64, index: usize) -> String {
    format!("{application_id}:{generation}:{index}")
}

/// Extracts grouping evidence, preferring an IEEE-1284 serial over a URI serial.
pub(super) fn evidence(device_id: Option<&DeviceId>, device_uri: &str) -> PhysicalDeviceEvidence {
    let mut evidence = device_id
        .map(PhysicalDeviceEvidence::from_device_id)
        .unwrap_or_default();
    evidence.set_normalized_device_uri(&normalized_device_uri(device_uri));

    if let Some(service) = dns_sd_service(device_uri) {
        evidence.set_dns_sd_service(&service);
    }
    if DeviceTransport::from_uri(device_uri).addresses_a_host()
        && let Some((host, port)) = uri_endpoint(device_uri)
    {
        evidence.set_network_endpoint(&host, port);
    }
    if let Some(serial) = uri_query_value(device_uri, "serial")
        && evidence.serial_number.is_none()
    {
        evidence.serial_number = Some(serial.trim().to_ascii_uppercase());
    }
    if let Some(uuid) = uri_query_value(device_uri, "uuid") {
        evidence.set_device_uuid(&uuid);
    }

    evidence
}

/// Removes the fragment, lowercases scheme and authority, removes an explicit default port, and
/// preserves the path. The original URI remains unchanged for configuring the printer.
fn normalized_device_uri(uri: &str) -> String {
    let without_fragment = uri.split('#').next().unwrap_or(uri);
    let Some((scheme, rest)) = without_fragment.split_once("://") else {
        return without_fragment.to_ascii_lowercase();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = authority.to_ascii_lowercase();
    let authority = match authority.rsplit_once(':') {
        Some((host, port))
            if Some(port) == default_port(&scheme).map(|it| it.to_string()).as_deref() =>
        {
            host.to_string()
        }
        _ => authority,
    };

    format!("{scheme}://{authority}/{path}")
}

/// Returns the port a scheme is understood to mean when a URI omits one.
fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "ipp" | "ipps" => Some(631),
        "socket" => Some(9100),
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

/// Extracts the authority host and an explicit or scheme-default port. This makes
/// `socket://host` and `socket://host:9100` produce the same host-and-port value.
fn uri_endpoint(uri: &str) -> Option<(String, Option<u16>)> {
    let (scheme, rest) = uri.split_once("://")?;
    let implied = default_port(&scheme.to_ascii_lowercase());
    let authority = rest.split(['/', '?']).next()?;
    let authority = authority.rsplit('@').next()?;
    if authority.is_empty() {
        return None;
    }

    if let Some(end) = authority.find(']') {
        let host = authority.get(..=end)?;
        let port = authority
            .get(end + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .and_then(|port| port.parse().ok());
        return Some((host.to_string(), port.or(implied)));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => Some((host.to_string(), Some(port))),
            // A colon that is not a port belongs to the host, such as a USB
            // device name containing one.
            Err(_) => Some((authority.to_string(), implied)),
        },
        None => Some((authority.to_string(), implied)),
    }
}

/// Extracts a `dnssd://` service instance, decoding percent and DNS-SD decimal escapes, removing a
/// trailing dot, trimming whitespace, and lowercasing the result.
fn dns_sd_service(uri: &str) -> Option<String> {
    let uri = uri.strip_prefix("cups:").unwrap_or(uri);
    if !uri
        .get(..DNS_SD_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(DNS_SD_PREFIX))
    {
        return None;
    }

    let instance = uri[DNS_SD_PREFIX.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let instance = percent_encoding::percent_decode_str(instance).decode_utf8_lossy();
    let instance = decode_dns_sd_escapes(&instance);
    let instance = instance.trim().trim_end_matches('.').to_ascii_lowercase();

    (!instance.is_empty()).then_some(instance)
}

const DNS_SD_PREFIX: &str = "dnssd://";

/// Undoes the `\DDD` escaping DNS-SD uses for characters a label cannot hold.
fn decode_dns_sd_escapes(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'\\' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }

        match bytes.get(index + 1..index + 4) {
            Some(digits) if digits.iter().all(u8::is_ascii_digit) => {
                let code = digits
                    .iter()
                    .fold(0u32, |code, digit| code * 10 + u32::from(digit - b'0'));
                match u8::try_from(code) {
                    Ok(byte) => decoded.push(byte),
                    // Not an escape this scheme can produce, so it is left as
                    // written rather than turned into something else.
                    Err(_) => decoded.extend_from_slice(&bytes[index..index + 4]),
                }
                index += 4;
            }
            _ => {
                decoded.extend(bytes.get(index + 1).copied());
                index += 2;
            }
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

/// Returns the first non-empty query value whose name matches `key` without ASCII case sensitivity.
fn uri_query_value(uri: &str, key: &str) -> Option<String> {
    uri.split_once('?')?
        .1
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.to_string())
        .filter(|value| !value.is_empty())
}

/// Chooses what to show the user for a device.
fn display_name(
    device_id: Option<&DeviceId>,
    device_info: Option<&str>,
    device_uri: &str,
) -> String {
    if let Some(info) = device_info.map(str::trim).filter(|info| !info.is_empty()) {
        return info.to_string();
    }

    if let Some(device_id) = device_id {
        let name = [device_id.manufacturer(), device_id.model()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        if !name.trim().is_empty() {
            return name;
        }
    }

    device_uri.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transports_by_scheme() {
        assert_eq!(
            DeviceTransport::from_uri("usb://Acme/X"),
            DeviceTransport::Usb
        );
        assert_eq!(
            DeviceTransport::from_uri("socket://192.0.2.10:9100"),
            DeviceTransport::Socket
        );
        assert_eq!(
            DeviceTransport::from_uri("DNSSD://Printer._pdl-datastream._tcp.local"),
            DeviceTransport::DnsSd
        );
        assert_eq!(
            DeviceTransport::from_uri("ipps://printer.local/ipp/print"),
            DeviceTransport::Ipp
        );
        assert_eq!(
            DeviceTransport::from_uri("hp:/net/HP_LaserJet?ip=192.0.2.10"),
            DeviceTransport::Vendor
        );
        assert_eq!(
            DeviceTransport::from_uri("nonsense"),
            DeviceTransport::Vendor
        );
    }

    #[test]
    fn local_usb_is_preferred_over_a_bare_socket() {
        assert!(DeviceTransport::Usb.preference() < DeviceTransport::Socket.preference());
        assert!(DeviceTransport::Ipp.preference() < DeviceTransport::Socket.preference());
        assert!(DeviceTransport::Vendor.preference() < DeviceTransport::Socket.preference());
    }

    #[test]
    fn normalization_folds_case_and_default_ports() {
        assert_eq!(
            normalized_device_uri("SOCKET://192.0.2.10:9100/"),
            normalized_device_uri("socket://192.0.2.10/")
        );
        assert_eq!(
            normalized_device_uri("ipp://Printer.local:631/ipp/print"),
            "ipp://printer.local/ipp/print"
        );
    }

    #[test]
    fn normalization_does_not_touch_a_vendor_uri_beyond_case() {
        assert_eq!(
            normalized_device_uri("hp:/net/HP_LaserJet?ip=192.0.2.10"),
            "hp:/net/hp_laserjet?ip=192.0.2.10"
        );
    }

    #[test]
    fn endpoints_are_read_from_network_uris_only() {
        assert_eq!(
            uri_endpoint("socket://192.0.2.10:9100"),
            Some(("192.0.2.10".to_string(), Some(9100)))
        );
        assert_eq!(
            uri_endpoint("ipps://[2001:db8::1]:8631/ipp/print"),
            Some(("[2001:db8::1]".to_string(), Some(8631)))
        );
        assert_eq!(
            uri_endpoint("usb://Acme/Test?serial=S1"),
            Some(("Acme".to_string(), None))
        );
        assert_eq!(uri_endpoint("nonsense"), None);
    }

    #[test]
    fn a_uri_that_omits_its_port_still_names_a_whole_endpoint() {
        assert_eq!(
            uri_endpoint("socket://192.0.2.50"),
            Some(("192.0.2.50".to_string(), Some(9100)))
        );
        assert_eq!(
            uri_endpoint("ipp://printer.lan/ipp/print"),
            Some(("printer.lan".to_string(), Some(631)))
        );
        assert_eq!(
            uri_endpoint("lprint://192.0.2.50/"),
            Some(("192.0.2.50".to_string(), None))
        );
    }

    #[test]
    fn a_usb_device_name_is_not_taken_as_a_host() {
        let evidence = evidence(None, "usb://Brother/HL-L2350DW");

        assert_eq!(evidence.network_hostname, None);
        assert_eq!(evidence.network_port, None);
    }

    #[test]
    fn serial_is_read_from_a_usb_uri_when_the_device_id_has_none() {
        let evidence = evidence(None, "usb://Acme/Test%20Laser?serial=abc123");

        assert_eq!(evidence.serial_number.as_deref(), Some("ABC123"));
    }

    #[test]
    fn a_device_id_serial_wins_over_the_uri() {
        let device_id = DeviceId::parse("MFG:Acme;MDL:Test;SN:FROM-ID;");
        let evidence = evidence(Some(&device_id), "usb://Acme/Test?serial=from-uri");

        assert_eq!(evidence.serial_number.as_deref(), Some("FROM-ID"));
    }

    #[test]
    fn display_name_prefers_the_application_description() {
        let device_id = DeviceId::parse("MFG:Acme;MDL:Test Laser;");

        assert_eq!(
            display_name(
                Some(&device_id),
                Some("Acme Test Laser (network)"),
                "socket://x"
            ),
            "Acme Test Laser (network)"
        );
        assert_eq!(
            display_name(Some(&device_id), Some("   "), "socket://x"),
            "Acme Test Laser"
        );
        assert_eq!(display_name(None, None, "socket://x"), "socket://x");
    }
}
