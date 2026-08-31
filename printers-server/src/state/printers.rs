//! The printers libcups says exist, as they were last seen.

use cosmic_settings_printers_core::PrinterEntry;
use std::collections::HashSet;

use super::State;
use super::endpoints::apply_resolved_device_endpoint;

impl State {
    pub(crate) async fn available_destination_cached(&self, id: &str) -> Option<PrinterEntry> {
        self.model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .available_destinations
            .get(id)
            .cloned()
    }

    pub(crate) async fn available_destinations_cached(&self) -> Vec<PrinterEntry> {
        let mut destinations = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .available_destinations
            .values()
            .cloned()
            .collect::<Vec<_>>();
        destinations.sort_by(|left, right| left.id().cmp(right.id()));
        destinations
    }

    pub(crate) fn update_available_destination(&self, mut incoming: PrinterEntry) {
        let id = incoming.id().to_string();
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_resolved_device_endpoint(&model.dnssd_device_endpoints, &mut incoming);
        let changed = model.available_destinations.get(&id) != Some(&incoming);
        if changed {
            model.available_destinations.insert(id.clone(), incoming);
        }
        drop(model);

        if changed {
            self.emit_available_destinations_changed(&id);
            self.reconcile_after_destination_change();
        }
    }

    /// Merges a partial enumeration callback without discarding attributes
    /// added by an earlier enrichment pass.
    pub(crate) fn merge_available_destination(&self, mut incoming: PrinterEntry) {
        let id = incoming.id().to_string();
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_resolved_device_endpoint(&model.dnssd_device_endpoints, &mut incoming);
        let changed = if let Some(existing) = model.available_destinations.get_mut(&id) {
            let before = existing.clone();
            existing.merge_enumeration_record(incoming);
            *existing != before
        } else {
            model.available_destinations.insert(id.clone(), incoming);
            true
        };
        drop(model);

        if changed {
            self.emit_available_destinations_changed(&id);
            self.reconcile_after_destination_change();
        }
    }

    pub(crate) fn remove_available_destination(&self, id: &str) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = model.available_destinations.remove(id).is_some();
        model.enumeration_misses.remove(id);
        drop(model);

        if changed {
            self.emit_available_destinations_changed(id);
        }
    }

    /// Prunes destinations absent from completed enumerations.
    pub(crate) fn retain_available_destinations(&self, present: &HashSet<String>) {
        /// How many passes in a row must miss a destination before it is dropped.
        const MISSES_BEFORE_DROPPING: u8 = 2;

        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let missing = model
            .available_destinations
            .keys()
            .filter(|id| !present.contains(*id))
            .cloned()
            .collect::<HashSet<_>>();

        // A destination seen again has not been missing at all, so its count starts over rather
        // than carrying one pass of absence towards a later, unrelated one.
        model
            .enumeration_misses
            .retain(|id, _| missing.contains(id));

        let mut removed = Vec::new();
        for id in missing {
            let misses = model.enumeration_misses.entry(id.clone()).or_default();
            *misses = misses.saturating_add(1);

            if *misses >= MISSES_BEFORE_DROPPING {
                model.enumeration_misses.remove(&id);
                if model.available_destinations.remove(&id).is_some() {
                    removed.push(id);
                }
            }
        }
        drop(model);

        for id in removed {
            self.emit_available_destinations_changed(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::{EndpointSource, PrintersEventKind};
    use std::collections::HashMap;

    fn destination(id: &str, location: &str) -> PrinterEntry {
        PrinterEntry::new(
            id,
            id,
            false,
            HashMap::from([("printer-location".to_string(), location.to_string())]),
        )
    }

    #[tokio::test]
    async fn destination_changes_update_cache_and_emit_once() {
        let context = State::new();
        let mut events = context.subscribe_events();

        context.update_available_destination(destination("office", "first floor"));
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::AvailableDestinationsChanged
        );

        context.update_available_destination(destination("office", "first floor"));
        assert!(events.try_recv().is_err());

        context.update_available_destination(destination("office", "second floor"));
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::AvailableDestinationsChanged
        );
        assert_eq!(
            context.available_destinations_cached().await[0].location(),
            Some("second floor")
        );

        context.remove_available_destination("office");
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::AvailableDestinationsChanged
        );
        assert!(context.available_destinations_cached().await.is_empty());
    }

    #[tokio::test]
    async fn a_destination_missing_from_one_pass_is_kept() {
        let context = State::new();
        context.update_available_destination(destination("office", "first floor"));

        context.retain_available_destinations(&HashSet::new());

        assert_eq!(context.available_destinations_cached().await.len(), 1);
    }

    #[tokio::test]
    async fn a_destination_missing_from_two_passes_running_is_dropped() {
        let context = State::new();
        context.update_available_destination(destination("office", "first floor"));

        context.retain_available_destinations(&HashSet::new());
        context.retain_available_destinations(&HashSet::new());

        assert!(context.available_destinations_cached().await.is_empty());
    }

    #[tokio::test]
    async fn being_seen_again_starts_the_count_over() {
        let context = State::new();
        context.update_available_destination(destination("office", "first floor"));
        let present = HashSet::from(["office".to_string()]);

        for _ in 0..4 {
            context.retain_available_destinations(&HashSet::new());
            context.retain_available_destinations(&present);
        }

        assert_eq!(context.available_destinations_cached().await.len(), 1);
    }

    #[tokio::test]
    async fn pruning_leaves_the_destinations_a_pass_did_see() {
        let context = State::new();
        context.update_available_destination(destination("office", "first floor"));
        context.update_available_destination(destination("studio", "attic"));

        let present = HashSet::from(["studio".to_string()]);
        context.retain_available_destinations(&present);
        context.retain_available_destinations(&present);

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].id(), "studio");
    }

    #[tokio::test]
    async fn partial_destination_update_preserves_enriched_options() {
        let context = State::new();
        let mut enriched = destination("office", "first floor");
        enriched.set_option("endpoint-hostname", "printer.local");
        enriched.set_option("endpoint-port", "8000");
        enriched.set_endpoint_source(EndpointSource::Connected);
        context.update_available_destination(enriched);

        let mut partial = destination("office", "second floor");
        partial.set_option("endpoint-hostname", "printer._ipps._tcp.local");
        partial.set_option("endpoint-port", "631");
        partial.set_endpoint_source(EndpointSource::Uri);
        context.merge_available_destination(partial);

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].location(), Some("second floor"));
        assert_eq!(cached[0].hostname(), Some("printer.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), None);
        assert_eq!(cached[0].endpoint_source(), Some(EndpointSource::Connected));
    }
}
