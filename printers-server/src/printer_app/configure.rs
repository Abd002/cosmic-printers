//! Printer Application `Create-Printer` requests and outcome reconciliation.

use cosmic_settings_printers_core::{
    ConfigurePrinterReply, Error, PrinterApplication, PrinterConfigurationState,
};
use cups_rs::{IppOperation, IppTag, IppValueTag};

use super::client::{OperationCost, PaError, PaRequest, bounded, check_status};
use super::identity::PaConfigurationCandidate;
use super::printers::{self, ConfiguredPrinter};
use super::reconcile::{PendingConfigurationState, PendingPaConfiguration};
use super::{drivers, errors, identity, web};
use crate::state::State;

/// The only value PAPPL accepts for `printer-service-type`.
const PRINTER_SERVICE_TYPE: &str = "print";

/// The attribute naming the device to configure.
pub(crate) const DEVICE_URI: &str = "smi55357-device-uri";

/// The attribute naming the driver to use.
pub(crate) const DRIVER: &str = "smi55357-driver";

/// The attribute carrying the device's IEEE-1284 identification.
const DEVICE_ID: &str = "printer-device-id";

/// The attribute carrying a printer's readable description.
const PRINTER_INFO: &str = "printer-info";

/// PAPPL rejects a `printer-name` longer than this.
const MAX_PRINTER_NAME_LENGTH: usize = 127;

/// How many suffixed names to try before giving up on finding a free one.
const MAX_NAME_ATTEMPTS: u32 = 50;

/// What a Printer Application was asked to create.
#[derive(Clone, Debug)]
pub(crate) struct CreatePrinterRequest {
    pub(crate) printer_name: String,
    /// The description shown to a user, kept separate from the queue name.
    pub(crate) printer_info: Option<String>,
    /// The exact URI the owning application reported for this device.
    pub(crate) device_uri: String,
    /// The driver name to use, or `auto` when the application may choose.
    pub(crate) driver: String,
    /// The device ID, passed through so the application can validate the match.
    pub(crate) device_id: Option<String>,
    /// The optional attributes this application said it accepts at creation.
    pub(crate) accepted_attributes: Vec<String>,
}

impl CreatePrinterRequest {
    fn accepts(&self, attribute: &str) -> bool {
        self.accepted_attributes.is_empty()
            || self
                .accepted_attributes
                .iter()
                .any(|accepted| accepted == attribute)
    }
}

/// How a create attempt ended.
#[derive(Debug)]
pub(crate) enum CreateOutcome {
    /// The printer was created.
    Created {
        printer_name: String,
        printer_uri: Option<String>,
        printer_uuid: Option<String>,
    },
    /// A printer for this device already existed.
    AlreadyConfigured { printer_name: String },
    /// The application refused, and named attributes it could not accept.
    Rejected {
        status: String,
        why: String,
        unsupported_attributes: Vec<String>,
    },
    /// The sent request has an unknown outcome and must not be retried.
    UnknownOutcome { printer_name: String, why: String },
}

impl CreateOutcome {
    /// Returns whether the application refused this route to the device rather
    /// than the request itself.
    pub(crate) fn device_route_was_refused(&self) -> bool {
        match self {
            Self::Rejected {
                unsupported_attributes,
                ..
            } => unsupported_attributes
                .iter()
                .any(|attribute| attribute == DEVICE_URI),
            _ => false,
        }
    }
}

