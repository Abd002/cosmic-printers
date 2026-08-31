//! Shared daemon state protected by one mutex for atomic cross-collection updates.

mod applications;
mod configuration;
mod discovery;
mod endpoints;
mod events;
mod leases;
mod memory;
mod printers;

use cosmic_settings_printers_core::{PrinterApplication, PrinterEntry, PrintersEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use tokio::sync::broadcast;

pub(crate) use endpoints::DnssdDeviceEndpoint;

use leases::{DEFAULT_SCAN_CONCURRENCY, KeyedLocks};
use memory::{RememberedAnswer, RememberedApplicationPrinters, RememberedConfiguredDevices};

use crate::printer_app::{AddPrinterDiscovery, PendingPaConfiguration};

#[derive(Debug, Default)]
struct Model {
    available_destinations: HashMap<String, PrinterEntry>,
    printer_applications: HashMap<String, PrinterApplication>,
    dnssd_device_endpoints: HashMap<String, DnssdDeviceEndpoint>,
    add_printer_discovery: AddPrinterDiscovery,
    /// Printers a Printer Application was asked to create, until the destination
    /// pipeline advertises them.
    pending_pa_configurations: HashMap<String, PendingPaConfiguration>,
    /// What each application last *answered* about a device's drivers, by
    /// application identifier and then device ID.
    driver_answers: HashMap<String, HashMap<String, RememberedAnswer>>,
    /// The printers each application says it already has, by device URI.
    configured_devices: HashMap<String, RememberedConfiguredDevices>,
    /// Each application's printers, used to route destinations to their owner.
    application_printers: HashMap<String, RememberedApplicationPrinters>,
    /// Consecutive enumeration misses; one timed pass is insufficient evidence of removal.
    enumeration_misses: HashMap<String, u8>,
}

#[derive(Clone, Debug)]
pub(crate) struct State {
    model: Arc<Mutex<Model>>,
    discovery_running: Arc<AtomicBool>,
    available_destinations_refresh_running: Arc<AtomicBool>,
    /// Bounds how many device scans run at once.
    pa_scan_semaphore: Arc<tokio::sync::Semaphore>,
    /// One lock per Printer Application, so two rounds cannot ask the same
    /// application for devices simultaneously.
    pa_scan_locks: KeyedLocks,
    /// One lock per application and device, so two clients cannot both create a
    /// printer for the same device.
    pa_configuration_locks: KeyedLocks,
    events: broadcast::Sender<PrintersEvent>,
}

impl State {
    pub(crate) fn new() -> Self {
        Self::with_scan_concurrency(DEFAULT_SCAN_CONCURRENCY)
    }

    /// Creates a context with an explicit scan concurrency limit.
    pub(crate) fn with_scan_concurrency(scan_concurrency: usize) -> Self {
        let (events, _) = broadcast::channel(32);
        Self {
            model: Arc::new(Mutex::new(Model::default())),
            discovery_running: Arc::new(AtomicBool::new(false)),
            available_destinations_refresh_running: Arc::new(AtomicBool::new(false)),
            pa_scan_semaphore: Arc::new(tokio::sync::Semaphore::new(scan_concurrency.max(1))),
            pa_scan_locks: Arc::new(Mutex::new(HashMap::new())),
            pa_configuration_locks: Arc::new(Mutex::new(HashMap::new())),
            events,
        }
    }

    fn locked_model(&self) -> std::sync::MutexGuard<'_, Model> {
        self.model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
