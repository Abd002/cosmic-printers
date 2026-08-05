use crate::grouping::DeviceIdentity;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct SupplyLevel {
    pub name: String,
    pub level_percent: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrinterStatus {
    Ready,
    Offline,
    LowToner,
}

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
///
/// The booleans are derived once from `operations-supported` so callers never
/// have to know which raw operation code means what.
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
    ///
    /// Finding devices is not enough: the application also has to be able to
    /// confirm it has a driver for one, and to create the printer.
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

/// The logical identity of a Printer Application.
///
/// Identity is the DNS-SD service instance: its name, service type, and domain.
/// Everything else about an advertisement is mutable — a Printer Application
/// that restarts on a different port, or that becomes reachable on a second
/// network interface, is still the same application.
///
/// The system UUID is deliberately not part of this. Several Printer
/// Applications running on one machine can report the same `system-uuid`, so
/// using it would silently merge unrelated applications.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PrinterApplicationId {
    service_name: String,
    service_type: String,
    domain: String,
}

impl PrinterApplicationId {
    /// Builds a normalized identity from a DNS-SD service instance.
    ///
    /// Each part is trimmed, has any trailing dot removed, and is lowercased,
    /// so `LPrint._ipps-system._tcp.` in `local.` and `lprint._ipps-system._tcp`
    /// in `local` are one application.
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

    /// Returns the stable string form used as a map key and on the wire.
    ///
    /// Separators inside a part are escaped, because a DNS-SD instance name may
    /// contain any character and two different applications must never encode
    /// to the same key.
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
///
/// `id` is [`PrinterApplicationId::as_key`]. Hostname, port, addresses, system
/// URI, TXT data, and endpoints are mutable discovery data that a later
/// resolution may replace; capabilities and state come from probing.
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
    ///
    /// Distinct from [`PrinterApplication::system_uri`]: that is the IPP endpoint
    /// used to talk to the application, and is not something to open in a
    /// browser. Only `http` and `https` ever appear here.
    pub web_interface_uri: Option<String>,
    /// Endpoints parsed from `system-xri-supported`.
    pub endpoints: Vec<SystemEndpoint>,
    pub capabilities: PrinterApplicationCapabilities,
    pub txt: BTreeMap<String, String>,
    pub state: PrinterApplicationState,
}

impl PrinterApplication {
    /// Merges a repeated DNS-SD resolution of this Printer Application.
    ///
    /// Probe-derived fields and state are preserved. DNS-SD fields are
    /// refreshed, while addresses are accumulated across interfaces.
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

    /// Returns whether this application is reachable only over the loopback
    /// interface.
    ///
    /// It matters because a Printer Application refuses remote administration
    /// unless it has been configured with an authentication service, so a
    /// non-local application cannot be an automatic configuration candidate.
    pub fn is_local(&self) -> bool {
        crate::host_is_local(&self.hostname)
            || self
                .addresses
                .iter()
                .any(|address| crate::host_is_local(address))
    }

    /// Returns the URI to actually administer this application on.
    ///
    /// [`PrinterApplication::system_uri`] records what was advertised, which is not
    /// what an administrative request can use. Two things differ, both verified
    /// against real Printer Applications:
    ///
    /// The host must be loopback. A Printer Application authorizes administration
    /// without credentials only for a loopback peer; a request arriving over the
    /// machine's own LAN address is remote by its reckoning and refused outright,
    /// even though it is the same process on the same computer. Reading attributes
    /// is unrestricted, so the advertised host appears to work right up until the
    /// first operation that matters.
    ///
    /// The scheme must be plain. An application advertises `_ipps-system._tcp` but
    /// negotiates TLS by HTTP upgrade rather than serving it immediately, so
    /// opening a TLS connection to the advertised port fails. On loopback the plain
    /// scheme is also safe, because the traffic never leaves the machine.
    ///
    /// A non-local application keeps its advertised URI. Administering one is
    /// refused regardless, and nothing here should quietly move a remote
    /// conversation off TLS.
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

/// A configured or discovered CUPS destination.
///
/// The destination identity is structured data. CUPS attributes, discovery
/// metadata, and capabilities are stored in the private options map and are
/// exposed through typed methods.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterEntry {
    id: String,
    name: String,
    is_default: bool,
    options: HashMap<String, String>,
}

