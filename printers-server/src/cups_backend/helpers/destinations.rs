use cups_rs::{Destination, enum_destinations};
use std::collections::HashMap;

use crate::error::{BackendError, BackendResult};

/// Enumerates all destinations reported by libcups, including DNS-SD services.
pub(in crate::cups_backend) fn available_destinations(
    timeout_ms: i32,
) -> BackendResult<HashMap<String, Destination>> {
    let mut destinations = HashMap::<String, Destination>::new();

    enum_destinations(
        cups_rs::DEST_FLAGS_NONE,
        timeout_ms,
        None,
        0,
        0,
        &mut |flags, destination, destinations: &mut HashMap<String, Destination>| {
            let id = destination.full_name();

            if flags & cups_rs::DEST_FLAGS_REMOVED != 0 {
                destinations.remove(&id);
            } else {
                merge_destination(destinations, id, destination);
            }

            true
        },
        &mut destinations,
    )
    .map_err(BackendError::FailedToGetPrinters)?;

    Ok(destinations)
}

fn merge_destination(
    destinations: &mut HashMap<String, Destination>,
    id: String,
    incoming: &Destination,
) {
    let Some(current) = destinations.get_mut(&id) else {
        destinations.insert(id, incoming.clone());
        return;
    };

    current.name.clone_from(&incoming.name);
    current.instance.clone_from(&incoming.instance);
    current.is_default = incoming.is_default;

    for (name, value) in &incoming.options {
        if !value.is_empty() || !current.options.contains_key(name) {
            current.options.insert(name.clone(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(name: &str, options: &[(&str, &str)]) -> Destination {
        Destination {
            name: name.to_string(),
            instance: None,
            is_default: false,
            options: options
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        }
    }

    #[test]
    fn repeated_callbacks_preserve_metadata_omitted_by_an_update() {
        let mut destinations = HashMap::new();
        let resolved = destination(
            "office",
            &[
                ("printer-uuid", "urn:uuid:1234"),
                ("printer-more-info", "https://printer.local/"),
            ],
        );
        let partial = destination("office", &[("printer-location", "Office")]);

        merge_destination(&mut destinations, resolved.full_name(), &resolved);
        merge_destination(&mut destinations, partial.full_name(), &partial);

        let merged = &destinations["office"];
        assert_eq!(
            merged.options.get("printer-uuid").map(String::as_str),
            Some("urn:uuid:1234")
        );
        assert_eq!(
            merged.options.get("printer-more-info").map(String::as_str),
            Some("https://printer.local/")
        );
        assert_eq!(
            merged.options.get("printer-location").map(String::as_str),
            Some("Office")
        );
    }

    #[test]
    fn services_on_one_host_remain_distinct_destinations() {
        let mut destinations = HashMap::new();
        let first = destination(
            "first",
            &[("device-uri", "ipps://host.local:8880/ipp/print")],
        );
        let second = destination(
            "second",
            &[("device-uri", "ipps://host.local:8881/ipp/print")],
        );

        merge_destination(&mut destinations, first.full_name(), &first);
        merge_destination(&mut destinations, second.full_name(), &second);

        assert_eq!(destinations.len(), 2);
        assert_eq!(
            destinations["first"]
                .options
                .get("device-uri")
                .map(String::as_str),
            Some("ipps://host.local:8880/ipp/print")
        );
        assert_eq!(
            destinations["second"]
                .options
                .get("device-uri")
                .map(String::as_str),
            Some("ipps://host.local:8881/ipp/print")
        );
    }
}
