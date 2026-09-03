//! What a Printer Application said last time, for the times it does not answer.

use std::collections::HashMap;

use super::State;
use crate::printer_app::{ConfiguredPrinter, PaDriverMatch};

/// Retains driver answers briefly to prevent transient discovery flicker.
const DRIVER_ANSWER_MEMORY: std::time::Duration = std::time::Duration::from_secs(600);

/// What an application answered about one device, and when.
#[derive(Clone, Debug)]
pub(super) struct RememberedAnswer {
    matched: PaDriverMatch,
    learned_at: std::time::Instant,
}

/// Retains configured-printer answers longer because duplicate setup is costly.
const CONFIGURED_DEVICE_MEMORY: std::time::Duration = std::time::Duration::from_secs(3600);

/// The printers one application said it has, and when it said so.
#[derive(Clone, Debug)]
pub(super) struct RememberedConfiguredDevices {
    /// Printer name by the device URI it was created for.
    by_device_uri: HashMap<String, String>,
    learned_at: std::time::Instant,
}

/// A cached application printer list used for stable ownership routing.
#[derive(Clone, Debug)]
pub(super) struct RememberedApplicationPrinters {
    printers: Vec<ConfiguredPrinter>,
    learned_at: std::time::Instant,
}

impl State {
    /// Returns the printers this application last said it has, by device URI.
    pub(crate) fn remembered_configured_devices(
        &self,
        application_id: &str,
    ) -> HashMap<String, String> {
        let model = self.locked_model();

        model
            .configured_devices
            .get(application_id)
            .filter(|remembered| {
                std::time::Instant::now().duration_since(remembered.learned_at)
                    < CONFIGURED_DEVICE_MEMORY
            })
            .map(|remembered| remembered.by_device_uri.clone())
            .unwrap_or_default()
    }

    /// Records the printers this application says it has, by device URI.
    pub(crate) fn remember_configured_devices(
        &self,
        application_id: &str,
        by_device_uri: HashMap<String, String>,
    ) {
        self.locked_model().configured_devices.insert(
            application_id.to_string(),
            RememberedConfiguredDevices {
                by_device_uri,
                learned_at: std::time::Instant::now(),
            },
        );
    }

    /// Returns a fresh cached printer list or an empty slice.
    pub(crate) fn remembered_application_printers(
        &self,
        application_id: &str,
    ) -> Vec<ConfiguredPrinter> {
        let model = self.locked_model();

        model
            .application_printers
            .get(application_id)
            .filter(|remembered| {
                std::time::Instant::now().duration_since(remembered.learned_at)
                    < CONFIGURED_DEVICE_MEMORY
            })
            .map(|remembered| remembered.printers.clone())
            .unwrap_or_default()
    }

    /// Records the printers this application listed.
    pub(crate) fn remember_application_printers(
        &self,
        application_id: &str,
        printers: Vec<ConfiguredPrinter>,
    ) {
        self.locked_model().application_printers.insert(
            application_id.to_string(),
            RememberedApplicationPrinters {
                printers,
                learned_at: std::time::Instant::now(),
            },
        );
    }

    pub(crate) fn remove_cached_application_printer(
        &self,
        application_id: &str,
        printer: &ConfiguredPrinter,
    ) {
        let mut model = self.locked_model();

        let remove_printers =
            if let Some(remembered) = model.application_printers.get_mut(application_id) {
                remembered
                    .printers
                    .retain(|cached| !same_application_printer(cached, printer));
                remembered.printers.is_empty()
            } else {
                false
            };
        if remove_printers {
            model.application_printers.remove(application_id);
        }

        let remove_devices =
            if let Some(remembered) = model.configured_devices.get_mut(application_id) {
                remembered.by_device_uri.retain(|device_uri, name| {
                    printer.device_uri.as_deref() != Some(device_uri.as_str())
                        && !name.eq_ignore_ascii_case(&printer.name)
                });
                remembered.by_device_uri.is_empty()
            } else {
                false
            };
        if remove_devices {
            model.configured_devices.remove(application_id);
        }
    }

    /// Returns what this application recently answered about each device's drivers.
    pub(crate) fn remembered_driver_answers(
        &self,
        application_id: &str,
    ) -> HashMap<String, PaDriverMatch> {
        let model = self.locked_model();
        let now = std::time::Instant::now();

        model
            .driver_answers
            .get(application_id)
            .map(|answers| {
                answers
                    .iter()
                    .filter(|(_, answer)| {
                        now.duration_since(answer.learned_at) < DRIVER_ANSWER_MEMORY
                    })
                    .map(|(device_id, answer)| (device_id.clone(), answer.matched.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Records what this application answered about each device's drivers.
    pub(crate) fn remember_driver_answers(
        &self,
        application_id: &str,
        answers: HashMap<String, PaDriverMatch>,
    ) {
        if answers.is_empty() {
            return;
        }

        let learned_at = std::time::Instant::now();
        let mut model = self.locked_model();
        let remembered = model
            .driver_answers
            .entry(application_id.to_string())
            .or_default();
        for (device_id, matched) in answers {
            remembered.insert(
                device_id,
                RememberedAnswer {
                    matched,
                    learned_at,
                },
            );
        }
    }
}

fn same_application_printer(left: &ConfiguredPrinter, right: &ConfiguredPrinter) -> bool {
    left.name.eq_ignore_ascii_case(&right.name)
        || left
            .printer_uri
            .as_deref()
            .zip(right.printer_uri.as_deref())
            .is_some_and(|(left, right)| left == right)
        || left
            .device_uri
            .as_deref()
            .zip(right.device_uri.as_deref())
            .is_some_and(|(left, right)| left == right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(name: &str, device_uri: &str) -> ConfiguredPrinter {
        ConfiguredPrinter {
            printer_id: Some(1),
            name: name.to_string(),
            device_uri: Some(device_uri.to_string()),
            printer_uri: Some(format!("ipp://localhost:8000/ipp/print/{name}")),
            printer_uuid: None,
            web_interface_uri: None,
        }
    }

    #[test]
    fn removing_one_cached_application_printer_keeps_the_other_cached_printers() {
        let context = State::new();
        let deleted = printer("Deleted", "socket://192.0.2.10");
        let kept = printer("Kept", "socket://192.0.2.11");

        context.remember_application_printers("app", vec![deleted.clone(), kept.clone()]);
        context.remember_configured_devices(
            "app",
            HashMap::from([
                (
                    deleted.device_uri.clone().expect("test printer has URI"),
                    deleted.name.clone(),
                ),
                (
                    kept.device_uri.clone().expect("test printer has URI"),
                    kept.name.clone(),
                ),
            ]),
        );

        context.remove_cached_application_printer("app", &deleted);

        assert_eq!(context.remembered_application_printers("app"), vec![kept]);
        assert_eq!(
            context.remembered_configured_devices("app"),
            HashMap::from([("socket://192.0.2.11".to_string(), "Kept".to_string())])
        );
    }
}
