use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, EndpointSource, PrinterApplication, PrinterApplicationScanState,
    PrinterApplicationState, PrinterEntry, PrintersEvent, PrintersEventKind,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::broadcast;

use crate::printer_application_backend::{
    AddPrinterDiscovery, DiscoveryGeneration, PaConfigurationCandidate, PaDriverMatch,
    PendingConfigurationState, PendingPaConfiguration, ResolveError, reconcile,
};

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
    ///
    /// Kept across rounds so a request that goes unanswered does not read as "no
    /// driver". An application's drivers do not change between two scans a moment
    /// apart; whether it replies does.
    driver_answers: HashMap<String, HashMap<String, RememberedAnswer>>,
    /// The printers each application says it already has, by device URI.
    ///
    /// Kept across rounds for the same reason as the driver answers: an
    /// application that does not answer has not stopped having them, and offering
    /// to set up a printer that is already set up is worse than saying nothing.
    configured_devices: HashMap<String, RememberedConfiguredDevices>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DnssdDeviceEndpoint {
    pub(crate) hostname: String,
    pub(crate) port: u16,
    pub(crate) address: Option<String>,
    pub(crate) is_local: bool,
}

impl DnssdDeviceEndpoint {
    fn apply_to(&self, printer: &mut PrinterEntry) {
        printer.set_option("dnssd-hostname", &self.hostname);
        printer.set_option("dnssd-port", self.port.to_string());
        printer.set_option("endpoint-is-local", self.is_local.to_string());
        if let Some(address) = &self.address {
            printer.set_option("endpoint-address", address);
        }
    }
}

/// How many Printer Applications to ask for devices at once.
///
/// Each scan makes an application rescan USB, SNMP, and DNS-SD, so running every
/// application at once would flood the local network and the USB bus while making
/// each individual scan slower.
const DEFAULT_SCAN_CONCURRENCY: usize = 4;

/// Locks created on demand, one per key.
///
/// The outer lock only guards the map; the inner one is what a caller waits on,
/// so holding a per-key lock across network work does not block anyone looking up
/// a different key.
type KeyedLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

#[derive(Clone, Debug)]
pub(crate) struct Context {
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

pub(crate) struct DiscoveryLease {
    running: Arc<AtomicBool>,
}

impl Drop for DiscoveryLease {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

impl Context {
    pub(crate) fn new() -> Self {
        Self::with_scan_concurrency(DEFAULT_SCAN_CONCURRENCY)
    }

    /// Creates a context with an explicit scan concurrency limit.
    ///
    /// Tests use a limit of one to make interleaving deterministic.
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

    pub(crate) async fn printer_applications_cached(&self) -> Vec<PrinterApplication> {
        let mut applications = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .printer_applications
            .values()
            .cloned()
            .collect::<Vec<_>>();
        applications.sort_by(|left, right| {
            left.service_name
                .cmp(&right.service_name)
                .then(left.id.cmp(&right.id))
        });
        applications
    }

    pub(crate) async fn available_destinations_cached(&self) -> Vec<PrinterEntry> {
        let mut destinations = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .available_destinations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        destinations.sort_by(|left, right| left.id().cmp(right.id()));
        destinations
    }

    pub(crate) fn update_available_destination(&self, mut incoming: PrinterEntry) {
        let id = incoming.id().to_string();
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_resolved_device_endpoint(&model.dnssd_device_endpoints, &mut incoming);
        let changed = model.available_destinations.get(&id) != Some(&incoming);
        if changed {
            model.available_destinations.insert(id, incoming);
        }
        drop(model);

        if changed {
            self.emit_available_destinations_changed();
            self.reconcile_after_destination_change();
        }
    }

    /// Merges a partial enumeration callback without discarding attributes
    /// added by an earlier enrichment pass.
    pub(crate) fn merge_available_destination(&self, mut incoming: PrinterEntry) {
        let id = incoming.id().to_string();
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_resolved_device_endpoint(&model.dnssd_device_endpoints, &mut incoming);
        let changed = if let Some(existing) = model.available_destinations.get_mut(&id) {
            let before = existing.clone();
            existing.merge_enumeration_record(incoming);
            *existing != before
        } else {
            model.available_destinations.insert(id, incoming);
            true
        };
        drop(model);

        if changed {
            self.emit_available_destinations_changed();
            self.reconcile_after_destination_change();
        }
    }

