//! Printer Application data shared across service boundaries.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a Printer Application can currently be used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrinterApplicationState {
    /// Advertised, not yet probed.
    Discovered,
    /// Being probed for its capabilities.
    Probing,
    /// Can find devices, match drivers, and create printers.
    Ready,
    /// Can find devices but cannot create printers remotely, so it cannot be an
    /// automatic configuration candidate.
    DiscoveryOnly,
    /// Offers no usable IPP administration, only its own web interface.
    ManualSetupOnly,
    /// Needs credentials before it will answer.
    AuthenticationRequired,
    /// Could not be reached.
    Unreachable,
    /// Answered, but implements none of the operations Add Printer needs.
    Unsupported,
    /// Answered unusably.
    Failed,
}

/// What a Printer Application told us it can do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterApplicationCapabilities {
    /// Implements `PAPPL-Find-Devices`.
    pub find_devices: bool,
    /// Implements `PAPPL-Find-Drivers`.
    pub find_drivers: bool,
    /// Implements `Create-Printer`.
    pub create_printer: bool,
    /// Implements `Get-Printers`, which is what makes duplicate detection
    /// possible.
    pub get_printers: bool,
    /// Implements `Delete-Printer`.
    pub delete_printer: bool,
    /// Implements `PAPPL-Create-Printers`, the batch "add everything" call.
    pub create_printers_batch: bool,
    /// Every operation code reported, kept for diagnostics.
    pub operations_supported: Vec<u16>,
    /// Attributes the application accepts when creating a printer. Advisory:
    /// PAPPL validates its own required set regardless.
    pub printer_creation_attributes_supported: Vec<String>,
    /// Attributes the application says are mandatory when creating a printer.
    pub mandatory_printer_attributes: Vec<String>,
    /// Device URI schemes the application can drive.
    pub device_uri_schemes_supported: Vec<String>,
    /// Service types the application can create, such as `print`.
    pub printer_service_types_supported: Vec<String>,
}

/// `PAPPL-Find-Devices`.
const OPERATION_FIND_DEVICES: u16 = 0x402b;
/// `PAPPL-Find-Drivers`.
const OPERATION_FIND_DRIVERS: u16 = 0x402c;
/// `PAPPL-Create-Printers`.
const OPERATION_CREATE_PRINTERS: u16 = 0x402d;
/// `Create-Printer`.
const OPERATION_CREATE_PRINTER: u16 = 0x004c;
/// `Delete-Printer`.
const OPERATION_DELETE_PRINTER: u16 = 0x004e;
/// `Get-Printers`.
const OPERATION_GET_PRINTERS: u16 = 0x004f;
/// `CUPS-Get-Printers`, which PAPPL accepts as an alias for `Get-Printers`.
const OPERATION_CUPS_GET_PRINTERS: u16 = 0x4002;

impl PrinterApplicationCapabilities {
    /// Derives the typed capabilities from a reported operation list.
    pub fn from_operations(operations: Vec<u16>) -> Self {
        let supports = |operation: u16| operations.contains(&operation);

        Self {
            find_devices: supports(OPERATION_FIND_DEVICES),
            find_drivers: supports(OPERATION_FIND_DRIVERS),
            create_printer: supports(OPERATION_CREATE_PRINTER),
            get_printers: supports(OPERATION_GET_PRINTERS) || supports(OPERATION_CUPS_GET_PRINTERS),
            delete_printer: supports(OPERATION_DELETE_PRINTER),
            create_printers_batch: supports(OPERATION_CREATE_PRINTERS),
            operations_supported: operations,
            ..Self::default()
        }
    }

    /// Returns true when this application can carry an Add Printer flow through
    /// to a created printer without the user visiting its web interface.
    pub fn supports_automatic_configuration(&self) -> bool {
        self.find_devices && self.find_drivers && self.create_printer
    }
}

/// One endpoint a Printer Application's system service is reachable at, as
/// reported in `system-xri-supported`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct SystemEndpoint {
    pub uri: String,
    /// The authentication the endpoint requires, when reported.
    pub authentication: Option<String>,
    /// The transport security the endpoint uses, when reported.
    pub security: Option<String>,
}

/// DNS-SD identity of a Printer Application, excluding the host-wide `system-uuid`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PrinterApplicationId {
    service_name: String,
    service_type: String,
    domain: String,
}

impl PrinterApplicationId {
    /// Builds a normalized identity from a DNS-SD service instance.
    pub fn new(service_name: &str, service_type: &str, domain: &str) -> Self {
        Self {
            service_name: normalize_dnssd_part(service_name),
            service_type: normalize_dnssd_part(service_type),
            domain: normalize_dnssd_part(domain),
        }
    }

    /// Returns the normalized DNS-SD instance name.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Returns the normalized DNS-SD service type.
    pub fn service_type(&self) -> &str {
        &self.service_type
    }

    /// Returns the normalized DNS-SD domain.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// Returns an unambiguous escaped key for the DNS-SD identity.
    pub fn as_key(&self) -> String {
        format!(
            "dnssd-system:{}:{}:{}",
            escape_key_part(&self.service_name),
            escape_key_part(&self.service_type),
            escape_key_part(&self.domain),
        )
    }
}

impl std::fmt::Display for PrinterApplicationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_key())
    }
}

fn normalize_dnssd_part(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn escape_key_part(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A")
}

/// A Printer Application discovered on the network.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterApplication {
    pub id: String,
    pub service_name: String,
    pub service_type: String,
    pub domain: String,
    pub hostname: String,
    pub port: u16,
    pub addresses: Vec<String>,
    pub system_uri: String,
    pub make_and_model: Option<String>,
    /// The application's own administration page, when it has a usable one.
    pub web_interface_uri: Option<String>,
    /// Endpoints parsed from `system-xri-supported`.
    pub endpoints: Vec<SystemEndpoint>,
    pub capabilities: PrinterApplicationCapabilities,
    pub txt: BTreeMap<String, String>,
    pub state: PrinterApplicationState,
}

