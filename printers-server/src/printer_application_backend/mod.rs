//! Setting up a printer through the Printer Application that can drive it.
//!
//! Add Printer exists for hardware that needs a driver: a raw socket label printer,
//! a PCL-only laser, an old inkjet. A driverless IPP printer, a shared CUPS queue,
//! and a printer an application has already created all arrive through the ordinary
//! destination pipeline instead, and nothing here runs for them.
//!
//! The shape of a round: browse DNS-SD for Printer Applications, ask each what it
//! can do, ask it for devices, ask it per device whether it has a driver, collapse
//! what several applications report into one row per physical printer, and — once
//! the user picks — ask the owning application to create the printer. It advertises
//! the result, the destination pipeline discovers it, and [`reconcile`] ties the two
//! together. Nothing here creates a [`cosmic_settings_printers_core::PrinterEntry`].

mod client;
mod configure;
mod configured_printers;
mod devices;
mod discovery;
mod drivers;
mod identity;
pub(crate) mod reconcile;
mod system;
mod web;

use cosmic_settings_printers_core::{
    ConfigurePrinterReply, Error, ListManualSetupApplicationsReply, ManualSetupPrinterApplication,
    PrinterApplication, PrinterApplicationCapabilities, PrinterApplicationScanState,
    PrinterApplicationState, PrinterConfigurationState, StartAddPrinterDiscoveryReply,
};
use std::collections::HashMap;

use crate::context::Context;

pub(crate) use client::PaError;
pub(crate) use discovery::{AddPrinterDiscovery, DiscoveryGeneration, ResolveError};
pub(crate) use drivers::PaDriverMatch;
pub(crate) use identity::PaConfigurationCandidate;
pub(crate) use reconcile::{PendingConfigurationState, PendingPaConfiguration};

pub(crate) async fn record_discovery(context: Context, application: PrinterApplication) {
    if context
        .merge_printer_application_discovery(application.clone())
        .await
    {
        spawn_system_probe(context, application);
    }
}

fn spawn_system_probe(context: Context, application: PrinterApplication) {
    tokio::spawn(async move {
        let application_id = application.id.clone();
        let result = system::get_system_attributes(application.administration_uri()).await;
        apply_probe_result(&context, &application_id, result).await;
    });
}

async fn apply_probe_result(
    context: &Context,
    application_id: &str,
    result: Result<system::SystemProbe, system::ProbeError>,
) {
    let state;
    let mut probe = None;

    match result {
        Ok(result) => {
            state = probed_state(&result.capabilities);
            probe = Some(result);
        }
        Err(system::ProbeError::AuthenticationRequired) => {
            state = PrinterApplicationState::AuthenticationRequired;
        }
        Err(system::ProbeError::Unreachable { why }) => {
            tracing::warn!(
                application_id,
                why,
                "printer application system probe was unreachable"
            );
            state = PrinterApplicationState::Unreachable;
        }
        Err(system::ProbeError::Failed { why }) => {
            tracing::warn!(
                application_id,
                why,
                "printer application system probe failed"
            );
            state = PrinterApplicationState::Failed;
        }
    }

    context
        .update_printer_application_probe(application_id, move |application| {
            if let Some(probe) = probe {
                application.make_and_model = probe.make_and_model;
                application.endpoints = probe.endpoints;
                application.capabilities = probe.capabilities;
            }
            application.state = state;
        })
        .await;
}

/// Classifies a Printer Application by what its capabilities allow.
///
/// Add Printer can only drive an application all the way through if it can find
/// devices, confirm it has a driver for one, and create the printer. An
/// application that can only find devices is still worth knowing about, and one
/// that can do neither is only useful through its own web interface.
fn probed_state(capabilities: &PrinterApplicationCapabilities) -> PrinterApplicationState {
    if capabilities.supports_automatic_configuration() {
        PrinterApplicationState::Ready
    } else if capabilities.find_devices {
        PrinterApplicationState::DiscoveryOnly
    } else if capabilities.operations_supported.is_empty() {
        PrinterApplicationState::Unsupported
    } else {
        PrinterApplicationState::ManualSetupOnly
    }
}