    /// Matches waiting configuration attempts once the destination cache is
    /// current, and emits separately from the destination event so a client can
    /// tell "a printer appeared" from "my printer finished being set up".
    fn reconcile_after_destination_change(&self) {
        if self.reconcile_pending_configurations() {
            self.emit_printer_configuration_changed();
        }
    }

    pub(crate) fn record_dnssd_device_endpoint(
        &self,
        service_name: String,
        endpoint: DnssdDeviceEndpoint,
    ) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        model
            .dnssd_device_endpoints
            .insert(service_name.clone(), endpoint.clone());

        let mut changed = false;
        for printer in model.available_destinations.values_mut() {
            if printer.endpoint_source() == Some(EndpointSource::Connected)
                || device_service_name(printer).as_deref() != Some(service_name.as_str())
            {
                continue;
            }
            let before = printer.clone();
            endpoint.apply_to(printer);
            changed |= *printer != before;
        }
        drop(model);

        if changed {
            self.emit_available_destinations_changed();
        }
    }

    pub(crate) fn remove_available_destination(&self, id: &str) {
        let changed = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .available_destinations
            .remove(id)
            .is_some();

        if changed {
            self.emit_available_destinations_changed();
        }
    }

    pub(crate) async fn merge_printer_application_discovery(
        &self,
        application: PrinterApplication,
    ) -> bool {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let application_id = application.id.clone();
        let inserted = !model.printer_applications.contains_key(&application_id);
        let mut restarted = false;
        let changed = if let Some(existing) = model.printer_applications.get_mut(&application_id) {
            let before = existing.clone();
            // A different endpoint means the application restarted, and a restarted
            // application may have been upgraded or had drivers added, so nothing it
            // said before can be relied on.
            restarted = existing.system_uri != application.system_uri;
            existing.merge_discovery_record(application);
            *existing != before
        } else {
            model
                .printer_applications
                .insert(application_id.clone(), application);
            true
        };
        if restarted {
            model.driver_answers.remove(&application_id);
            model.configured_devices.remove(&application_id);
        }
        drop(model);

        if changed {
            self.emit_printer_applications_changed();
        }
        inserted
    }

    pub(crate) async fn update_printer_application_probe(
        &self,
        application_id: &str,
        update: impl FnOnce(&mut PrinterApplication),
    ) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = if let Some(application) = model.printer_applications.get_mut(application_id)
        {
            let before = application.clone();
            update(application);
            *application != before
        } else {
            false
        };
        drop(model);

        if changed {
            self.emit_printer_applications_changed();
        }
    }

    pub(crate) async fn retain_printer_applications(&self, active_ids: &HashSet<String>) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = model
            .printer_applications
            .keys()
            .filter(|id| !active_ids.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        model
            .printer_applications
            .retain(|id, _| active_ids.contains(id));
        // An application that stopped being advertised takes its own Add Printer
        // results with it, and what it said about drivers with them. Every other
        // application's results stay: one application going away says nothing about
        // the others.
        let discovery_changed = removed.iter().fold(false, |changed, id| {
            model.driver_answers.remove(id);
            model.configured_devices.remove(id);
            model.add_printer_discovery.remove_application(id) || changed
        });
        drop(model);

        if !removed.is_empty() {
            self.emit_printer_applications_changed();
        }
        if discovery_changed {
            self.emit_add_printer_discovery_changed();
        }
    }

    pub(crate) fn subscribe_events(&self) -> broadcast::Receiver<PrintersEvent> {
        self.events.subscribe()
    }

