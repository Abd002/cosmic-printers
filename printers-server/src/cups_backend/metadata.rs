use cosmic_config::{ConfigGet, ConfigSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use cosmic_settings_printers_core::{Error, PrinterEntry};

use super::helpers::split_queue_instance;

const CONFIG_ID: &str = "com.system76.CosmicSettings.Printers";
const CONFIG_VERSION: u64 = 1;
const METADATA_KEY: &str = "queue_metadata";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct QueueMetadata {
    pub device_uuid: Option<String>,
    pub printer_more_info: Option<String>,
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

pub(super) fn apply(printers: &mut HashMap<String, PrinterEntry>) -> Result<(), Error> {
    let config = config()?;
    let entries = load_from(&config);

    for printer in printers.values_mut() {
        let (queue_name, _) = split_queue_instance(&printer.id);
        let Some(metadata) = entries.get(queue_name) else {
            continue;
        };

        if let Some(device_uuid) = &metadata.device_uuid {
            printer
                .options
                .insert("device-uuid".to_string(), device_uuid.clone());
        }
        if let Some(printer_more_info) = &metadata.printer_more_info {
            printer
                .options
                .insert("printer-more-info".to_string(), printer_more_info.clone());
            printer.web_page = Some(printer_more_info.clone());
        }
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