impl PrinterApplication {
    /// Merges a repeated DNS-SD resolution of this Printer Application.
    pub fn merge_discovery_record(&mut self, incoming: Self) {
        self.service_name = incoming.service_name;
        self.service_type = incoming.service_type;
        self.domain = incoming.domain;
        self.hostname = incoming.hostname;
        self.port = incoming.port;
        self.system_uri = incoming.system_uri;
        self.txt = incoming.txt;

        for address in incoming.addresses {
            if !self.addresses.contains(&address) {
                self.addresses.push(address);
            }
        }
        self.addresses.sort();
    }

    /// Returns whether the application is reachable over loopback.
    pub fn is_local(&self) -> bool {
        crate::host_is_local(&self.hostname)
            || self
                .addresses
                .iter()
                .any(|address| crate::host_is_local(address))
    }

    /// Returns a loopback URI because PAPPL rejects unauthenticated administration over LAN addresses.
    pub fn administration_uri(&self) -> String {
        if !self.is_local() {
            return self.system_uri.clone();
        }

        format!("ipp://localhost:{}/ipp/system", self.port)
    }

    /// Returns whether Add Printer can configure through this application
    /// without sending the user to its web interface.
    pub fn supports_automatic_configuration(&self) -> bool {
        self.state == PrinterApplicationState::Ready
            && self.capabilities.supports_automatic_configuration()
            && self.is_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer_application(service_name: &str, hostname: &str, port: u16) -> PrinterApplication {
        let id = PrinterApplicationId::new(service_name, "_ipps-system._tcp", "local.");

        PrinterApplication {
            id: id.as_key(),
            service_name: service_name.into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: hostname.into(),
            port,
            addresses: vec!["192.0.2.1".into()],
            system_uri: format!("ipps://{hostname}:{port}/ipp/system"),
            make_and_model: Some(service_name.into()),
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: PrinterApplicationCapabilities::from_operations(vec![
                OPERATION_FIND_DEVICES,
                OPERATION_FIND_DRIVERS,
                OPERATION_CREATE_PRINTER,
            ]),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Ready,
        }
    }

    #[test]
    fn printer_application_discovery_merge_preserves_probe_results() {
        let mut existing = printer_application("LPrint", "printer.local", 8000);
        let mut incoming = existing.clone();
        incoming.addresses = vec!["2001:db8::1".into()];
        incoming.make_and_model = None;
        incoming.capabilities = PrinterApplicationCapabilities::default();
        incoming.state = PrinterApplicationState::Discovered;

        existing.merge_discovery_record(incoming);

        assert_eq!(
            existing.addresses,
            vec!["192.0.2.1".to_string(), "2001:db8::1".to_string()]
        );
        assert!(existing.capabilities.find_devices);
        assert_eq!(existing.state, PrinterApplicationState::Ready);
    }

    #[test]
    fn identity_ignores_the_endpoint_so_a_restart_updates_one_application() {
        let first = printer_application("LPrint", "printer.local", 8000);
        let restarted = printer_application("LPrint", "desktop.local", 8001);

        assert_eq!(first.id, restarted.id);
    }

    #[test]
    fn identity_normalizes_case_and_trailing_dots() {
        assert_eq!(
            PrinterApplicationId::new("LPrint", "_IPPS-System._tcp.", "Local."),
            PrinterApplicationId::new(" lprint ", "_ipps-system._tcp", "local")
        );
    }

    #[test]
    fn identity_keeps_different_services_apart_even_with_one_system_uuid() {
        let first = printer_application("LPrint", "localhost", 8000);
        let second = printer_application("PostScript Printer Application", "localhost", 8001);

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn identity_keys_escape_separators_in_a_service_name() {
        let colon = PrinterApplicationId::new("weird:name", "_ipps-system._tcp", "local");
        let split = PrinterApplicationId::new("weird", "name:_ipps-system._tcp", "local");

        assert_ne!(colon.as_key(), split.as_key());
    }

    #[test]
    fn capabilities_are_derived_from_reported_operations() {
        let capabilities = PrinterApplicationCapabilities::from_operations(vec![
            OPERATION_FIND_DEVICES,
            OPERATION_FIND_DRIVERS,
            OPERATION_CREATE_PRINTER,
            OPERATION_CUPS_GET_PRINTERS,
        ]);

        assert!(capabilities.find_devices);
        assert!(capabilities.find_drivers);
        assert!(capabilities.create_printer);
        assert!(capabilities.get_printers);
        assert!(!capabilities.delete_printer);
        assert!(!capabilities.create_printers_batch);
        assert!(capabilities.supports_automatic_configuration());
    }

    #[test]
    fn discovery_without_creation_is_not_automatically_configurable() {
        let capabilities = PrinterApplicationCapabilities::from_operations(vec![
            OPERATION_FIND_DEVICES,
            OPERATION_FIND_DRIVERS,
        ]);

        assert!(!capabilities.supports_automatic_configuration());
    }

    #[test]
    fn a_remote_application_is_never_an_automatic_candidate() {
        let mut remote = printer_application("LPrint", "printer.example", 8000);
        remote.addresses = vec!["198.51.100.7".into()];

        assert!(!remote.is_local());
        assert!(!remote.supports_automatic_configuration());

        let mut local = remote.clone();
        local.hostname = "localhost".into();
        local.addresses = vec!["127.0.0.1".into()];

        assert!(local.is_local());
        assert!(local.supports_automatic_configuration());
    }
}