    fn emit_printer_applications_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::PrinterApplicationsChanged,
        });
    }

    fn emit_available_destinations_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::AvailableDestinationsChanged,
        });
    }

    pub(crate) fn try_start_printer_application_discovery(&self) -> Option<DiscoveryLease> {
        self.discovery_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DiscoveryLease {
                running: Arc::clone(&self.discovery_running),
            })
    }

    pub(crate) fn try_start_available_destinations_refresh(&self) -> Option<DiscoveryLease> {
        self.available_destinations_refresh_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DiscoveryLease {
                running: Arc::clone(&self.available_destinations_refresh_running),
            })
    }

    /// Starts a new Add Printer discovery generation.
    ///
    /// The applications to ask are snapshotted here, under the model lock, so the
    /// round has a fixed membership even if discovery changes while it runs.
    pub(crate) fn start_add_printer_discovery(&self) -> DiscoveryGeneration {
        let mut model = self.locked_model();
        let applications = model
            .printer_applications
            .values()
            .filter(|application| is_worth_asking(application))
            .map(|application| (application.id.clone(), application_name(application)))
            .collect::<Vec<_>>();
        let generation = model.add_printer_discovery.start(applications);
        drop(model);

        self.emit_add_printer_discovery_changed();

        generation
    }

    /// Returns the printers this application last said it has, by device URI.
    ///
    /// An application is the only authority on which device each of its printers
    /// points at, so if it does not answer, what it said before is all there is —
    /// and it is far better than reporting a printer that is already set up as
    /// something to add. Replaced whole the moment it answers again, so a printer
    /// deleted there stops being remembered.
    pub(crate) fn remembered_configured_devices(
        &self,
        application_id: &str,
    ) -> HashMap<String, String> {
        let model = self.locked_model();

        model
            .configured_devices
            .get(application_id)
            .filter(|remembered| {
                std::time::Instant::now().duration_since(remembered.learned_at)
                    < CONFIGURED_DEVICE_MEMORY
            })
            .map(|remembered| remembered.by_device_uri.clone())
            .unwrap_or_default()
    }

    /// Records the printers this application says it has, by device URI.
    pub(crate) fn remember_configured_devices(
        &self,
        application_id: &str,
        by_device_uri: HashMap<String, String>,
    ) {
        self.locked_model().configured_devices.insert(
            application_id.to_string(),
            RememberedConfiguredDevices {
                by_device_uri,
                learned_at: std::time::Instant::now(),
            },
        );
    }

    /// Returns what this application recently answered about each device's drivers.
    ///
    /// Only used where a fresh request produced no answer at all. A fresh answer
    /// always wins, including one that contradicts what is remembered, so an answer
    /// that was wrong is corrected the moment the application replies again.
    pub(crate) fn remembered_driver_answers(
        &self,
        application_id: &str,
    ) -> HashMap<String, PaDriverMatch> {
        let model = self.locked_model();
        let now = std::time::Instant::now();

        model
            .driver_answers
            .get(application_id)
            .map(|answers| {
                answers
                    .iter()
                    .filter(|(_, answer)| {
                        now.duration_since(answer.learned_at) < DRIVER_ANSWER_MEMORY
                    })
                    .map(|(device_id, answer)| (device_id.clone(), answer.matched.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Records what this application answered about each device's drivers.
    ///
    /// Answers accumulate per device and a newer one replaces an older one, so a
    /// device that stops being supported stops being remembered as supported.
    pub(crate) fn remember_driver_answers(
        &self,
        application_id: &str,
        answers: HashMap<String, PaDriverMatch>,
    ) {
        if answers.is_empty() {
            return;
        }

        let learned_at = std::time::Instant::now();
        let mut model = self.locked_model();
        let remembered = model
            .driver_answers
            .entry(application_id.to_string())
            .or_default();
        for (device_id, matched) in answers {
            remembered.insert(
                device_id,
                RememberedAnswer {
                    matched,
                    learned_at,
                },
            );
        }
    }

    /// Returns the applications to ask in a generation, with their system URIs.
    pub(crate) fn add_printer_scan_targets(&self) -> Vec<PrinterApplication> {
        let model = self.locked_model();
        let wanted = model
            .add_printer_discovery
            .reply()
            .printer_application_scans
            .into_iter()
            .map(|status| status.printer_application_id)
            .collect::<Vec<_>>();

        wanted
            .into_iter()
            .filter_map(|id| model.printer_applications.get(&id).cloned())
            .collect()
    }

    /// Records that an application's scan has begun.
    pub(crate) fn mark_printer_application_searching(
        &self,
        generation: DiscoveryGeneration,
        application_id: &str,
    ) {
        let changed = self
            .locked_model()
            .add_printer_discovery
            .mark_searching(generation, application_id);

        if changed {
            self.emit_add_printer_discovery_changed();
        }
    }

    /// Replaces one application's results, emitting at most one event.
    pub(crate) fn replace_printer_application_snapshot(
        &self,
        generation: DiscoveryGeneration,
        application_id: &str,
        state: PrinterApplicationScanState,
        candidates: Vec<PaConfigurationCandidate>,
        quarantined: usize,
    ) {
        let changed = self.locked_model().add_printer_discovery.replace_snapshot(
            generation,
            application_id,
            state,
            candidates,
            quarantined,
        );

        if changed {
            self.emit_add_printer_discovery_changed();
        }
    }

    /// Returns the current Add Printer discovery state.
    pub(crate) fn add_printer_discovery_reply(&self) -> AddPrinterDiscoveryReply {
        self.locked_model().add_printer_discovery.reply()
    }

    /// Returns the current discovery generation.
    ///
    /// Cheaper than building a whole reply, which matters because a scan task
    /// checks it before writing results.
    pub(crate) fn add_printer_generation(&self) -> DiscoveryGeneration {
        self.locked_model().add_printer_discovery.generation()
    }

    /// Resolves a client's selection to the candidate the server recorded.
    ///
    /// The candidate is cloned out so the model lock is not held across the
    /// network work that follows.
    pub(crate) fn resolve_add_printer_candidate(
        &self,
        generation: DiscoveryGeneration,
        physical_printer_id: &str,
        candidate_id: &str,
    ) -> Result<PaConfigurationCandidate, ResolveError> {
        self.locked_model()
            .add_printer_discovery
            .resolve(generation, physical_printer_id, candidate_id)
            .cloned()
    }

    /// Returns whether a candidate is still reported by its application.
    pub(crate) fn add_printer_candidate_is_current(&self, candidate_id: &str) -> bool {
        self.locked_model()
            .add_printer_discovery
            .candidate_is_current(candidate_id)
    }

    /// Acquires a slot to run a device scan in.
    pub(crate) async fn acquire_scan_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(&self.pa_scan_semaphore)
            .acquire_owned()
            .await
            .expect("scan semaphore is never closed")
    }

    /// Returns the lock that serializes scans of one Printer Application.
    pub(crate) fn scan_lock(&self, application_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        keyed_lock(&self.pa_scan_locks, application_id)
    }

    /// Returns the lock that serializes configuration of one device through one
    /// Printer Application.
    ///
    /// Keyed on both, because the same device reached through two applications is
    /// two independent configurations, while the same device through one
    /// application must not be configured twice at once.
    pub(crate) fn configuration_lock(
        &self,
        application_id: &str,
        device_uri: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        keyed_lock(
            &self.pa_configuration_locks,
            &format!("{application_id}\u{1}{device_uri}"),
        )
    }

    /// Records a configuration attempt.
    pub(crate) fn insert_pending_configuration(&self, pending: PendingPaConfiguration) {
        self.locked_model()
            .pending_pa_configurations
            .insert(pending.operation_id.clone(), pending);
        self.emit_printer_configuration_changed();
    }

    /// Returns a configuration attempt.
    pub(crate) fn pending_configuration(
        &self,
        operation_id: &str,
    ) -> Option<PendingPaConfiguration> {
        self.locked_model()
            .pending_pa_configurations
            .get(operation_id)
            .cloned()
    }

    /// Matches waiting configuration attempts against the destinations now known.
    ///
    /// Called after the destination cache has been updated, never before: the
    /// destination pipeline owns every `PrinterEntry`, and this only records which
    /// one an attempt turned into.
    fn reconcile_pending_configurations(&self) -> bool {
        let mut model = self.locked_model();
        if model.pending_pa_configurations.is_empty() {
            return false;
        }

        let destinations = model
            .available_destinations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut changed = false;

        for pending in model.pending_pa_configurations.values_mut() {
            if !pending.state.is_awaiting() {
                continue;
            }

            match reconcile::find_destination(pending, destinations.iter()) {
                Some(destination) => {
                    tracing::debug!(
                        destination_id = destination.id(),
                        attempt = reconcile::describe(pending),
                        "printer configuration reconciled to a destination"
                    );
                    pending.state = PendingConfigurationState::Reconciled {
                        destination_id: destination.id().to_string(),
                    };
                    changed = true;
                }
                // A printer that was created but never advertised would otherwise
                // leave the attempt waiting forever.
                None if pending.created_at.elapsed() >= reconcile::ADVERTISEMENT_TIMEOUT => {
                    tracing::warn!(
                        attempt = reconcile::describe(pending),
                        "printer was created but never advertised; setup needs finishing by hand"
                    );
                    pending.state = PendingConfigurationState::ManualActionRequired;
                    changed = true;
                }
                None => {}
            }
        }

        changed
    }

    fn locked_model(&self) -> std::sync::MutexGuard<'_, Model> {
        self.model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn emit_add_printer_discovery_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::AddPrinterDiscoveryChanged,
        });
    }

    fn emit_printer_configuration_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::PrinterConfigurationChanged,
        });
    }
}