/// Sends `Create-Printer` and establishes what happened.
pub(crate) fn create_printer(
    system_uri: &str,
    request: &CreatePrinterRequest,
) -> Result<CreateOutcome, PaError> {
    // IPP groups must be emitted once and in order. PAPPL requires its vendor URI and driver
    // attributes in the operation group despite their printer-oriented names.
    let mut ipp = PaRequest::new(IppOperation::CreatePrinter, system_uri)?
        .string(
            IppTag::Operation,
            IppValueTag::Keyword,
            "printer-service-type",
            PRINTER_SERVICE_TYPE,
        )?
        .string(
            IppTag::Operation,
            IppValueTag::Uri,
            DEVICE_URI,
            &request.device_uri,
        )?
        .string(
            IppTag::Operation,
            IppValueTag::Keyword,
            DRIVER,
            &request.driver,
        )?
        .string(
            IppTag::Printer,
            IppValueTag::Name,
            "printer-name",
            &request.printer_name,
        )?;

    if let Some(device_id) = &request
        .device_id
        .as_deref()
        .filter(|_| request.accepts(DEVICE_ID))
    {
        ipp = ipp.string(IppTag::Printer, IppValueTag::Text, DEVICE_ID, device_id)?;
    }
    if let Some(info) = &request
        .printer_info
        .as_deref()
        .filter(|_| request.accepts(PRINTER_INFO))
    {
        ipp = ipp.string(IppTag::Printer, IppValueTag::Text, PRINTER_INFO, info)?;
    }

    // Rejection attributes distinguish an unsupported driver from an unusable URI.
    let response = match ipp.send_allowing_failure(system_uri, OperationCost::Create) {
        Ok(response) => response,
        Err(PaError::Unreachable { why }) => {
            // The request may have been carried out. Look before retrying.
            return Ok(recover_unknown_outcome(system_uri, request, why));
        }
        Err(error) => return Err(error),
    };

    if let Err(error) = check_status(&response) {
        return match error {
            // PAPPL may create the printer before reporting an unsupported attribute.
            PaError::Rejected { status, why } => Ok(created_or_rejected(
                system_uri,
                request,
                CreateOutcome::Rejected {
                    status,
                    why,
                    unsupported_attributes: unsupported_attributes(&response),
                },
            )),
            other => Err(other),
        };
    }

    Ok(CreateOutcome::Created {
        printer_name: response
            .find_attribute("printer-name", None)
            .and_then(|attribute| attribute.get_string(0))
            .map(bounded)
            .unwrap_or_else(|| request.printer_name.clone()),
        printer_uri: response
            .find_attribute("printer-uri-supported", None)
            .and_then(|attribute| attribute.get_string(0))
            .map(bounded),
        printer_uuid: response
            .find_attribute("printer-uuid", None)
            .and_then(|attribute| attribute.get_string(0))
            .map(bounded),
    })
}

/// Resolves a lost reply by exact device URI, then requested name.
fn recover_unknown_outcome(
    system_uri: &str,
    request: &CreatePrinterRequest,
    why: String,
) -> CreateOutcome {
    created_or_rejected(
        system_uri,
        request,
        CreateOutcome::UnknownOutcome {
            printer_name: request.printer_name.clone(),
            why,
        },
    )
}

/// Returns the printer this request produced, or `otherwise` if it produced none.
fn created_or_rejected(
    system_uri: &str,
    request: &CreatePrinterRequest,
    otherwise: CreateOutcome,
) -> CreateOutcome {
    let Ok(printers) = printers::get_printers(system_uri) else {
        return otherwise;
    };
    let Some(existing) = printers::find_by_device_uri(&printers, &request.device_uri)
        .or_else(|| printers::find_by_name(&printers, &request.printer_name))
    else {
        return otherwise;
    };

    CreateOutcome::Created {
        printer_name: existing.name.clone(),
        printer_uri: existing.printer_uri.clone(),
        printer_uuid: existing.printer_uuid.clone(),
    }
}

/// Returns PAPPL's unsupported attributes for rejection classification.
fn unsupported_attributes(response: &cups_rs::IppResponse) -> Vec<String> {
    let mut names = response
        .attributes()
        .into_iter()
        .filter(|attribute| attribute.group_tag() == Some(IppTag::UnsupportedGroup))
        .filter_map(|attribute| attribute.name())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();

    names
}

/// Normalizes a display name to PAPPL's 127-character queue-name grammar.
pub(crate) fn printer_name_from_display_name(display_name: &str) -> String {
    let mut name = String::new();

    for character in display_name.chars() {
        let mapped = if character.is_ascii_alphanumeric() {
            character
        } else if matches!(character, '-' | '.' | '_') || character.is_whitespace() {
            '_'
        } else {
            // Drop unsupported characters rather than guessing transliterations.
            continue;
        };

        // Collapse runs of separators so `Acme   Laser` does not become
        // `Acme___Laser`.
        if mapped == '_' && name.ends_with('_') {
            continue;
        }
        name.push(mapped);
    }

    let name = name.trim_matches('_');
    let mut name = if name.is_empty() {
        "printer".to_string()
    } else {
        name.to_string()
    };

    // Must start with a letter or underscore, so a leading digit gets a prefix
    // rather than being dropped.
    if name.starts_with(|character: char| character.is_ascii_digit()) {
        name.insert(0, '_');
    }

    truncate_name(name)
}

