use cosmic_config::{ConfigGet, ConfigSet};
use cups_rs::Destination;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use cosmic_settings_printers_core::Error;

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
        .map_err(|_| Error::CupsFailed)
}

pub(super) fn apply(destinations: &mut HashMap<String, Destination>) -> Result<(), Error> {
    let config = config()?;
    let entries = load_from(&config);

    for destination in destinations.values_mut() {
        let Some(metadata) = entries.get(&destination.name) else {
            continue;
        };

        if let Some(device_uuid) = &metadata.device_uuid {
            destination
                .options
                .insert("device-uuid".to_string(), device_uuid.clone());
        }
        if let Some(printer_more_info) = &metadata.printer_more_info {
            destination
                .options
                .insert("printer-more-info".to_string(), printer_more_info.clone());
        }
    }

    Ok(())
}

fn config() -> Result<cosmic_config::Config, Error> {
    cosmic_config::Config::new_state(CONFIG_ID, CONFIG_VERSION).map_err(|_| Error::CupsFailed)
}

fn load_from(config: &cosmic_config::Config) -> MetadataMap {
    config.get(METADATA_KEY).unwrap_or_default()
}
