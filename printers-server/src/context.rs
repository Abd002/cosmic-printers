use cosmic_settings_printers_core::{
    EndpointSource, PrinterApplication, PrinterEntry, PrintersEvent, PrintersEventKind,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::broadcast;

#[derive(Debug, Default)]
struct Model {
    available_destinations: HashMap<String, PrinterEntry>,
    printer_applications: HashMap<String, PrinterApplication>,
    dnssd_device_endpoints: HashMap<String, DnssdDeviceEndpoint>,
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

#[derive(Clone, Debug)]
pub(crate) struct Context {
    model: Arc<Mutex<Model>>,
    discovery_running: Arc<AtomicBool>,
    available_destinations_refresh_running: Arc<AtomicBool>,
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
        let (events, _) = broadcast::channel(32);
        Self {
            model: Arc::new(Mutex::new(Model::default())),
            discovery_running: Arc::new(AtomicBool::new(false)),
            available_destinations_refresh_running: Arc::new(AtomicBool::new(false)),
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
        let inserted = !model.printer_applications.contains_key(&application.id);
        let changed = if let Some(existing) = model.printer_applications.get_mut(&application.id) {
            let before = existing.clone();
            existing.merge_discovery_record(application);
            *existing != before
        } else {
            model
                .printer_applications
                .insert(application.id.clone(), application);
            true
        };
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
        let previous_len = model.printer_applications.len();
        model
            .printer_applications
            .retain(|id, _| active_ids.contains(id));
        let changed = model.printer_applications.len() != previous_len;
        drop(model);

        if changed {
            self.emit_printer_applications_changed();
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
            system_uuid: None,
            make_and_model: None,
            operations_supported: Vec::new(),
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
