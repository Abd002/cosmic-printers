use cosmic_config::{ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

pub(super) fn save(queue_name: &str, metadata: QueueMetadata) -> Result<(), Error> {
    let config = config()?;
    let mut entries = load_from(&config);
    entries.insert(queue_name.to_string(), metadata);
    config
        .set(METADATA_KEY, entries)
        .map_err(|error| Error::ConfigFailed {
            why: error.to_string(),
        })
}

pub(super) fn remove(queue_name: &str) -> Result<(), Error> {
    let config = config()?;
    let mut entries = load_from(&config);
    entries.remove(queue_name);
    config
        .set(METADATA_KEY, entries)
        .map_err(|error| Error::ConfigFailed {
            why: error.to_string(),
        })
}

pub(super) fn contains_discovered_printer_id(printer_id: &str) -> Result<bool, Error> {
    let config = config()?;
    let entries = load_from(&config);

    Ok(entries.values().any(|metadata| {
        discovered_printer_id(&metadata.discovered_printer).as_deref() == Some(printer_id)
    }))
}

pub(super) fn apply(printers: &mut HashMap<String, PrinterEntry>) -> Result<(), Error> {
    let config = config()?;
    let entries = load_from(&config);

    for printer in printers.values_mut() {
        let (queue_name, _) = split_queue_instance(&printer.id);
        let Some(metadata) = entries.get(queue_name) else {
            continue;
        };

        printer.merge_from(metadata.discovered_printer.clone());
    }

    Ok(())
}

fn config() -> Result<cosmic_config::Config, Error> {
    cosmic_config::Config::new_state(CONFIG_ID, CONFIG_VERSION).map_err(|error| {
        Error::ConfigFailed {
            why: error.to_string(),
        }
    })
}

fn load_from(config: &cosmic_config::Config) -> MetadataMap {
    config.get(METADATA_KEY).unwrap_or_default()
}
