use crate::avahi::discovered_printer_id;
use cosmic_settings_printers_core::{
    PrinterApplication, PrinterEntry, PrintersEvent, PrintersEventKind,
};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, broadcast};

#[derive(Debug, Default)]
struct Model {
    discovered_printers: Vec<PrinterEntry>,
    printer_applications: HashMap<String, PrinterApplication>,
}

#[derive(Clone, Debug)]
pub(crate) struct Context {
    model: Arc<Mutex<Model>>,
    discovery_running: Arc<AtomicBool>,
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
            events,
        }
    }

    pub(crate) async fn discovered_printers_cached(&self) -> Vec<PrinterEntry> {
        self.model.lock().await.discovered_printers.clone()
    }

    pub(crate) async fn printer_applications_cached(&self) -> Vec<PrinterApplication> {
        let mut applications = self
            .model
            .lock()
            .await
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

    pub(crate) async fn merge_printer_application_discovery(
        &self,
        application: PrinterApplication,
    ) -> bool {
        let mut model = self.model.lock().await;
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
        let mut model = self.model.lock().await;
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
        let mut model = self.model.lock().await;
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

    pub(crate) async fn discovered_printer(&self, printer_id: &str) -> Option<PrinterEntry> {
        self.model
            .lock()
            .await
            .discovered_printers
            .iter()
            .find(|printer| discovered_printer_id(printer).as_deref() == Some(printer_id))
            .cloned()
    }

    pub(crate) async fn update_discovered_printers(
        &self,
        update: impl FnOnce(&mut Vec<PrinterEntry>),
    ) {
        let mut model = self.model.lock().await;
        update(&mut model.discovered_printers);
        model.discovered_printers.sort_by(|left, right| {
            left.name()
                .cmp(right.name())
                .then(left.id().cmp(right.id()))
        });
    }

    pub(crate) fn subscribe_events(&self) -> broadcast::Receiver<PrintersEvent> {
        self.events.subscribe()
    }

    pub(crate) async fn merge_discovered_printer_by(
        &self,
        printer: PrinterEntry,
        matches: impl Fn(&PrinterEntry, &PrinterEntry) -> bool,
    ) {
        let mut added = false;
        self.update_discovered_printers(|printers| {
            if let Some(index) = printers
                .iter()
                .position(|existing| matches(existing, &printer))
            {
                printers[index].merge_discovery_record(printer);
            } else {
                printers.push(printer);
                added = true;
            }
        })
        .await;

        if added {
            self.emit_discovered_printers_changed();
        }
    }

    pub(crate) async fn merge_discovered_printers_by(
        &self,
        incoming: impl IntoIterator<Item = PrinterEntry>,
        matches: impl Fn(&PrinterEntry, &PrinterEntry) -> bool,
    ) {
        let mut added = false;
        self.update_discovered_printers(|printers| {
            for printer in incoming {
                if let Some(index) = printers
                    .iter()
                    .position(|existing| matches(existing, &printer))
                {
                    printers[index].merge_discovery_record(printer);
                } else {
                    printers.push(printer);
                    added = true;
                }
            }
        })
        .await;

        if added {
            self.emit_discovered_printers_changed();
        }
    }

    fn emit_discovered_printers_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::DiscoveredPrintersChanged,
        });
    }

    fn emit_printer_applications_changed(&self) {
        let _ = self.events.send(PrintersEvent {
            kind: PrintersEventKind::PrinterApplicationsChanged,
        });
    }

    pub(crate) async fn retain_discovered_printers_by(
        &self,
        incoming: impl IntoIterator<Item = PrinterEntry>,
        matches: impl Fn(&PrinterEntry, &PrinterEntry) -> bool,
    ) {
        let incoming = incoming.into_iter().collect::<Vec<_>>();
        self.update_discovered_printers(|printers| {
            printers.retain(|printer| incoming.iter().any(|other| matches(printer, other)));
        })
        .await;
    }

    pub(crate) fn try_start_discovery(&self) -> Option<DiscoveryLease> {
        self.discovery_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DiscoveryLease {
                running: Arc::clone(&self.discovery_running),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::PrinterApplicationState;
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

    #[tokio::test]
    async fn printer_applications_use_a_separate_cache_and_event() {
        let context = Context::new();
        let mut events = context.subscribe_events();

        assert!(
            context
                .merge_printer_application_discovery(application("app"))
                .await
        );
        assert!(context.discovered_printers_cached().await.is_empty());
        assert_eq!(context.printer_applications_cached().await.len(), 1);
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::PrinterApplicationsChanged
        );

        context.retain_printer_applications(&HashSet::new()).await;
        assert!(context.printer_applications_cached().await.is_empty());
        assert!(context.discovered_printers_cached().await.is_empty());
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
    async fn retaining_applications_does_not_change_discovered_printers() {
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
        assert!(context.discovered_printers_cached().await.is_empty());
    }

    #[test]
    fn discovery_lease_releases_on_drop() {
        let context = Context::new();
        let lease = context.try_start_discovery().unwrap();
        assert!(context.try_start_discovery().is_none());

        drop(lease);

        assert!(context.try_start_discovery().is_some());
    }
}