/// Starts a round of Add Printer discovery.
///
/// Returns as soon as the generation exists, with one task per Printer
/// Application running behind it. Results are published per application as they
/// arrive rather than at the end, because a device scan can take tens of seconds
/// and one slow application must not hide the others.
pub(crate) async fn start_add_printer_discovery(context: Context) -> StartAddPrinterDiscoveryReply {
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
async fn scan_application(
    context: Context,
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

    let application = match reprobe_if_needed(&context, application).await {
        Ok(application) => application,
        Err(state) => {
            context.replace_printer_application_snapshot(
                generation,
                // The identifier is still valid even though the probe failed.
                &probe_failure_id(&state.0),
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

/// Finds devices and establishes which of them this application can drive.
///
/// Driver support is checked per device, because Find-Devices does no filtering:
/// an application reports printers it has no driver for, and showing those as
/// ready candidates would send the user down a path that fails at the end.
/// Results are cached by device ID within the scan, so an application reporting
/// one printer over several transports is only asked once.
///
/// `remembered` holds what this application answered recently, and is consulted
/// only where asking now produced no answer. Without it a request that timed out
/// reads as "no driver", which is what makes a printer offer a driver and then, a
/// refresh later, not.
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
    // A device this application already has a printer for is set up, and offering
    // to set it up again would only produce a second queue for one printer. If it
    // will not say, what it said before stands: it has not stopped having them.
    let answered = configured_printers::get_printers(system_uri)
        .ok()
        .map(|printers| {
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
                    // No answer now, but this application answered about this exact
                    // device recently. Reporting the answer it gave is closer to the
                    // truth than reporting that it has no driver.
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

/// Re-probes an application whose capabilities are missing or were never
/// established.
///
/// Discovery may have seen the application only moments ago, or an earlier probe
/// may have failed while the application was starting up. Either way its
/// capabilities decide whether asking for devices is worth doing.
async fn reprobe_if_needed(
    context: &Context,
    application: PrinterApplication,
) -> Result<PrinterApplication, (String, PrinterApplicationScanState)> {
    let needs_probe = application.capabilities.operations_supported.is_empty()
        || matches!(
            application.state,
            PrinterApplicationState::Discovered
                | PrinterApplicationState::Probing
                | PrinterApplicationState::Unreachable
                | PrinterApplicationState::Failed
        );
    if !needs_probe {
        return Ok(application);
    }

    let application_id = application.id.clone();
    let result = system::get_system_attributes(application.administration_uri()).await;
    let scan_state = match &result {
        Ok(_) => None,
        Err(system::ProbeError::AuthenticationRequired) => {
            Some(PrinterApplicationScanState::AuthenticationRequired)
        }
        Err(system::ProbeError::Unreachable { .. }) => {
            Some(PrinterApplicationScanState::Unreachable)
        }
        Err(system::ProbeError::Failed { .. }) => Some(PrinterApplicationScanState::Failed),
    };
    apply_probe_result(context, &application_id, result).await;

    match scan_state {
        Some(state) => Err((application_id, state)),
        None => context
            .printer_applications_cached()
            .await
            .into_iter()
            .find(|candidate| candidate.id == application_id)
            .ok_or((application_id, PrinterApplicationScanState::Unreachable)),
    }
}

fn probe_failure_id(application_id: &str) -> String {
    application_id.to_string()
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

/// Lists Printer Applications that can be set up through their own interface.
///
/// Every advertised application with a usable web page is included, whether or
/// not it found any devices: an application that found nothing is exactly the one
/// a user needs to open when Add Printer came up empty.
pub(crate) async fn manual_setup_applications(
    context: &Context,
) -> ListManualSetupApplicationsReply {
    let printer_applications = context
        .printer_applications_cached()
        .await
        .into_iter()
        .filter_map(|application| {
            let web_interface_uri = web::application_web_interface(&application)
                .and_then(|uri| web::validate_web_interface(&uri))?;

            Some(ManualSetupPrinterApplication {
                printer_application_id: application.id.clone(),
                display_name: application
                    .make_and_model
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| application.service_name.clone()),
                web_interface_uri,
                state: application.state,
            })
        })
        .collect();

    ListManualSetupApplicationsReply {
        printer_applications,
    }
}

/// Configures a discovered printer through the Printer Application the user
/// chose.
///
/// Every input is revalidated here rather than trusted from the request:
/// discovery may have moved on, the application may have gone away, and the
/// device may have been unplugged since the row was drawn. The caller supplies
/// only identifiers — the device URI and driver come from what the server
/// recorded, so a client cannot ask for an arbitrary device to be configured.
pub(crate) async fn configure_discovered_printer(
    context: &Context,
    request: cosmic_settings_printers_core::ConfigureDiscoveredPrinterRequest,
) -> Result<ConfigurePrinterReply, Error> {
    let candidate = context
        .resolve_add_printer_candidate(
            request.discovery_generation,
            &request.physical_printer_id,
            &request.candidate_id,
        )
        .map_err(|error| resolve_error(error, &request))?;

    let application = context
        .printer_applications_cached()
        .await
        .into_iter()
        .find(|application| application.id == candidate.printer_application_id)
        .ok_or_else(|| Error::PrinterApplicationNotFound {
            application_id: candidate.printer_application_id.clone(),
        })?;

    if !application.supports_automatic_configuration() {
        return Err(Error::PrinterConfigurationManualActionRequired {
            application_id: application.id.clone(),
            web_interface_uri: web::application_web_interface(&application),
            why: "this printer application cannot create printers over IPP".to_string(),
        });
    }

    // The scan already found a printer for this device. Creating another would give
    // one printer two queues, so this is reported rather than acted on.
    if let drivers::PaDriverMatch::AlreadyConfigured { printer_name } = &candidate.driver_match {
        return Err(Error::DiscoveredPrinterAlreadyConfigured {
            application_id: application.id.clone(),
            printer_name: printer_name.clone(),
        });
    }

    let driver = candidate
        .driver_match
        .driver_for_creation()
        .ok_or_else(|| Error::PrinterConfigurationManualActionRequired {
            application_id: application.id.clone(),
            web_interface_uri: web::application_web_interface(&application),
            why: "no single driver matches this printer".to_string(),
        })?
        .to_string();

    let endpoint = candidate
        .preferred_endpoint()
        .ok_or_else(|| Error::PrinterApplicationCandidateNotFound {
            candidate_id: request.candidate_id.clone(),
        })?
        .clone();

    // The device may have gone since the row was drawn.
    if !context.add_printer_candidate_is_current(&candidate.id) {
        return Err(Error::PrinterApplicationCandidateNotFound {
            candidate_id: request.candidate_id.clone(),
        });
    }

    // Serialized per application and device, so two clients configuring the same
    // printer at once produce one printer rather than two.
    let configuration_lock = context.configuration_lock(&application.id, &endpoint.device_uri);
    let _configuration_guard = configuration_lock.lock().await;

    // What the queue is named after, in preference order: what the user asked
    // for, then the device's own make and model, then the description the
    // application gave. The make and model is preferred over the description
    // because it is what the user saw on the row they picked, and because a
    // description often carries qualifiers — "(network)" — that do not belong in a
    // queue name.
    let queue_source = request
        .requested_display_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| candidate.make_and_model.clone())
        .unwrap_or_else(|| candidate.display_name.clone());
    // The readable description keeps the application's own wording.
    let display_name = request
        .requested_display_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| candidate.display_name.clone());
    let system_uri = application.administration_uri();
    let printer = PrinterToCreate {
        driver,
        device_id: candidate.device_id.clone(),
        queue_source,
        display_name,
        accepted_attributes: application
            .capabilities
            .printer_creation_attributes_supported
            .clone(),
        supports_get_printers: application.capabilities.get_printers,
    };

    // Every route belongs to this candidate, so a fallback stays within one
    // application and one physical printer. Falling back across applications
    // would configure through software the user did not choose.
    let routes = std::iter::once(endpoint.clone())
        .chain(candidate.fallback_endpoints().iter().cloned())
        .collect::<Vec<_>>();

    let outcome = tokio::task::spawn_blocking(move || {
        create_printer_over_endpoints(&system_uri, &routes, &printer)
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?;

    finish_configuration(context, &application, &candidate, &request, outcome)
}

/// Tries each of a candidate's routes to the device in preference order.
///
/// Two kinds of failure are worth another route: one that could not be reached at
/// all, and one the application refused *as a device* — an application can report
/// the same printer over several schemes and decline one of them. Anything else
/// stops here: a rejected driver or an already-configured device would give the
/// same answer again, and an ambiguous outcome must never be retried because the
/// printer may already exist.
fn create_printer_over_endpoints(
    system_uri: &str,
    endpoints: &[identity::PaDeviceEndpoint],
    printer: &PrinterToCreate,
) -> Result<configure::CreateOutcome, PaError> {
    let mut last = None;

    for endpoint in endpoints {
        tracing::debug!(
            device_uri = endpoint.device_uri,
            transport = ?endpoint.transport,
            "creating printer through printer application endpoint"
        );

        match create_printer_blocking(system_uri, &endpoint.device_uri, printer) {
            Ok(outcome) if outcome.device_route_was_refused() => {
                tracing::debug!(
                    device_uri = endpoint.device_uri,
                    "the application refused this device uri, trying the next route to this printer"
                );
                last = Some(Ok(outcome));
            }
            Err(PaError::Unreachable { why }) => {
                tracing::debug!(
                    device_uri = endpoint.device_uri,
                    why,
                    "endpoint was unreachable, trying the next route to this printer"
                );
                last = Some(Err(PaError::Unreachable { why }));
            }
            settled => return settled,
        }
    }

    last.unwrap_or_else(|| {
        Err(PaError::malformed(
            "the selected printer has no usable endpoint".to_string(),
        ))
    })
}

/// What to ask an application to create, independent of the route it is asked over.
struct PrinterToCreate {
    driver: String,
    device_id: Option<String>,
    /// What the queue name is derived from.
    queue_source: String,
    /// The readable description, used only where the application accepts one.
    display_name: String,
    accepted_attributes: Vec<String>,
    supports_get_printers: bool,
}

/// Sends `Create-Printer`, having first checked for an existing printer.
fn create_printer_blocking(
    system_uri: &str,
    device_uri: &str,
    printer: &PrinterToCreate,
) -> Result<configure::CreateOutcome, PaError> {
    let existing = if printer.supports_get_printers {
        configured_printers::get_printers(system_uri).unwrap_or_default()
    } else {
        Vec::new()
    };

    // The same device already configured through this application is not an
    // error and not a second printer.
    if let Some(already) = configured_printers::find_by_device_uri(&existing, device_uri) {
        return Ok(configure::CreateOutcome::AlreadyConfigured {
            printer_name: already.name.clone(),
        });
    }

    let base_name = configure::printer_name_from_display_name(&printer.queue_source);
    let printer_name = configure::unique_printer_name(&base_name, &existing).ok_or_else(|| {
        PaError::malformed(format!(
            "could not find an unused printer name based on '{base_name}'"
        ))
    })?;

    configure::create_printer(
        system_uri,
        &configure::CreatePrinterRequest {
            printer_name,
            printer_info: Some(printer.display_name.clone()),
            device_uri: device_uri.to_string(),
            driver: printer.driver.clone(),
            device_id: printer.device_id.clone(),
            accepted_attributes: printer.accepted_attributes.clone(),
        },
    )
}

/// Records the outcome, keeping a receipt for every result that named a printer.
///
/// A receipt is kept even when the call failed, because the states that matter
/// most to a user are the awkward ones: a device that turned out to be already
/// configured, or a request whose outcome could not be established. Those need to
/// remain inspectable through [`printer_configuration`] rather than existing only
/// as a returned error.
///
/// A receipt is never a destination. The Printer Application advertises the
/// printer it created, the ordinary destination pipeline discovers it, and
/// reconciliation records which destination the receipt became.
fn finish_configuration(
    context: &Context,
    application: &PrinterApplication,
    candidate: &PaConfigurationCandidate,
    request: &cosmic_settings_printers_core::ConfigureDiscoveredPrinterRequest,
    outcome: Result<configure::CreateOutcome, PaError>,
) -> Result<ConfigurePrinterReply, Error> {
    let web_interface_uri = web::application_web_interface(application);
    let receipt = |printer_name: &str,
                   state: PendingConfigurationState,
                   printer_uri: Option<String>,
                   printer_uuid: Option<String>|
     -> String {
        let operation_id = format!(
            "{}:{}:{printer_name}",
            application.id, request.discovery_generation
        );
        context.insert_pending_configuration(PendingPaConfiguration {
            operation_id: operation_id.clone(),
            printer_application_id: application.id.clone(),
            physical_printer_id: request.physical_printer_id.clone(),
            candidate_id: candidate.id.clone(),
            configured_printer_name: printer_name.to_string(),
            expected_printer_uri: printer_uri,
            expected_printer_uuid: printer_uuid,
            application_endpoint: Some((application.hostname.clone(), application.port)),
            web_interface_uri: web_interface_uri.clone(),
            created_at: std::time::Instant::now(),
            state,
        });

        operation_id
    };

    match outcome {
        Ok(configure::CreateOutcome::Created {
            printer_name,
            printer_uri,
            printer_uuid,
        }) => {
            let operation_id = receipt(
                &printer_name,
                PendingConfigurationState::AwaitingAdvertisement,
                printer_uri,
                printer_uuid,
            );

            // Ask the destination pipeline to look now rather than at its next
            // scheduled pass, so a newly created printer appears promptly.
            crate::cups_backend::refresh_available_destinations(context.clone());

            Ok(ConfigurePrinterReply {
                operation_id,
                state: PrinterConfigurationState::AwaitingAdvertisement,
                configured_printer_name: printer_name,
                destination_id: None,
                web_interface_uri,
            })
        }
        Ok(configure::CreateOutcome::AlreadyConfigured { printer_name }) => {
            receipt(
                &printer_name,
                PendingConfigurationState::AlreadyConfigured,
                None,
                None,
            );

            Err(Error::DiscoveredPrinterAlreadyConfigured {
                application_id: application.id.clone(),
                printer_name,
            })
        }
        Ok(configure::CreateOutcome::Rejected {
            status,
            why,
            unsupported_attributes,
        }) => {
            // A refused driver or a refused device URI both leave the same way
            // forward: the application's own setup, where the user can pick
            // something else or add the device by hand. Reporting a bare rejection
            // would be a dead end.
            //
            // Every route this candidate offered has already been tried by the
            // time a refused device URI reaches here.
            if let Some(attribute) = unsupported_attributes.iter().find(|attribute| {
                *attribute == configure::DRIVER || *attribute == configure::DEVICE_URI
            }) {
                let why = if attribute == configure::DRIVER {
                    "the printer application rejected the selected driver"
                } else {
                    "the printer application rejected its own device URI for this printer"
                };

                return Err(Error::PrinterConfigurationManualActionRequired {
                    application_id: application.id.clone(),
                    web_interface_uri,
                    why: why.to_string(),
                });
            }

            Err(Error::PrinterConfigurationRejected {
                application_id: application.id.clone(),
                status,
                why: if why.is_empty() {
                    format!(
                        "unsupported attributes: {}",
                        unsupported_attributes.join(", ")
                    )
                } else {
                    why
                },
            })
        }
        Ok(configure::CreateOutcome::UnknownOutcome { printer_name, why }) => {
            tracing::warn!(
                application_id = application.id,
                printer_name,
                why,
                "could not confirm whether the printer was created"
            );
            receipt(
                &printer_name,
                PendingConfigurationState::UnknownOutcome,
                None,
                None,
            );

            Err(Error::PrinterConfigurationUnknownOutcome {
                application_id: application.id.clone(),
                printer_name,
            })
        }
        Err(error) => Err(configuration_error(application, error, web_interface_uri)),
    }
}

/// Returns the state of an earlier configuration attempt.
pub(crate) fn printer_configuration(
    context: &Context,
    operation_id: &str,
) -> Result<ConfigurePrinterReply, Error> {
    let pending = context.pending_configuration(operation_id).ok_or_else(|| {
        Error::PrinterConfigurationUnknownOutcome {
            application_id: String::new(),
            printer_name: operation_id.to_string(),
        }
    })?;

    let (state, destination_id) = match &pending.state {
        PendingConfigurationState::AwaitingAdvertisement => {
            (PrinterConfigurationState::AwaitingAdvertisement, None)
        }
        PendingConfigurationState::Reconciled { destination_id } => (
            PrinterConfigurationState::Reconciled,
            Some(destination_id.clone()),
        ),
        PendingConfigurationState::AlreadyConfigured => {
            (PrinterConfigurationState::AlreadyConfigured, None)
        }
        PendingConfigurationState::ManualActionRequired => {
            (PrinterConfigurationState::ManualActionRequired, None)
        }
        PendingConfigurationState::UnknownOutcome => {
            (PrinterConfigurationState::UnknownOutcome, None)
        }
    };

    Ok(ConfigurePrinterReply {
        operation_id: pending.operation_id,
        state,
        configured_printer_name: pending.configured_printer_name,
        destination_id,
        web_interface_uri: pending.web_interface_uri,
    })
}

fn resolve_error(
    error: ResolveError,
    request: &cosmic_settings_printers_core::ConfigureDiscoveredPrinterRequest,
) -> Error {
    match error {
        ResolveError::NotStarted => Error::AddPrinterDiscoveryNotStarted,
        ResolveError::Expired { generation } => Error::AddPrinterDiscoveryExpired { generation },
        ResolveError::PrinterNotFound => Error::DiscoveredPhysicalPrinterNotFound {
            printer_id: request.physical_printer_id.clone(),
        },
        ResolveError::CandidateNotFound => Error::PrinterApplicationCandidateNotFound {
            candidate_id: request.candidate_id.clone(),
        },
    }
}

fn configuration_error(
    application: &PrinterApplication,
    error: PaError,
    web_interface_uri: Option<String>,
) -> Error {
    let application_id = application.id.clone();

    match error {
        PaError::AuthenticationRequired => {
            Error::PrinterApplicationAuthenticationRequired { application_id }
        }
        PaError::Forbidden { why } => Error::PrinterConfigurationManualActionRequired {
            application_id,
            web_interface_uri,
            why,
        },
        PaError::Unreachable { .. } => Error::PrinterApplicationUnavailable { application_id },
        PaError::OperationNotSupported => Error::PrinterApplicationOperationNotSupported {
            application_id,
            operation: "Create-Printer".to_string(),
        },
        PaError::Rejected { status, why } => Error::PrinterConfigurationRejected {
            application_id,
            status,
            why,
        },
        PaError::Malformed { why } => Error::MalformedPrinterApplicationResponse {
            application_id,
            operation: "Create-Printer".to_string(),
            why,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn application() -> PrinterApplication {
        PrinterApplication {
            id: "app".into(),
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
            capabilities: PrinterApplicationCapabilities::default(),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Discovered,
        }
    }

    fn probe(operations: Vec<u16>) -> system::SystemProbe {
        system::SystemProbe {
            make_and_model: Some("Example Application".into()),
            endpoints: Vec::new(),
            capabilities: PrinterApplicationCapabilities::from_operations(operations),
        }
    }

    /// Find-Devices, Find-Drivers, and Create-Printer together.
    const AUTOMATIC_OPERATIONS: [u16; 3] = [0x402b, 0x402c, 0x004c];

    #[tokio::test]
    async fn full_capability_support_marks_application_ready() {
        let context = Context::new();
        context
            .merge_printer_application_discovery(application())
            .await;
        apply_probe_result(&context, "app", Ok(probe(AUTOMATIC_OPERATIONS.to_vec()))).await;

        let applications = context.printer_applications_cached().await;
        assert_eq!(applications[0].state, PrinterApplicationState::Ready);
        assert!(applications[0].capabilities.find_drivers);
        assert_eq!(
            applications[0].make_and_model.as_deref(),
            Some("Example Application")
        );
    }

    /// An application that can list devices but cannot create a printer is not a
    /// candidate for automatic configuration, but is still worth showing.
    #[tokio::test]
    async fn discovery_without_creation_is_marked_discovery_only() {
        let context = Context::new();
        context
            .merge_printer_application_discovery(application())
            .await;
        apply_probe_result(&context, "app", Ok(probe(vec![0x000b, 0x402b]))).await;

        let applications = context.printer_applications_cached().await;
        assert_eq!(
            applications[0].state,
            PrinterApplicationState::DiscoveryOnly
        );
    }

    #[tokio::test]
    async fn no_device_operations_leaves_only_manual_setup() {
        let context = Context::new();
        context
            .merge_printer_application_discovery(application())
            .await;
        apply_probe_result(&context, "app", Ok(probe(vec![0x000b, 0x003a]))).await;

        let applications = context.printer_applications_cached().await;
        assert_eq!(
            applications[0].state,
            PrinterApplicationState::ManualSetupOnly
        );
    }

    #[tokio::test]
    async fn an_empty_operation_list_is_unsupported() {
        let context = Context::new();
        context
            .merge_printer_application_discovery(application())
            .await;
        apply_probe_result(&context, "app", Ok(probe(Vec::new()))).await;

        let applications = context.printer_applications_cached().await;
        assert_eq!(applications[0].state, PrinterApplicationState::Unsupported);
    }

    #[tokio::test]
    async fn maps_probe_failures_without_removing_application() {
        for (error, expected) in [
            (
                system::ProbeError::AuthenticationRequired,
                PrinterApplicationState::AuthenticationRequired,
            ),
            (
                system::ProbeError::Unreachable {
                    why: "unreachable".into(),
                },
                PrinterApplicationState::Unreachable,
            ),
            (
                system::ProbeError::Failed {
                    why: "failed".into(),
                },
                PrinterApplicationState::Failed,
            ),
        ] {
            let context = Context::new();
            context
                .merge_printer_application_discovery(application())
                .await;
            apply_probe_result(&context, "app", Err(error)).await;

            let applications = context.printer_applications_cached().await;
            assert_eq!(applications.len(), 1);
            assert_eq!(applications[0].state, expected);
        }
    }
}
