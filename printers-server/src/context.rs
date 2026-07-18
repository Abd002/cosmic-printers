use crate::{avahi::discovered_printer_id, backend::Model};
use cosmic_settings_printers_core::{
    PrinterApplication, PrinterEntry, PrintersEvent, PrintersEventKind,
};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

#[derive(Clone, Debug)]
pub struct Context {
    model: Arc<Mutex<Model>>,
    events: broadcast::Sender<PrintersEvent>,
}

impl Context {
    pub async fn new() -> Self {
        let (events, _) = broadcast::channel(32);
        Self {
            model: Arc::new(Mutex::new(Model::new())),
            events,
        }
    }

    pub async fn discovered_printers(&self) -> Vec<PrinterEntry> {
        self.model.lock().await.discovered_printers.clone()
    }

    pub async fn list_printer_applications(&self) -> Vec<PrinterApplication> {
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

    pub async fn upsert_printer_application(&self, application: PrinterApplication) -> bool {
        let mut model = self.model.lock().await;
        let inserted = !model.printer_applications.contains_key(&application.id);
        let changed = if let Some(existing) = model.printer_applications.get_mut(&application.id) {
            let before = existing.clone();
            existing.merge_from(application);
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

    pub async fn update_printer_application(
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

    pub async fn remove_printer_application(&self, application_id: &str) {
        let removed = self
            .model
            .lock()
            .await
            .printer_applications
            .remove(application_id)
            .is_some();
        if removed {
            self.emit_printer_applications_changed();
        }
    }

    pub async fn retain_printer_applications(&self, active_ids: &HashSet<String>) {
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

    pub async fn discovered_printer(&self, printer_id: &str) -> Option<PrinterEntry> {
        self.model
            .lock()
            .await
            .discovered_printers
            .iter()
            .find(|printer| discovered_printer_id(printer).as_deref() == Some(printer_id))
            .cloned()
    }

    pub async fn printers(&self) -> Vec<PrinterEntry> {
        self.model.lock().await.printers.clone()
    }

    pub async fn printer(&self, printer_id: &str) -> Option<PrinterEntry> {
        self.model
            .lock()
            .await
            .printers
            .iter()
            .find(|printer| printer.id == printer_id)
            .cloned()
    }

    pub async fn set_printers(&self, printers: Vec<PrinterEntry>) {
        self.model.lock().await.printers = printers;
    }

    pub async fn update_discovered_printers(&self, update: impl FnOnce(&mut Vec<PrinterEntry>)) {
        let mut model = self.model.lock().await;
        update(&mut model.discovered_printers);
        model
            .discovered_printers
            .sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<PrintersEvent> {
        self.events.subscribe()
    }

    pub async fn update_discovered_printer(
        &self,
        printer_id: &str,
        update: impl FnOnce(&mut PrinterEntry),
    ) {
        self.update_discovered_printers(|printers| {
            if let Some(printer) = printers
                .iter_mut()
                .find(|printer| discovered_printer_id(printer).as_deref() == Some(printer_id))
            {
                update(printer);
            }
        })
        .await;
    }

    pub async fn merge_discovered_printer_by(
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
                printers[index].merge_from(printer);
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

    pub async fn merge_discovered_printers_by(
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
                    printers[index].merge_from(printer);
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

    pub async fn retain_discovered_printers_by(
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

    pub async fn start_auto_add(&self, printer_id: String) -> bool {
        self.model
            .lock()
            .await
            .auto_add_in_progress
            .insert(printer_id)
    }

    pub async fn finish_auto_add(&self, printer_id: &str) {
        self.model
            .lock()
            .await
            .auto_add_in_progress
            .remove(printer_id);
    }

    pub async fn start_discovery_if_idle(&self) -> bool {
        let mut model = self.model.lock().await;
        if model.discovery_running {
            false
        } else {
            model.discovery_running = true;
            true
        }
    }

    pub async fn finish_discovery(&self) {
        self.model.lock().await.discovery_running = false;
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
        let context = Context::new().await;
        let mut events = context.subscribe_events();

        assert!(context.upsert_printer_application(application("app")).await);
        assert!(context.discovered_printers().await.is_empty());
        assert_eq!(context.list_printer_applications().await.len(), 1);
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::PrinterApplicationsChanged
        );

        context.remove_printer_application("app").await;
        assert!(context.list_printer_applications().await.is_empty());
        assert!(context.discovered_printers().await.is_empty());
    }

    #[tokio::test]
    async fn retaining_applications_does_not_change_discovered_printers() {
        let context = Context::new().await;
        context
            .upsert_printer_application(application("keep"))
            .await;
        context
            .upsert_printer_application(application("remove"))
            .await;

        context
            .retain_printer_applications(&HashSet::from(["keep".to_string()]))
            .await;

        let applications = context.list_printer_applications().await;
        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].id, "keep");
        assert!(context.discovered_printers().await.is_empty());
    }
}