/// Returns a name not already used in this Printer Application.
pub(crate) fn unique_printer_name(base: &str, printers: &[ConfiguredPrinter]) -> Option<String> {
    if printers::find_by_name(printers, base).is_none() {
        return Some(base.to_string());
    }

    (2..=MAX_NAME_ATTEMPTS).find_map(|suffix| {
        let candidate = truncate_name_with_suffix(base, suffix);
        printers::find_by_name(printers, &candidate)
            .is_none()
            .then_some(candidate)
    })
}

fn truncate_name(mut name: String) -> String {
    if name.len() > MAX_PRINTER_NAME_LENGTH {
        name.truncate(MAX_PRINTER_NAME_LENGTH);
    }

    name.trim_end_matches('_').to_string()
}

/// Appends a suffix, trimming the base so the whole name still fits.
fn truncate_name_with_suffix(base: &str, suffix: u32) -> String {
    let suffix = format!("_{suffix}");
    let room = MAX_PRINTER_NAME_LENGTH.saturating_sub(suffix.len());
    let mut name = base.to_string();
    if name.len() > room {
        name.truncate(room);
    }
    name.push_str(&suffix);

    name
}

/// Configures a discovered printer through the selected Printer Application.
/// Discovery state is revalidated, and device URIs and drivers come only from server-held records.
pub(crate) async fn configure_discovered_printer(
    context: &State,
    request: cosmic_settings_printers_core::ConfigureDiscoveredPrinterRequest,
) -> Result<ConfigurePrinterReply, Error> {
    let candidate = context
        .resolve_add_printer_candidate(
            request.discovery_generation,
            &request.physical_printer_id,
            &request.candidate_id,
        )
        .map_err(|error| errors::resolve_error(error, &request))?;

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

    // Prefer the user's name, then the displayed make/model, then the description.
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

    // Fallback routes remain within the selected application and physical printer.
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

/// Tries a candidate's device routes in preference order.
/// Only unreachable or device-rejected routes fall through; ambiguous outcomes may already have created it.
fn create_printer_over_endpoints(
    system_uri: &str,
    endpoints: &[identity::PaDeviceEndpoint],
    printer: &PrinterToCreate,
) -> Result<CreateOutcome, PaError> {
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
) -> Result<CreateOutcome, PaError> {
    let existing = if printer.supports_get_printers {
        printers::get_printers(system_uri).unwrap_or_default()
    } else {
        Vec::new()
    };

    // The same device already configured through this application is not an
    // error and not a second printer.
    if let Some(already) = printers::find_by_device_uri(&existing, device_uri) {
        return Ok(CreateOutcome::AlreadyConfigured {
            printer_name: already.name.clone(),
        });
    }

    let base_name = printer_name_from_display_name(&printer.queue_source);
    let printer_name = unique_printer_name(&base_name, &existing).ok_or_else(|| {
        PaError::malformed(format!(
            "could not find an unused printer name based on '{base_name}'"
        ))
    })?;

    create_printer(
        system_uri,
        &CreatePrinterRequest {
            printer_name,
            printer_info: Some(printer.display_name.clone()),
            device_uri: device_uri.to_string(),
            driver: printer.driver.clone(),
            device_id: printer.device_id.clone(),
            accepted_attributes: printer.accepted_attributes.clone(),
        },
    )
}

/// Keeps a receipt whenever an outcome names a printer, including uncertain failures.
fn finish_configuration(
    context: &State,
    application: &PrinterApplication,
    candidate: &PaConfigurationCandidate,
    request: &cosmic_settings_printers_core::ConfigureDiscoveredPrinterRequest,
    outcome: Result<CreateOutcome, PaError>,
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
        Ok(CreateOutcome::Created {
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
            crate::cups::refresh_available_destinations(context.clone());

            Ok(ConfigurePrinterReply {
                operation_id,
                state: PrinterConfigurationState::AwaitingAdvertisement,
                configured_printer_name: printer_name,
                destination_id: None,
                web_interface_uri,
            })
        }
        Ok(CreateOutcome::AlreadyConfigured { printer_name }) => {
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
        Ok(CreateOutcome::Rejected {
            status,
            why,
            unsupported_attributes,
        }) => {
            // Exhausted driver or URI routes require manual setup in the application.
            if let Some(attribute) = unsupported_attributes
                .iter()
                .find(|attribute| *attribute == DRIVER || *attribute == DEVICE_URI)
            {
                let why = if attribute == DRIVER {
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
        Ok(CreateOutcome::UnknownOutcome { printer_name, why }) => {
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
        Err(error) => Err(errors::configuration_error(
            application,
            error,
            web_interface_uri,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(name: &str) -> ConfiguredPrinter {
        ConfiguredPrinter {
            printer_id: Some(1),
            name: name.to_string(),
            device_uri: None,
            printer_uri: None,
            printer_uuid: None,
            web_interface_uri: None,
        }
    }

    #[test]
    fn spaces_become_underscores() {
        assert_eq!(
            printer_name_from_display_name("HP LaserJet 4050"),
            "HP_LaserJet_4050"
        );
    }

    #[test]
    fn characters_a_printer_application_rejects_are_removed() {
        assert_eq!(
            printer_name_from_display_name("Acme/Laser #2 (front)"),
            "AcmeLaser_2_front"
        );
        assert_eq!(
            printer_name_from_display_name("Acme\u{7}Laser"),
            "AcmeLaser"
        );
    }

    #[test]
    fn runs_of_separators_collapse() {
        assert_eq!(
            printer_name_from_display_name("Acme   Test    Laser"),
            "Acme_Test_Laser"
        );
        assert_eq!(printer_name_from_display_name("__Acme__"), "Acme");
    }

    #[test]
    fn a_name_always_starts_with_a_letter_or_underscore() {
        assert_eq!(printer_name_from_display_name("4050 Laser"), "_4050_Laser");
    }

    #[test]
    fn an_unusable_display_name_still_yields_a_name() {
        assert_eq!(printer_name_from_display_name(""), "printer");
        assert_eq!(printer_name_from_display_name("///"), "printer");
        assert_eq!(printer_name_from_display_name("测试打印机"), "printer");
    }

    #[test]
    fn names_stay_within_the_length_a_printer_application_accepts() {
        let name = printer_name_from_display_name(&"a".repeat(300));

        assert_eq!(name.len(), MAX_PRINTER_NAME_LENGTH);
    }

    #[test]
    fn normalization_is_stable() {
        let once = printer_name_from_display_name("HP LaserJet 4050");
        let twice = printer_name_from_display_name(&once);

        assert_eq!(once, twice);
    }

    #[test]
    fn a_free_name_is_used_unchanged() {
        assert_eq!(
            unique_printer_name("HP_LaserJet_4050", &[]).as_deref(),
            Some("HP_LaserJet_4050")
        );
    }

    #[test]
    fn a_name_used_by_another_device_gets_a_suffix() {
        let printers = vec![configured("HP_LaserJet_4050")];

        assert_eq!(
            unique_printer_name("HP_LaserJet_4050", &printers).as_deref(),
            Some("HP_LaserJet_4050_2")
        );

        let printers = vec![
            configured("HP_LaserJet_4050"),
            configured("HP_LaserJet_4050_2"),
        ];
        assert_eq!(
            unique_printer_name("HP_LaserJet_4050", &printers).as_deref(),
            Some("HP_LaserJet_4050_3")
        );
    }

    #[test]
    fn suffixing_keeps_the_name_within_the_length_limit() {
        let base = "a".repeat(MAX_PRINTER_NAME_LENGTH);
        let printers = vec![configured(&base)];
        let name = unique_printer_name(&base, &printers).expect("a unique name");

        assert!(name.len() <= MAX_PRINTER_NAME_LENGTH);
        assert!(name.ends_with("_2"));
    }

    #[test]
    fn giving_up_is_reported_rather_than_looping() {
        let mut printers = vec![configured("printer")];
        printers
            .extend((2..=MAX_NAME_ATTEMPTS).map(|suffix| configured(&format!("printer_{suffix}"))));

        assert_eq!(unique_printer_name("printer", &printers), None);
    }
}