/// Identifies how a printer endpoint was obtained.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointSource {
    Uri,
    Connected,
}

impl PrinterEntry {
    /// Creates a destination from its identity and normalized options.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        is_default: bool,
        options: HashMap<String, String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            is_default,
            options,
        }
    }

    /// Returns the stable CUPS destination identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the display name reported by CUPS or discovery.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether this destination is the system default.
    pub fn is_default(&self) -> bool {
        self.is_default
    }

    /// Returns a normalized option by its IPP/CUPS name.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .get(name)
            .map(String::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    /// Iterates normalized options for backend operations.
    #[doc(hidden)]
    pub fn options(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.options
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Inserts or replaces a normalized option.
    pub fn set_option(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.options.insert(name.into(), value.into());
    }

    #[doc(hidden)]
    pub fn set_endpoint_source(&mut self, source: EndpointSource) {
        self.set_option(
            "endpoint-source",
            match source {
                EndpointSource::Uri => "uri",
                EndpointSource::Connected => "connected",
            },
        );
    }

    #[doc(hidden)]
    pub fn endpoint_source(&self) -> Option<EndpointSource> {
        match self.option("endpoint-source")? {
            "uri" => Some(EndpointSource::Uri),
            "connected" => Some(EndpointSource::Connected),
            _ => None,
        }
    }

    /// Merges normalized options into this destination.
    pub fn merge_options<I>(&mut self, options: I)
    where
        I: IntoIterator<Item = (String, String)>,
    {
        for (name, value) in options {
            if !value.is_empty() {
                self.set_option(name, value);
            }
        }
    }

    /// Merges a partial CUPS enumeration update while retaining an endpoint
    /// previously selected by a successful device connection.
    pub fn merge_enumeration_record(&mut self, incoming: Self) {
        const CONNECTED_ENDPOINT_OPTIONS: &[&str] = &[
            "endpoint-hostname",
            "endpoint-port",
            "endpoint-address",
            "endpoint-is-local",
            "endpoint-source",
            "dnssd-hostname",
            "dnssd-port",
        ];

        let preserve_connected_endpoint = self.endpoint_source() == Some(EndpointSource::Connected);
        if !incoming.name.is_empty() {
            self.name = incoming.name;
        }
        self.is_default = incoming.is_default;
        self.merge_options(incoming.options.into_iter().filter(|(name, _)| {
            !preserve_connected_endpoint || !CONNECTED_ENDPOINT_OPTIONS.contains(&name.as_str())
        }));
    }

    /// Returns the printer service URI reported by the destination.
    pub fn printer_uri(&self) -> Option<&str> {
        self.option("printer-uri-supported")
            .and_then(preferred_printer_uri)
    }

    /// Returns the destination device URI.
    pub fn device_uri(&self) -> Option<&str> {
        self.option("device-uri")
    }

    /// Returns the configured web interface URL.
    pub fn web_page(&self) -> Option<&str> {
        self.option("printer-more-info")
    }

    /// Sets the printer location option.
    pub fn set_location(&mut self, location: impl Into<String>) {
        self.set_option("printer-location", location);
    }

    /// Returns the printer location.
    pub fn location(&self) -> Option<&str> {
        self.option("printer-location")
    }

    /// Returns the printer make and model.
    pub fn model(&self) -> Option<&str> {
        self.option("printer-make-and-model")
    }

    /// Returns the driver version, when reported by the backend.
    pub fn driver_version(&self) -> Option<&str> {
        self.option("printer-driver-version")
    }

    /// Returns the endpoint hostname.
    pub fn hostname(&self) -> Option<&str> {
        self.option("dnssd-hostname")
            .or_else(|| self.option("endpoint-hostname"))
    }

    /// Returns the endpoint port.
    pub fn port(&self) -> Option<u16> {
        self.option("dnssd-port")
            .or_else(|| self.option("endpoint-port"))
            .and_then(|port| port.parse().ok())
    }

    /// Returns the resource path the endpoint resolved to.
    ///
    /// A discovered printer's own URI names a DNS-SD service rather than a printer,
    /// so this is what says which printer on that endpoint it is.
    pub fn endpoint_resource_path(&self) -> Option<&str> {
        self.option("endpoint-resource-path")
    }

    /// Returns whether the endpoint is on this machine.
    ///
    /// Uses what connecting to the device observed, when it was recorded, and
    /// otherwise judges the hostname — where a name that would need resolving
    /// counts as remote, because deciding this must not block on DNS.
    pub fn endpoint_is_local(&self) -> bool {
        self.option("endpoint-is-local")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| self.endpoint_host().is_some_and(crate::host_is_local))
    }

    /// Returns the host and port to contact this destination at.
    ///
    /// A local endpoint is named `localhost` rather than by the machine's own name,
    /// because a Printer Application treats a request arriving over the machine's
    /// LAN address as remote and refuses it.
    pub fn endpoint(&self) -> Option<(String, u16)> {
        let host = self.endpoint_host()?;
        let host = if self.endpoint_is_local() {
            "localhost"
        } else {
            host
        };

        Some((host.to_string(), self.port()?))
    }

    fn endpoint_host(&self) -> Option<&str> {
        self.hostname().or_else(|| self.endpoint_address())
    }

    /// Returns the current operational status.
    pub fn status(&self) -> PrinterStatus {
        if self
            .option_values("printer-state-reasons")
            .iter()
            .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
        {
            return PrinterStatus::LowToner;
        }

        match self.option("printer-state") {
            Some("5") => PrinterStatus::Offline,
            Some("3" | "4") => PrinterStatus::Ready,
            _ => PrinterStatus::Ready,
        }
    }

    /// Returns a status message suitable for a queue row.
    pub fn queue_status(&self) -> Option<&str> {
        self.option("queue-status")
            .or_else(|| self.option("printer-state-message"))
    }

    /// Returns supported media values.
    pub fn paper_sizes(&self) -> Vec<String> {
        self.option_values("media-supported")
    }

    /// Returns supported sides values.
    pub fn print_sides(&self) -> Vec<String> {
        self.option_values("sides-supported")
    }

    /// Returns the default media value.
    pub fn default_paper_size(&self) -> Option<&str> {
        self.option("media-default")
    }

    /// Sets the default media value.
    pub fn set_default_paper_size(&mut self, paper_size: impl Into<String>) {
        self.set_option("media-default", paper_size);
    }

    /// Returns the default sides value.
    pub fn default_print_sides(&self) -> Option<&str> {
        self.option("sides-default")
    }

    /// Sets the default sides value.
    pub fn set_default_print_sides(&mut self, print_sides: impl Into<String>) {
        self.set_option("sides-default", print_sides);
    }

    /// Returns the printer UUID, including aliases used by DNS-SD metadata.
    pub fn printer_uuid(&self) -> Option<&str> {
        self.option("printer-uuid")
            .or_else(|| self.option("uuid"))
            .or_else(|| self.option("UUID"))
    }

    /// Returns a separately reported physical device UUID.
    pub fn device_uuid(&self) -> Option<&str> {
        self.option("device-uuid")
    }

    /// Returns the resolved network address used by grouping.
    pub fn endpoint_address(&self) -> Option<&str> {
        self.option("endpoint-address")
    }

    /// Returns all reported supply levels.
    pub fn supplies(&self) -> Vec<SupplyLevel> {
        let names = self.option_values("marker-names");
        let levels = self.option_values("marker-levels");

        names
            .into_iter()
            .zip(levels)
            .filter_map(|(name, level)| {
                let level_percent = level.parse::<i32>().ok()?.clamp(0, 100) as u8;
                Some(SupplyLevel {
                    name,
                    level_percent,
                })
            })
            .collect()
    }

    fn option_values(&self, name: &str) -> Vec<String> {
        self.option(name)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Merges a partial or resolved DNS-SD record into this discovered printer.
    pub fn merge_discovery_record(&mut self, incoming: Self) {
        if self.name.is_empty() {
            self.name = incoming.name;
        }

        self.merge_options(incoming.options);
    }
}

fn preferred_printer_uri(value: &str) -> Option<&str> {
    let mut uris = value
        .split(',')
        .map(str::trim)
        .filter(|uri| !uri.is_empty());
    let first = uris.next()?;

    Some(
        std::iter::once(first)
            .chain(uris)
            .find(|uri| {
                uri.get(..7)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("ipps://"))
            })
            .unwrap_or(first),
    )
}

