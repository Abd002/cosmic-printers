use crate::{avahi::discovered_printer_id, backend::Model};
use cosmic_settings_printers_core::PrinterEntry;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Context {
    model: Arc<Mutex<Model>>,
}

impl Context {
    pub async fn new() -> Self {
        Self {
            model: Arc::new(Mutex::new(Model::new())),
        }
    }

    pub async fn discovered_printers(&self) -> Vec<PrinterEntry> {
        self.model.lock().await.discovered_printers.clone()
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
        self.update_discovered_printers(|printers| {
            if let Some(index) = printers
                .iter()
                .position(|existing| matches(existing, &printer))
            {
                printers[index].merge_from(printer);
            } else {
                printers.push(printer);
            }
        })
        .await;
    }

    pub async fn merge_discovered_printers_by(
        &self,
        incoming: impl IntoIterator<Item = PrinterEntry>,
        matches: impl Fn(&PrinterEntry, &PrinterEntry) -> bool,
    ) {
        self.update_discovered_printers(|printers| {
            for printer in incoming {
                if let Some(index) = printers
                    .iter()
                    .position(|existing| matches(existing, &printer))
                {
                    printers[index].merge_from(printer);
                } else {
                    printers.push(printer);
                }
            }
        })
        .await;
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
