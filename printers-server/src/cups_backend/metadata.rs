use cosmic_config::{ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use cosmic_settings_printers_core::PrinterEntry;

use super::helpers::split_queue_instance;
use crate::error::{BackendError, BackendResult};

const CONFIG_ID: &str = "com.system76.CosmicSettings.Printers";
const CONFIG_VERSION: u64 = 1;
const METADATA_KEY: &str = "queue_metadata";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct QueueMetadata {
    pub discovered_printer: PrinterEntry,
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

    fn remove(&self, queue_name: &str) -> BackendResult<()> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        entries.remove(queue_name);
        self.write_unlocked(entries)
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

pub(super) fn remove(queue_name: &str) -> BackendResult<()> {
    store()?.remove(queue_name)
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
