use crate::DeviceIdentity;
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
    pub fn merge_from(&mut self, incoming: Self) {
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

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterEntry {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub printer_local_uri: String,
    pub status: PrinterStatus,
    pub queue_status: String,
    pub location: String,
    pub model: String,
    pub device_uri: String,
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub web_page: Option<String>,
    pub driver_version: String,
    pub paper_size_idx: usize,
    pub print_sides_idx: usize,
    pub options: HashMap<String, String>,
    pub supplies: Vec<SupplyLevel>,
    pub paper_sizes: Vec<String>,
    pub print_sides: Vec<String>,
}

impl PrinterEntry {
    pub fn merge_from(&mut self, incoming: Self) {
        if self.name.is_empty() {
            self.name = incoming.name;
        }
        if self.printer_local_uri.is_empty() {
            self.printer_local_uri = incoming.printer_local_uri;
        }
        if self.queue_status.is_empty() {
            self.queue_status = incoming.queue_status;
        }
        if self.location.is_empty() {
            self.location = incoming.location;
        }
        if self.model.is_empty() {
            self.model = incoming.model;
        }
        if self.device_uri.is_empty() {
            self.device_uri = incoming.device_uri;
        }
        if self.hostname.is_none() {
            self.hostname = incoming.hostname;
        }
        if self.port.is_none() {
            self.port = incoming.port;
        }
        if self.web_page.is_none() {
            self.web_page = incoming.web_page;
        }
        if self.driver_version.is_empty() {
            self.driver_version = incoming.driver_version;
        }
        if self.supplies.is_empty() {
            self.supplies = incoming.supplies;
        }
        if self.paper_sizes.is_empty() {
            self.paper_sizes = incoming.paper_sizes;
        }
        if self.print_sides.is_empty() {
            self.print_sides = incoming.print_sides;
        }

        for (key, value) in incoming.options {
            if !value.is_empty() {
                self.options.insert(key, value);
            }
        }
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
