use cosmic_config::{ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use cosmic_settings_printers_core::{Error, PrinterEntry};

use super::helpers::split_queue_instance;
use crate::avahi::discovered_printer_id;

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
    fn new() -> Result<Self, Error> {
        Ok(Self {
            config: config()?,
            write_lock: Mutex::new(()),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, Error> {
        self.write_lock.lock().map_err(|_| Error::Internal {
            why: "metadata store lock was poisoned".to_string(),
        })
    }

    fn load_unlocked(&self) -> Result<MetadataMap, Error> {
        load_from(&self.config)
    }

    fn write_unlocked(&self, entries: MetadataMap) -> Result<(), Error> {
        self.config
            .set(METADATA_KEY, entries)
            .map_err(|error| Error::ConfigFailed {
                why: error.to_string(),
            })
    }

    fn save(&self, queue_name: &str, metadata: QueueMetadata) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        entries.insert(queue_name.to_string(), metadata);
        self.write_unlocked(entries)
    }

    fn remove(&self, queue_name: &str) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        entries.remove(queue_name);
        self.write_unlocked(entries)
    }

    fn contains_discovered_printer_id(&self, printer_id: &str) -> Result<bool, Error> {
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
    ) -> Result<(), Error> {
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
    ) -> Result<Vec<String>, Error> {
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
    ) -> Result<(), Error> {
        let _lock = self.lock()?;
        let mut entries = self.load_unlocked()?;
        let queue_names = queue_names.into_iter().collect::<HashSet<_>>();
        entries.retain(|queue_name, _| queue_names.contains(queue_name.as_str()));
        self.write_unlocked(entries)
    }

    fn apply(&self, printers: &mut HashMap<String, PrinterEntry>) -> Result<(), Error> {
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

fn store() -> Result<&'static MetadataStore, Error> {
    static STORE: OnceLock<Result<MetadataStore, Error>> = OnceLock::new();
    match STORE.get_or_init(MetadataStore::new) {
        Ok(store) => Ok(store),
        Err(error) => Err(Error::ConfigFailed {
            why: error.to_string(),
        }),
    }
}

pub(super) fn save(queue_name: &str, metadata: QueueMetadata) -> Result<(), Error> {
    store()?.save(queue_name, metadata)
}

pub(super) fn remove(queue_name: &str) -> Result<(), Error> {
    store()?.remove(queue_name)
}

pub(super) fn contains_discovered_printer_id(printer_id: &str) -> Result<bool, Error> {
    store()?.contains_discovered_printer_id(printer_id)
}

pub(super) fn refresh_discovered_printer(
    printer_id: &str,
    printer: &PrinterEntry,
) -> Result<(), Error> {
    store()?.refresh_discovered_printer(printer_id, printer)
}

pub(super) fn stale_discovered_queue_names(
    active_printer_ids: &HashSet<String>,
) -> Result<Vec<String>, Error> {
    store()?.stale_discovered_queue_names(active_printer_ids)
}

pub(super) fn retain_for_configured_queues<'a>(
    queue_names: impl IntoIterator<Item = &'a str>,
) -> Result<(), Error> {
    store()?.retain_for_configured_queues(queue_names)
}

pub(super) fn apply(printers: &mut HashMap<String, PrinterEntry>) -> Result<(), Error> {
    store()?.apply(printers)
}

fn config() -> Result<cosmic_config::Config, Error> {
    cosmic_config::Config::new_state(CONFIG_ID, CONFIG_VERSION).map_err(|error| {
        Error::ConfigFailed {
            why: error.to_string(),
        }
    })
}

fn load_from(config: &cosmic_config::Config) -> Result<MetadataMap, Error> {
    match config.get(METADATA_KEY) {
        Ok(entries) => Ok(entries),
        Err(cosmic_config::Error::NotFound) => Ok(HashMap::new()),
        Err(error) => Err(Error::ConfigFailed {
            why: error.to_string(),
        }),
    }
}