#[cfg(test)]
mod printer_entry_tests {
    use super::*;

    fn printer(options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            "printer",
            "Printer",
            false,
            options
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        )
    }

    #[test]
    fn normalizes_printer_uuid_aliases() {
        assert_eq!(
            printer(&[("printer-uuid", "urn:uuid:standard")]).printer_uuid(),
            Some("urn:uuid:standard")
        );
        assert_eq!(printer(&[("uuid", "lower")]).printer_uuid(), Some("lower"));
        assert_eq!(printer(&[("UUID", "upper")]).printer_uuid(), Some("upper"));
    }

    #[test]
    fn returns_printer_more_info_as_web_page() {
        assert_eq!(
            printer(&[("printer-more-info", "https://printer.local/")]).web_page(),
            Some("https://printer.local/")
        );
    }

    #[test]
    fn optional_metadata_can_be_absent() {
        let printer = printer(&[("device-uri", "ipps://printer._ipps._tcp.local/")]);

        assert_eq!(printer.printer_uuid(), None);
        assert_eq!(printer.web_page(), None);
    }

    #[test]
    fn secure_printer_uri_is_preferred_from_supported_values() {
        let printer = printer(&[(
            "printer-uri-supported",
            "ipp://host:8889/ipp/print,ipps://host:8889/ipp/print",
        )]);

        assert_eq!(printer.printer_uri(), Some("ipps://host:8889/ipp/print"));
    }

    #[test]
    fn enumeration_preserves_connected_endpoint() {
        let mut existing = printer(&[
            ("endpoint-hostname", "printer.local"),
            ("endpoint-port", "8000"),
            ("endpoint-is-local", "true"),
            ("endpoint-source", "connected"),
        ]);
        let incoming = printer(&[
            ("endpoint-hostname", "printer._ipps._tcp.local"),
            ("endpoint-port", "631"),
            ("printer-location", "Office"),
        ]);

        existing.merge_enumeration_record(incoming);

        assert_eq!(existing.hostname(), Some("printer.local"));
        assert_eq!(existing.port(), Some(8000));
        assert_eq!(existing.endpoint_address(), None);
        assert_eq!(existing.endpoint_source(), Some(EndpointSource::Connected));
        assert_eq!(existing.location(), Some("Office"));
    }
}

