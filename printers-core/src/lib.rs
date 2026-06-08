use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zlink::{ReplyError, introspect};

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

impl PrinterStatus {
    fn label(&self) -> String {
        match self {
            Self::Ready => "printer-ready".to_string(),
            Self::Offline => "printer-offline".to_string(),
            Self::LowToner => "printer-low-toner".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterEntry {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub printer_uri: String,
    pub status: PrinterStatus,
    pub queue_status: String,
    pub location: String,
    pub model: String,
    pub device_name: String,
    pub web_page: Option<String>,
    pub driver_version: String,
    pub paper_size_idx: usize,
    pub print_sides_idx: usize,
    pub options: HashMap<String, String>,
    pub supplies: Vec<SupplyLevel>,
    pub paper_sizes: Vec<String>,
    pub print_sides: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct GroupedDevice {
    uuid: Option<String>,
    hostname: Option<String>,
    port: Option<u16>,
    device_uri_prefix: Option<String>,
    queues: Vec<PrinterEntry>,
}

impl GroupedDevice {
    pub fn queues(&self) -> &[PrinterEntry] {
        &self.queues
    }

    pub fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    pub fn hostname(&self) -> Option<&str> {
        self.hostname.as_deref()
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn device_uri_prefix(&self) -> Option<&str> {
        self.device_uri_prefix.as_deref()
    }

    fn new(printer: PrinterEntry) -> Self {
        let uuid = non_empty_option(&printer.options, "printer-uuid").map(str::to_ascii_lowercase);
        let endpoint = printer_uri(&printer).and_then(parse_uri_endpoint);
        let device_uri_prefix = printer_uri(&printer).map(uri_prefix);

        Self {
            uuid,
            hostname: endpoint.as_ref().map(|(hostname, _)| hostname.clone()),
            port: endpoint.map(|(_, port)| port),
            device_uri_prefix,
            queues: vec![printer],
        }
    }

    fn matches(&self, printer: &PrinterEntry) -> bool {
        let printer_uuid =
            non_empty_option(&printer.options, "printer-uuid").map(str::to_ascii_lowercase);

        if self.uuid.is_some() || printer_uuid.is_some() {
            return self.uuid == printer_uuid;
        }

        let printer_endpoint = printer_uri(printer).and_then(parse_uri_endpoint);
        if let (Some(group_hostname), Some(group_port), Some((hostname, port))) =
            (&self.hostname, self.port, printer_endpoint)
        {
            return group_hostname == &hostname && group_port == port;
        }

        let printer_uri_prefix = printer_uri(printer).map(uri_prefix);
        match (&self.device_uri_prefix, printer_uri_prefix) {
            (Some(group_prefix), Some(prefix)) => group_prefix == &prefix,
            _ => false,
        }
    }

    fn add(&mut self, printer: PrinterEntry) {
        if self.uuid.is_none() {
            self.uuid =
                non_empty_option(&printer.options, "printer-uuid").map(str::to_ascii_lowercase);
        }

        if self.hostname.is_none() || self.port.is_none() {
            if let Some((hostname, port)) = printer_uri(&printer).and_then(parse_uri_endpoint) {
                self.hostname = Some(hostname);
                self.port = Some(port);
            }
        }

        if self.device_uri_prefix.is_none() {
            self.device_uri_prefix = printer_uri(&printer).map(uri_prefix);
        }

        self.queues.push(printer);
    }
}

pub fn group_printers(printers: Vec<PrinterEntry>) -> Vec<GroupedDevice> {
    let mut devices = Vec::<GroupedDevice>::new();

    for printer in printers {
        if let Some(device) = devices.iter_mut().find(|device| device.matches(&printer)) {
            device.add(printer);
        } else {
            devices.push(GroupedDevice::new(printer));
        }
    }

    for device in &mut devices {
        device.queues.sort_by(|left, right| left.id.cmp(&right.id));
    }

    devices
}

fn printer_uri(printer: &PrinterEntry) -> Option<&str> {
    non_empty_option(&printer.options, "device-uri")
        .or_else(|| non_empty_option(&printer.options, "printer-uri-supported"))
}

fn non_empty_option<'a>(options: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn parse_uri_endpoint(uri: &str) -> Option<(String, u16)> {
    let (scheme, rest) = uri.split_once("://")?;
    let authority = rest.split('/').next()?.rsplit('@').next()?.trim();
    if authority.is_empty() {
        return None;
    }

    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ipp" | "http" => Some(631),
        "ipps" | "https" => Some(443),
        "socket" => Some(9100),
        "lpd" => Some(515),
        _ => None,
    };

    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let port = authority
            .get(end + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .and_then(|port| port.parse::<u16>().ok())
            .or(default_port)?;
        return Some((host.to_ascii_lowercase(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse::<u16>().ok()),
        _ => (authority, default_port),
    };

    Some((host.to_ascii_lowercase(), port?))
}

fn uri_prefix(uri: &str) -> String {
    uri.split(['?', '#'])
        .next()
        .unwrap_or(uri)
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListPrintersReply {
    pub printers: Vec<PrinterEntry>,
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

#[derive(Debug, PartialEq, ReplyError, introspect::ReplyError)]
#[zlink(interface = "com.system76.CosmicSettings.Printers")]
pub enum Error {
    FailedToGetPrinters,
    CupsFailed,
    PrinterNotFound,
    NoDefaultPrinter,
}