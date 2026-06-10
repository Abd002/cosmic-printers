use crate::DeviceIdentity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    pub web_page: Option<String>,
    pub driver_version: String,
    pub paper_size_idx: usize,
    pub print_sides_idx: usize,
    pub options: HashMap<String, String>,
    pub supplies: Vec<SupplyLevel>,
    pub paper_sizes: Vec<String>,
    pub print_sides: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GroupedDevice {
    pub(crate) identity: DeviceIdentity,
    pub(crate) queues: Vec<PrinterEntry>,
}

impl GroupedDevice {
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
pub struct DiscoveredPrinter {
    pub id: String,
    pub name: String,
    pub device_uri: String,
    pub location: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListDiscoveredPrintersReply {
    pub printers: Vec<DiscoveredPrinter>,
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