/// A set of active destinations that appear to come from the same source.
///
/// This is the main-view grouping domain: it answers "which of the printers the
/// system already has belong together", where a source is a physical IPP device,
/// a Printer Application, or a remote CUPS server. A Printer Application group
/// may hold many queues for many different physical printers.
///
/// It is not the Add Printer domain. Add Printer groups PA-owned observations of
/// printers that are *not yet configured*, by physical hardware — see
/// [`crate::DiscoveredPhysicalPrinter`] and
/// [`crate::group_by_physical_device`]. The two use different evidence and
/// different rules, and neither reuses the other's types.
#[derive(Debug, Clone)]
pub struct GroupedDevice {
    pub(crate) identity: DeviceIdentity,
    pub(crate) application: Option<PrinterApplication>,
    pub(crate) queues: Vec<PrinterEntry>,
}

impl GroupedDevice {
    /// Returns Printer Application metadata for this device, when discovered.
    pub fn printer_application(&self) -> Option<&PrinterApplication> {
        self.application.as_ref()
    }

    /// Returns every configured queue associated with this physical device.
    pub fn queues(&self) -> &[PrinterEntry] {
        &self.queues
    }

    /// Returns the normalized printer UUID used for strongest matching.
    pub fn uuid(&self) -> Option<&str> {
        self.identity.uuid()
    }

