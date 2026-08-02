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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrinterApplicationState {
    Discovered,
    Ready,
    Unsupported,
    AuthenticationRequired,
    Unreachable,
    Failed,
}

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
    pub system_uuid: Option<String>,
    pub make_and_model: Option<String>,
    pub operations_supported: Vec<u16>,
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

    /// Returns the printer service URI reported by the destination.
    pub fn printer_uri(&self) -> Option<&str> {
        self.option("printer-uri-supported")
            .or_else(|| self.option("printer-local-uri"))
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

    /// Returns the physical device UUID used by grouping.
    pub fn device_uuid(&self) -> Option<&str> {
        self.option("device-uuid")
    }

    /// Returns the DNS-SD address used by grouping.
    pub fn dnssd_address(&self) -> Option<&str> {
        self.option("dnssd-address")
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
pub struct ListDiscoveredPrintersReply {
    pub printers: Vec<PrinterEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListPrinterApplicationsReply {
    pub printer_applications: Vec<PrinterApplication>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrintersEventKind {
    DiscoveredPrintersChanged,
    PrinterApplicationsChanged,
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
        let mut existing = printer("", "", &[("dnssd-address", "192.0.2.1")]);
        let incoming = printer(
            "",
            "Office Printer",
            &[
                ("dnssd-address", "192.0.2.2"),
                ("printer-location", "Office"),
            ],
        );

        existing.merge_discovery_record(incoming);

        assert_eq!(existing.name(), "Office Printer");
        assert_eq!(existing.dnssd_address(), Some("192.0.2.2"));
        assert_eq!(existing.location(), Some("Office"));
    }

    #[test]
    fn printer_application_discovery_merge_preserves_probe_results() {
        let mut existing = PrinterApplication {
            id: "app".into(),
            service_name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: "printer.local".into(),
            port: 8000,
            addresses: vec!["192.0.2.1".into()],
            system_uri: "ipps://printer.local:8000/ipp/system".into(),
            system_uuid: Some("urn:uuid:system".into()),
            make_and_model: Some("LPrint".into()),
            operations_supported: vec![0x402b],
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Ready,
        };
        let mut incoming = existing.clone();
        incoming.addresses = vec!["2001:db8::1".into()];
        incoming.system_uuid = None;
        incoming.make_and_model = None;
        incoming.operations_supported.clear();
        incoming.state = PrinterApplicationState::Discovered;

        existing.merge_discovery_record(incoming);

        assert_eq!(
            existing.addresses,
            vec!["192.0.2.1".to_string(), "2001:db8::1".to_string()]
        );
        assert_eq!(existing.system_uuid.as_deref(), Some("urn:uuid:system"));
        assert_eq!(existing.operations_supported, vec![0x402b]);
        assert_eq!(existing.state, PrinterApplicationState::Ready);
    }
}
