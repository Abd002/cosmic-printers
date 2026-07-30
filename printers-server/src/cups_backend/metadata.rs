use cosmic_config::{ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use cosmic_settings_printers_core::PrinterEntry;

use super::helpers::split_queue_instance;
use crate::avahi::discovered_printer_id;
use crate::error::{BackendError, BackendResult};

const CONFIG_ID: &str = "com.system76.CosmicSettings.Printers";
const CONFIG_VERSION: u64 = 1;
const METADATA_KEY: &str = "queue_metadata";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct QueueMetadata {
    pub discovered_printer: PrinterEntry,
}

impl QueueMetadata {
    pub(super) fn from_discovered_printer(printer: &PrinterEntry) -> Self {
        Self {
            discovered_printer: printer.clone(),
        }
    }
}

type MetadataMap = HashMap<String, QueueMetadata>;

struct MetadataStore {
    config: cosmic_config::Config,
    write_lock: Mutex<()>,
}

impl MetadataStore {
    fn new() -> BackendResult<Self> {
        Ok(Self {
            config: config()?,
            write_lock: Mutex::new(()),
        })
    }

    fn lock(&self) -> BackendResult<std::sync::MutexGuard<'_, ()>> {
        self.write_lock
            .lock()
            .map_err(|_| BackendError::Internal("metadata store lock was poisoned".to_string()))
    }

    fn load_unlocked(&self) -> BackendResult<MetadataMap> {
        load_from(&self.config)
    }

    fn write_unlocked(&self, entries: MetadataMap) -> BackendResult<()> {
        self.config
            .set(METADATA_KEY, entries)
            .map_err(BackendError::Config)
    }

    fn save(&self, queue_name: &str, metadata: QueueMetadata) -> BackendResult<()> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        entries.insert(queue_name.to_string(), metadata);
        self.write_unlocked(entries)
    }

    fn remove(&self, queue_name: &str) -> BackendResult<()> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        entries.remove(queue_name);
        self.write_unlocked(entries)
    }

    fn contains_discovered_printer_id(&self, printer_id: &str) -> BackendResult<bool> {
        let _lock = self.lock()?;
        let entries = self.load_unlocked()?;

        Ok(entries.values().any(|metadata| {
            discovered_printer_id(&metadata.discovered_printer).as_deref() == Some(printer_id)
        }))
    }

    fn refresh_discovered_printer(
        &self,
        printer_id: &str,
        printer: &PrinterEntry,
    ) -> BackendResult<()> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;

        if let Some(metadata) = entries.values_mut().find(|metadata| {
            discovered_printer_id(&metadata.discovered_printer).as_deref() == Some(printer_id)
        }) {
            *metadata = QueueMetadata::from_discovered_printer(printer);
            self.write_unlocked(entries)?;
        }

        Ok(())
    }

    fn stale_discovered_queue_names(
        &self,
        active_printer_ids: &HashSet<String>,
    ) -> BackendResult<Vec<String>> {
        let _lock = self.lock()?;
        let entries = self.load_unlocked()?;

        Ok(entries
            .into_iter()
            .filter_map(|(queue_name, metadata)| {
                let printer_id = discovered_printer_id(&metadata.discovered_printer)?;
                (!active_printer_ids.contains(&printer_id)).then_some(queue_name)
            })
            .collect())
    }

    fn retain_for_configured_queues<'a>(
        &self,
        queue_names: impl IntoIterator<Item = &'a str>,
    ) -> BackendResult<()> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        let queue_names = queue_names.into_iter().collect::<HashSet<_>>();
        entries.retain(|queue_name, _| queue_names.contains(queue_name.as_str()));
        self.write_unlocked(entries)
    }

    fn apply(&self, printers: &mut HashMap<String, PrinterEntry>) -> BackendResult<()> {
        let _lock = self.lock()?;
        let entries = self.load_unlocked()?;

        for printer in printers.values_mut() {
            let (queue_name, _) = split_queue_instance(printer.id());
            let Some(metadata) = entries.get(queue_name) else {
                continue;
            };

            printer.apply_discovery_metadata(&metadata.discovered_printer);
        }

        Ok(())
    }
}

fn store() -> BackendResult<&'static MetadataStore> {
    static STORE: OnceLock<MetadataStore> = OnceLock::new();
    if let Some(store) = STORE.get() {
        return Ok(store);
    }

    let candidate = MetadataStore::new()?;
    let _ = STORE.set(candidate);
    STORE
        .get()
        .ok_or_else(|| BackendError::Internal("metadata store initialization failed".to_string()))
}

pub(super) fn save(queue_name: &str, metadata: QueueMetadata) -> BackendResult<()> {
    store()?.save(queue_name, metadata)
}

pub(super) fn remove(queue_name: &str) -> BackendResult<()> {
    store()?.remove(queue_name)
}

pub(super) fn contains_discovered_printer_id(printer_id: &str) -> BackendResult<bool> {
    store()?.contains_discovered_printer_id(printer_id)
}

pub(super) fn refresh_discovered_printer(
    printer_id: &str,
    printer: &PrinterEntry,
) -> BackendResult<()> {
    store()?.refresh_discovered_printer(printer_id, printer)
}

pub(super) fn stale_discovered_queue_names(
    active_printer_ids: &HashSet<String>,
) -> BackendResult<Vec<String>> {
    store()?.stale_discovered_queue_names(active_printer_ids)
}

pub(super) fn retain_for_configured_queues<'a>(
    queue_names: impl IntoIterator<Item = &'a str>,
) -> BackendResult<()> {
    store()?.retain_for_configured_queues(queue_names)
}

pub(super) fn apply(printers: &mut HashMap<String, PrinterEntry>) -> BackendResult<()> {
    store()?.apply(printers)
}

fn config() -> BackendResult<cosmic_config::Config> {
    cosmic_config::Config::new_state(CONFIG_ID, CONFIG_VERSION).map_err(BackendError::Config)
}

fn load_from(config: &cosmic_config::Config) -> BackendResult<MetadataMap> {
    match config.get(METADATA_KEY) {
        Ok(entries) => Ok(entries),
        Err(cosmic_config::Error::NotFound) => Ok(HashMap::new()),
        Err(error) => Err(BackendError::Config(error)),
    }
}