    /// Returns the normalized hostname used when no UUID is available.
    pub fn hostname(&self) -> Option<&str> {
        self.identity.hostname()
    }

    /// Returns the URI port used for host-and-port matching.
    pub fn port(&self) -> Option<u16> {
        self.identity.port()
    }

    /// Returns the normalized URI used as the final matching fallback.
    pub fn device_uri_prefix(&self) -> Option<&str> {
        self.identity.uri()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListPrintersReply {
    pub printers: Vec<PrinterEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListPrinterApplicationsReply {
    pub printer_applications: Vec<PrinterApplication>,
}

/// What changed, so a client knows which cache to re-read.
///
/// Each kind names one thing that became stale. There is deliberately no
/// general-purpose "something changed" event: a client that wants Add Printer
/// results should not be woken by an unrelated queue going offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrintersEventKind {
    /// The set or attributes of available destinations changed.
    AvailableDestinationsChanged,
    /// The set or state of discovered Printer Applications changed.
    PrinterApplicationsChanged,
    /// An Add Printer discovery generation produced new results.
    AddPrinterDiscoveryChanged,
    /// A printer configuration attempt changed state.
    PrinterConfigurationChanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrintersEvent {
    pub kind: PrintersEventKind,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct GetJobsReply {
    pub jobs: Vec<JobInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrintTestPageReply {
    pub job_id: i32,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type,
)]
pub enum JobFilter {
    #[default]
    Active,
    Completed,
    All,
}

#[derive(Clone, Debug, Serialize, Deserialize, zlink::introspect::Type)]
pub struct JobInfo {
    pub id: i32,
    pub printer_id: String,
    pub title: String,
    pub state: JobState,
    pub user: String,
    pub size: i32,
    pub priority: i32,
    pub creation_time: i64,
    pub processing_time: i64,
    pub completed_time: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, zlink::introspect::Type)]
pub enum JobState {
    Pending,
    Processing,
    Completed,
    Canceled,
    Aborted,
    Held,
    Stopped,
    Failed,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(id: &str, name: &str, options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            id,
            name,
            false,
            options
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        )
    }

    #[test]
    fn discovery_merge_fills_name_and_refreshes_options() {
        let mut existing = printer("", "", &[("endpoint-address", "192.0.2.1")]);
        let incoming = printer(
            "",
            "Office Printer",
            &[
                ("endpoint-address", "192.0.2.2"),
                ("printer-location", "Office"),
            ],
        );

        existing.merge_discovery_record(incoming);

        assert_eq!(existing.name(), "Office Printer");
        assert_eq!(existing.endpoint_address(), Some("192.0.2.2"));
        assert_eq!(existing.location(), Some("Office"));
    }

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
        // Two Printer Applications on one machine can report the same
        // system-uuid, so identity must not depend on it. Nothing in
        // PrinterApplication carries one.
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
        // A Printer Application refuses remote administration unless it was
        // given an authentication service, so it can only be set up by hand.
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
