//! Running one round of Add Printer discovery.

use cosmic_settings_printers_core::{
    PrinterApplication, PrinterApplicationScanState, StartAddPrinterDiscoveryReply,
};
use std::collections::HashMap;

use super::client::PaError;
use super::identity::PaConfigurationCandidate;
use super::round::DiscoveryGeneration;
use super::{devices, drivers, identity, printers, probe};
use crate::state::State;

/// Starts a discovery generation and publishes each application's results independently.
pub(crate) async fn start_add_printer_discovery(context: State) -> StartAddPrinterDiscoveryReply {
    let generation = context.start_add_printer_discovery();

    for application in context.add_printer_scan_targets() {
        let context = context.clone();
        tokio::spawn(async move {
            scan_application(context, application, generation).await;
        });
    }

    StartAddPrinterDiscoveryReply { generation }
}

/// Asks one Printer Application for devices and checks driver support.
pub(super) async fn scan_application(
    context: State,
    application: PrinterApplication,
    generation: DiscoveryGeneration,
) {
    // One scan per application at a time. A second round starting while this one
    // runs waits here rather than making the application rescan concurrently.
    let scan_lock = context.scan_lock(&application.id);
    let _scan_guard = scan_lock.lock().await;
    let _permit = context.acquire_scan_permit().await;

    // The round may have been superseded while waiting for the lock.
    if context.add_printer_generation() != generation {
        return;
    }
    context.mark_printer_application_searching(generation, &application.id);

    let application = match probe::reprobe_if_needed(&context, application).await {
        Ok(application) => application,
        Err(state) => {
            context.replace_printer_application_snapshot(
                generation,
                // The identifier is still valid even though the probe failed.
                &probe::probe_failure_id(&state.0),
                state.1,
                Vec::new(),
                0,
            );
            return;
        }
    };

    if !application.capabilities.find_devices {
        context.replace_printer_application_snapshot(
            generation,
            &application.id,
            PrinterApplicationScanState::Unsupported,
            Vec::new(),
            0,
        );
        return;
    }

    // A Printer Application on another host refuses administration outright, so
    // asking it for devices would only produce a forbidden reply.
    if !application.is_local() {
        context.replace_printer_application_snapshot(
            generation,
            &application.id,
            PrinterApplicationScanState::Unsupported,
            Vec::new(),
            0,
        );
        return;
    }

    let system_uri = application.administration_uri();
    let application_id = application.id.clone();
    let remembered = context.remembered_driver_answers(&application.id);
    let remembered_configured = context.remembered_configured_devices(&application.id);
    let scan = tokio::task::spawn_blocking(move || {
        scan_devices_blocking(
            &application_id,
            &system_uri,
            generation,
            &remembered,
            &remembered_configured,
        )
    })
    .await;

    match scan {
        Ok(Ok(scan)) => {
            context.remember_driver_answers(&application.id, scan.driver_answers);
            if let Some(configured) = scan.configured_devices {
                context.remember_configured_devices(&application.id, configured);
            }
            context.replace_printer_application_snapshot(
                generation,
                &application.id,
                PrinterApplicationScanState::Complete,
                scan.candidates,
                scan.quarantined,
            );
        }
        Ok(Err(error)) => {
            let state = scan_state_for(&error);
            tracing::debug!(
                application_id = application.id,
                ?error,
                "printer application device scan did not complete"
            );
            context.replace_printer_application_snapshot(
                generation,
                &application.id,
                state,
                Vec::new(),
                0,
            );
        }
        Err(error) => {
            tracing::warn!(
                application_id = application.id,
                %error,
                "printer application device scan task failed"
            );
            context.replace_printer_application_snapshot(
                generation,
                &application.id,
                PrinterApplicationScanState::Failed,
                Vec::new(),
                0,
            );
        }
    }
}

/// What one application's scan produced.
struct ApplicationScan {
    candidates: Vec<PaConfigurationCandidate>,
    quarantined: usize,
    /// What the application answered about drivers, to be remembered for the next
    /// round. Only real answers appear here.
    driver_answers: HashMap<String, drivers::PaDriverMatch>,
    /// The printers the application says it has, by device URI, when it said.
    /// `None` means it did not answer and what it said before still stands.
    configured_devices: Option<HashMap<String, String>>,
}

/// Finds devices and checks driver support once per device ID.
fn scan_devices_blocking(
    application_id: &str,
    system_uri: &str,
    generation: DiscoveryGeneration,
    remembered: &HashMap<String, drivers::PaDriverMatch>,
    remembered_configured: &HashMap<String, String>,
) -> Result<ApplicationScan, PaError> {
    let found = devices::find_devices(application_id, system_uri, generation)?;
    let mut candidates = identity::collapse_observations(found.observations);
    let mut checked = HashMap::<String, drivers::PaDriverMatch>::new();
    let mut answers = HashMap::<String, drivers::PaDriverMatch>::new();
    // Cache ownership answers to avoid offering duplicate setup after a missed reply.
    let answered = printers::get_printers(system_uri).ok().map(|printers| {
        printers
            .into_iter()
            .filter_map(|printer| {
                // A printer with no device URI recorded says nothing about which
                // device is already set up.
                printer.device_uri.map(|uri| (uri, printer.name))
            })
            .collect::<HashMap<String, String>>()
    });
    let configured = answered
        .clone()
        .unwrap_or_else(|| remembered_configured.clone());

    for candidate in &mut candidates {
        let already = candidate
            .endpoints
            .iter()
            .find_map(|endpoint| configured.get(&endpoint.device_uri));
        if let Some(already) = already {
            candidate.driver_match = drivers::PaDriverMatch::AlreadyConfigured {
                printer_name: already.clone(),
            };
            continue;
        }

        let Some(device_id) = candidate.device_id.clone() else {
            candidate.driver_match = drivers::PaDriverMatch::Unsupported;
            continue;
        };

        candidate.driver_match = match checked.get(&device_id) {
            Some(asked) => asked.clone(),
            None => {
                let mut matched = drivers::find_drivers(system_uri, Some(&device_id));
                if matched.is_an_answer() {
                    answers.insert(device_id.clone(), matched.clone());
                } else if let Some(remembered) = remembered.get(&device_id) {
                    // Reuse a recent answer when the same device receives no reply.
                    tracing::debug!(
                        application_id,
                        ?matched,
                        "using what this application answered before about a device"
                    );
                    matched = remembered.clone();
                }
                checked.insert(device_id, matched.clone());
                matched
            }
        };
    }

    Ok(ApplicationScan {
        candidates,
        quarantined: found.quarantined,
        driver_answers: answers,
        configured_devices: answered,
    })
}

fn scan_state_for(error: &PaError) -> PrinterApplicationScanState {
    match error {
        PaError::AuthenticationRequired => PrinterApplicationScanState::AuthenticationRequired,
        PaError::Forbidden { .. } => PrinterApplicationScanState::AuthenticationRequired,
        PaError::Unreachable { .. } => PrinterApplicationScanState::Unreachable,
        PaError::OperationNotSupported => PrinterApplicationScanState::Unsupported,
        PaError::Rejected { .. } | PaError::Malformed { .. } => PrinterApplicationScanState::Failed,
    }
}