/// Returns a lock for a key, creating it on first use.
fn keyed_lock(locks: &KeyedLocks, key: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    Arc::clone(locks.entry(key.to_string()).or_default())
}

/// How long an answer about a device's drivers stands in for a missing one.
///
/// Long enough to cover a refresh made straight away, which is when the same
/// printer flipping between "ready" and "no driver" is glaring. Short enough that
/// an answer which was wrong, or has stopped being true, cannot go on being
/// repeated: after this, silence is reported as silence.
const DRIVER_ANSWER_MEMORY: std::time::Duration = std::time::Duration::from_secs(600);

/// What an application answered about one device, and when.
#[derive(Clone, Debug)]
struct RememberedAnswer {
    matched: PaDriverMatch,
    learned_at: std::time::Instant,
}

/// How long an application's list of its own printers stands in for a missing one.
///
/// Longer than the driver memory, because it is a stronger claim: an application
/// having a printer for a device is a fact about its own configuration, which does
/// not change unless somebody changes it, and a printer wrongly offered as new is
/// worse than one wrongly withheld.
const CONFIGURED_DEVICE_MEMORY: std::time::Duration = std::time::Duration::from_secs(3600);

/// The printers one application said it has, and when it said so.
#[derive(Clone, Debug)]
struct RememberedConfiguredDevices {
    /// Printer name by the device URI it was created for.
    by_device_uri: HashMap<String, String>,
    learned_at: std::time::Instant,
}

