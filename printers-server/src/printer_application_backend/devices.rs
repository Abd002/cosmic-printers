//! `PAPPL-Find-Devices`: asking one Printer Application what it can see.
//!
//! Every device this returns is *owned by that application*. The device URI is
//! opaque — `hp:/net/...`, `usb://...`, `socket://...`, or something a vendor
//! invented — and only means anything to the application that produced it. It is
//! preserved byte for byte and replayed only to that same application. A
//! separately normalized form exists purely for comparison.
//!
//! Find-Devices does not filter by driver support: a Printer Application will
//! happily report a printer it has no driver for. Deciding whether it can
//! actually drive one is [`super::drivers`]' job.

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
    ///
    /// An unrecognized scheme is a vendor backend, not an error: a Printer
    /// Application is entitled to its own addressing.
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

    /// Returns how strongly this transport should be preferred, lower first.
    ///
    /// A local USB attachment is the most direct route to a printer that is
    /// physically here. A secure network path beats an unencrypted one. A vendor
    /// scheme sits above a bare socket, because an application that offers its
    /// own backend usually needs it to drive the device properly.
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

/// The transports to ask for one at a time when asking for all of them failed.
///
/// A combined scan fails as a unit, so one broken transport hides every printer
/// the others found — an SNMP sweep with nothing to answer it reports the host is
/// down, and a delegated backend that cannot run reports an invalid argument.
/// Both were observed on applications that scanned DNS-SD and USB perfectly well
/// in the same breath.
const EACH_DEVICE_TYPE: &[&str] = &["dns-sd", "usb", "snmp", "other-local", "other-network"];

/// Asks a Printer Application which devices it can see.
///
/// Returns the observations it reported. A collection that cannot be turned into
/// a usable observation is dropped and counted rather than failing the scan,
/// because one malformed entry should not hide the devices that parsed. The same
/// reasoning extends to a whole transport: if asking for everything fails, each
/// transport is asked separately and whatever answers is kept.
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

    // An application that scanned and found nothing answers `not-found` rather
    // than an empty success. That is a complete scan with no devices, not a
    // failure: treating it as one would mark a perfectly healthy application as
    // broken every time no printer happened to be plugged in.
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

    // PAPPL reports one smi55357-device-col per device, so every value of the
    // repeated attribute has to be read; looking at only the first would see one
    // device. Some versions also respond twice on one code path, which shows up
    // as duplicate collections and is deduplicated below.
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
    /// How many collections were unusable. Reported so a Printer Application
    /// that returns malformed devices can be diagnosed instead of silently
    /// appearing to have found fewer printers.
    pub(crate) quarantined: usize,
}

/// Builds an observation from one device collection.
///
/// Returns `None` when the collection has no usable device URI, which is the one
/// member that cannot be substituted: without it there is nothing to configure.
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

/// Gathers identity evidence from the device ID and the URI.
///
/// The device ID is the good source. The URI contributes the endpoint and, for
/// schemes that carry one, a serial number — but only as a comparison-only
/// normalized form, never as something to send anywhere.
pub(super) fn evidence(device_id: Option<&DeviceId>, device_uri: &str) -> PhysicalDeviceEvidence {
    let mut evidence = device_id
        .map(PhysicalDeviceEvidence::from_device_id)
        .unwrap_or_default();
    evidence.set_normalized_device_uri(&normalized_device_uri(device_uri));

    if let Some(service) = dns_sd_service(device_uri) {
        evidence.set_dns_sd_service(&service);
    }
    if let Some((host, port)) = uri_endpoint(device_uri) {
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

/// Normalizes a device URI for comparison only.
///
/// Lowercases the scheme and authority, drops a default port, and drops the
/// fragment, so two applications that spell the same endpoint differently still
/// compare equal. The original URI is untouched.
fn normalized_device_uri(uri: &str) -> String {
    let without_fragment = uri.split('#').next().unwrap_or(uri);
    let Some((scheme, rest)) = without_fragment.split_once("://") else {
        return without_fragment.to_ascii_lowercase();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let authority = authority.to_ascii_lowercase();
    let authority = match (scheme.as_str(), authority.rsplit_once(':')) {
        ("socket", Some((host, "9100"))) => host.to_string(),
        ("ipp", Some((host, "631"))) | ("ipps", Some((host, "631"))) => host.to_string(),
        _ => authority,
    };

    format!("{scheme}://{authority}/{path}")
}

/// Extracts a host and port from a device URI, when it has a network authority.
fn uri_endpoint(uri: &str) -> Option<(String, Option<u16>)> {
    let (_, rest) = uri.split_once("://")?;
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
        return Some((host.to_string(), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => Some((host.to_string(), Some(port))),
            // A colon that is not a port belongs to the host, such as a USB
            // device name containing one.
            Err(_) => Some((authority.to_string(), None)),
        },
        None => Some((authority.to_string(), None)),
    }
}

/// Returns the DNS-SD service instance a device URI names, if it names one.
///
/// One application can report one service under more than one device scheme —
/// PAPPL's own `dnssd:`, and `cups:dnssd:` where it also offers the CUPS
/// backends — spelling the instance name differently each time: percent escapes
/// in one, DNS-SD's `\032` escapes in the other, and a `uuid` query on only one
/// of them. Reducing both to the service they name is what keeps one printer
/// from becoming two rows.
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
///
/// A service instance shown as `Office Printer` travels as
/// `Office\032Printer`, so the escapes have to come out before two spellings of
/// one name can be compared.
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

/// Reads a query parameter out of a device URI.
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
///
/// The application's own description is best, since it is what that application
/// would call the printer. The device ID's make and model is the fallback, and
/// the URI is the last resort so a row is never nameless.
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
