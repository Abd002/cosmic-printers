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
