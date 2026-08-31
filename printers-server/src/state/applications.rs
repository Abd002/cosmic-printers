//! The Printer Applications DNS-SD is advertising, and what probing them found.

use cosmic_settings_printers_core::PrinterApplication;
use std::collections::HashSet;

use super::State;

impl State {
    pub(crate) async fn printer_applications_cached(&self) -> Vec<PrinterApplication> {
        let mut applications = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    pub(crate) async fn merge_printer_application_discovery(
        &self,
        application: PrinterApplication,
    ) -> bool {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let application_id = application.id.clone();
        let inserted = !model.printer_applications.contains_key(&application_id);
        let mut restarted = false;
        let changed = if let Some(existing) = model.printer_applications.get_mut(&application_id) {
            let before = existing.clone();
            // An endpoint change invalidates capabilities cached before the restart.
            restarted = existing.system_uri != application.system_uri;
            existing.merge_discovery_record(application);
            *existing != before
        } else {
            model
                .printer_applications
                .insert(application_id.clone(), application);
            true
        };
        if restarted {
            model.driver_answers.remove(&application_id);
            model.configured_devices.remove(&application_id);
        }
        drop(model);

        if changed {
            self.emit_printer_applications_changed();
        }
        inserted
    }

    pub(crate) async fn update_printer_application_probe(
        &self,
        application_id: &str,
        update: impl FnOnce(&mut PrinterApplication),
    ) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    pub(crate) async fn retain_printer_applications(&self, active_ids: &HashSet<String>) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = model
            .printer_applications
            .keys()
            .filter(|id| !active_ids.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        model
            .printer_applications
            .retain(|id, _| active_ids.contains(id));
        // Remove only the departed application's discovery and driver results.
        let discovery_changed = removed.iter().fold(false, |changed, id| {
            model.driver_answers.remove(id);
            model.configured_devices.remove(id);
            model.add_printer_discovery.remove_application(id) || changed
        });
        drop(model);

        if !removed.is_empty() {
            self.emit_printer_applications_changed();
        }
        if discovery_changed {
            self.emit_add_printer_discovery_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::{PrinterApplicationState, PrintersEventKind};
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
            make_and_model: None,
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: cosmic_settings_printers_core::PrinterApplicationCapabilities::default(),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Discovered,
        }
    }

    #[tokio::test]
    async fn printer_applications_use_a_separate_cache_and_event() {
        let context = State::new();
        let mut events = context.subscribe_events();

        assert!(
            context
                .merge_printer_application_discovery(application("app"))
                .await
        );
        assert_eq!(context.printer_applications_cached().await.len(), 1);
        assert_eq!(
            events.recv().await.unwrap().kind,
            PrintersEventKind::PrinterApplicationsChanged
        );

        context.retain_printer_applications(&HashSet::new()).await;
        assert!(context.printer_applications_cached().await.is_empty());
    }

    #[tokio::test]
    async fn repeated_application_discovery_merges_without_requesting_another_probe() {
        let context = State::new();
        let mut first = application("app");
        first.addresses = vec!["192.0.2.1".into()];
        let mut repeated = application("app");
        repeated.addresses = vec!["2001:db8::1".into()];

        assert!(context.merge_printer_application_discovery(first).await);
        assert!(!context.merge_printer_application_discovery(repeated).await);

        let applications = context.printer_applications_cached().await;
        assert_eq!(
            applications[0].addresses,
            vec!["192.0.2.1".to_string(), "2001:db8::1".to_string()]
        );
    }

    #[tokio::test]
    async fn retaining_applications_removes_inactive_entries() {
        let context = State::new();
        context
            .merge_printer_application_discovery(application("keep"))
            .await;
        context
            .merge_printer_application_discovery(application("remove"))
            .await;

        context
            .retain_printer_applications(&HashSet::from(["keep".to_string()]))
            .await;

        let applications = context.printer_applications_cached().await;
        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].id, "keep");
    }
}