/// Returns whether a Printer Application belongs in a discovery round.
///
/// An application that has not finished being probed is included rather than
/// judged on capabilities it has not reported yet, and rather than on an address
/// list that DNS-SD has not filled in — a service that re-announces itself is
/// briefly indistinguishable from a brand new one, and dropping it from the round
/// for that showed a printer as having no application that could drive it. The scan
/// probes it and decides then.
fn is_worth_asking(application: &PrinterApplication) -> bool {
    application.capabilities.find_devices
        || application.is_local()
        || matches!(
            application.state,
            PrinterApplicationState::Discovered | PrinterApplicationState::Probing
        )
}

/// Returns the name to show for a Printer Application.
fn application_name(application: &PrinterApplication) -> String {
    application
        .make_and_model
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| application.service_name.clone())
}

fn apply_resolved_device_endpoint(
    endpoints: &HashMap<String, DnssdDeviceEndpoint>,
    printer: &mut PrinterEntry,
) {
    if printer.endpoint_source() == Some(EndpointSource::Connected) {
        return;
    }
    if let Some(endpoint) = device_service_name(printer).and_then(|name| endpoints.get(&name)) {
        endpoint.apply_to(printer);
    }
}

fn device_service_name(printer: &PrinterEntry) -> Option<String> {
    let uri = url::Url::parse(printer.device_uri()?).ok()?;
    Some(
        uri.host_str()?
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::{EndpointSource, PrinterApplicationState};
    use std::collections::BTreeMap;

    fn application(id: &str) -> PrinterApplication {
        PrinterApplication {
            id: id.into(),
            service_name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: "printer.local".into(),
            port: 8000,
            addresses: vec!["192.0.2.1".into()],
            system_uri: "ipps://printer.local:8000/ipp/system".into(),
            make_and_model: None,
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: cosmic_settings_printers_core::PrinterApplicationCapabilities::default(),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Discovered,
        }
    }

    fn destination(id: &str, location: &str) -> PrinterEntry {
        PrinterEntry::new(
            id,
            id,
            false,
            HashMap::from([("printer-location".to_string(), location.to_string())]),
        )
    }

    fn dnssd_destination(id: &str) -> PrinterEntry {
        let mut printer = destination(id, "");
        printer.set_option("device-uri", format!("ipps://{id}._ipps._tcp.local/"));
        printer
    }

    fn resolved_endpoint() -> DnssdDeviceEndpoint {
        DnssdDeviceEndpoint {
            hostname: "desktop.local".into(),
            port: 8000,
            address: Some("192.0.2.1".into()),
            is_local: true,
        }
    }

    #[tokio::test]
    async fn destination_changes_update_cache_and_emit_once() {
        let context = Context::new();
        let mut events = context.subscribe_events();

        context.update_available_destination(destination("office", "first floor"));
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::AvailableDestinationsChanged
        );

        context.update_available_destination(destination("office", "first floor"));
        assert!(events.try_recv().is_err());

        context.update_available_destination(destination("office", "second floor"));
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::AvailableDestinationsChanged
        );
        assert_eq!(
            context.available_destinations_cached().await[0].location(),
            Some("second floor")
        );

        context.remove_available_destination("office");
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::AvailableDestinationsChanged
        );
        assert!(context.available_destinations_cached().await.is_empty());
    }

    #[tokio::test]
    async fn partial_destination_update_preserves_enriched_options() {
        let context = Context::new();
        let mut enriched = destination("office", "first floor");
        enriched.set_option("endpoint-hostname", "printer.local");
        enriched.set_option("endpoint-port", "8000");
        enriched.set_endpoint_source(EndpointSource::Connected);
        context.update_available_destination(enriched);

        let mut partial = destination("office", "second floor");
        partial.set_option("endpoint-hostname", "printer._ipps._tcp.local");
        partial.set_option("endpoint-port", "631");
        partial.set_endpoint_source(EndpointSource::Uri);
        context.merge_available_destination(partial);

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].location(), Some("second floor"));
        assert_eq!(cached[0].hostname(), Some("printer.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), None);
        assert_eq!(cached[0].endpoint_source(), Some(EndpointSource::Connected));
    }

    #[tokio::test]
    async fn dnssd_endpoint_is_applied_when_resolution_arrives_first() {
        let context = Context::new();
        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );

        context.merge_available_destination(dnssd_destination("SocketLabel"));

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), Some("192.0.2.1"));
        assert_eq!(cached[0].option("endpoint-is-local"), Some("true"));
    }

    #[tokio::test]
    async fn dnssd_endpoint_is_applied_when_destination_arrives_first() {
        let context = Context::new();
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), Some("192.0.2.1"));
        assert_eq!(cached[0].option("endpoint-is-local"), Some("true"));
    }

    #[tokio::test]
    async fn later_destination_update_keeps_resolved_dnssd_endpoint() {
        let context = Context::new();
        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.update_available_destination(dnssd_destination("SocketLabel"));

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
    }

    #[tokio::test]
    async fn printer_applications_use_a_separate_cache_and_event() {
        let context = Context::new();
        let mut events = context.subscribe_events();

        assert!(
            context
                .merge_printer_application_discovery(application("app"))
                .await
        );
        assert_eq!(context.printer_applications_cached().await.len(), 1);
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::PrinterApplicationsChanged
        );

        context.retain_printer_applications(&HashSet::new()).await;
        assert!(context.printer_applications_cached().await.is_empty());
    }

    #[tokio::test]
    async fn repeated_application_discovery_merges_without_requesting_another_probe() {
        let context = Context::new();
        let mut first = application("app");
        first.addresses = vec!["192.0.2.1".into()];
        let mut repeated = application("app");
        repeated.addresses = vec!["2001:db8::1".into()];

        assert!(context.merge_printer_application_discovery(first).await);
        assert!(!context.merge_printer_application_discovery(repeated).await);

        let applications = context.printer_applications_cached().await;
        assert_eq!(
            applications[0].addresses,
            vec!["192.0.2.1".to_string(), "2001:db8::1".to_string()]
        );
    }

    #[tokio::test]
    async fn retaining_applications_removes_inactive_entries() {
        let context = Context::new();
        context
            .merge_printer_application_discovery(application("keep"))
            .await;
        context
            .merge_printer_application_discovery(application("remove"))
            .await;

        context
            .retain_printer_applications(&HashSet::from(["keep".to_string()]))
            .await;

        let applications = context.printer_applications_cached().await;
        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].id, "keep");
    }

    #[test]
    fn discovery_lease_releases_on_drop() {
        let context = Context::new();
        let lease = context.try_start_printer_application_discovery().unwrap();
        assert!(context.try_start_printer_application_discovery().is_none());

        drop(lease);

        assert!(context.try_start_printer_application_discovery().is_some());
    }

    #[test]
    fn destination_refresh_lease_prevents_parallel_enumerations() {
        let context = Context::new();
        let lease = context.try_start_available_destinations_refresh().unwrap();
        assert!(context.try_start_available_destinations_refresh().is_none());

        drop(lease);

        assert!(context.try_start_available_destinations_refresh().is_some());
    }
}
