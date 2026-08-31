use cosmic_settings_printers_core::{
    PrinterApplication, PrinterApplicationCapabilities, PrinterApplicationScanState,
    PrinterApplicationState, SystemEndpoint,
};
use cups_rs::{IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

use crate::error::BackendError;
use crate::ipp::{CupsResultExt, add_requesting_user, send_to};
use crate::state::State;

/// Probe attributes excluding host-wide `system-uuid`, which cannot identify an application.
const SYSTEM_ATTRIBUTES: &[&str] = &[
    "system-name",
    "system-make-and-model",
    "operations-supported",
    "system-xri-supported",
    "system-mandatory-printer-attributes",
    "printer-creation-attributes-supported",
    "printer-service-type-supported",
    "smi55357-device-uri-schemes-supported",
];

/// What a Printer Application reported about itself.
pub(super) struct SystemProbe {
    pub make_and_model: Option<String>,
    pub endpoints: Vec<SystemEndpoint>,
    pub capabilities: PrinterApplicationCapabilities,
}

#[derive(Debug)]
pub(super) enum ProbeError {
    AuthenticationRequired,
    Unreachable { why: String },
    Failed { why: String },
}

pub(super) async fn get_system_attributes(system_uri: String) -> Result<SystemProbe, ProbeError> {
    tokio::task::spawn_blocking(move || get_system_attributes_blocking(&system_uri))
        .await
        .map_err(|error| ProbeError::Failed {
            why: error.to_string(),
        })?
}

fn get_system_attributes_blocking(system_uri: &str) -> Result<SystemProbe, ProbeError> {
    let mut request = IppRequest::new(IppOperation::GetSystemAttributes)
        .cups_err()
        .map_err(probe_failed)?;
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "system-uri",
            system_uri,
        )
        .cups_err()
        .map_err(probe_failed)?;
    add_requesting_user(&mut request).map_err(probe_failed)?;
    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            SYSTEM_ATTRIBUTES,
        )
        .cups_err()
        .map_err(probe_failed)?;

    let response = send_to(request, system_uri).map_err(|error| match error {
        BackendError::PermissionDenied { .. } => ProbeError::AuthenticationRequired,
        // Transport failures are transient availability failures, not unsupported applications.
        error => ProbeError::Unreachable {
            why: error.to_string(),
        },
    })?;

    match response.status() {
        status if status.is_successful() => {}
        IppStatus::ErrorNotAuthorized
        | IppStatus::ErrorForbidden
        | IppStatus::ErrorNotAuthenticated => return Err(ProbeError::AuthenticationRequired),
        status => {
            return Err(ProbeError::Failed {
                why: format!("Get-System-Attributes returned status {status:?}"),
            });
        }
    }

    let operations = response
        .find_attribute("operations-supported", None)
        .ok_or_else(|| ProbeError::Failed {
            why: "Get-System-Attributes response missing operations-supported".to_string(),
        })?;
    let mut operations_supported = (0..operations.count())
        .filter_map(|index| u16::try_from(operations.get_integer(index)).ok())
        .collect::<Vec<_>>();
    operations_supported.sort_unstable();
    operations_supported.dedup();

    let mut capabilities = PrinterApplicationCapabilities::from_operations(operations_supported);
    capabilities.mandatory_printer_attributes =
        keywords(&response, "system-mandatory-printer-attributes");
    capabilities.printer_creation_attributes_supported =
        keywords(&response, "printer-creation-attributes-supported");
    capabilities.printer_service_types_supported =
        keywords(&response, "printer-service-type-supported");
    capabilities.device_uri_schemes_supported =
        keywords(&response, "smi55357-device-uri-schemes-supported");

    Ok(SystemProbe {
        make_and_model: optional_string(&response, "system-make-and-model")
            .or_else(|| optional_string(&response, "system-name")),
        endpoints: system_endpoints(&response),
        capabilities,
    })
}

/// Parses usable authenticated endpoints from `system-xri-supported`.
fn system_endpoints(response: &cups_rs::IppResponse) -> Vec<SystemEndpoint> {
    let mut endpoints = Vec::new();

    for attribute in response.attributes_named("system-xri-supported") {
        for collection in attribute.collections() {
            let Some(uri) = collection.text("xri-uri") else {
                continue;
            };
            let endpoint = SystemEndpoint {
                uri,
                authentication: collection.text("xri-authentication"),
                security: collection.text("xri-security"),
            };
            if !endpoints.contains(&endpoint) {
                endpoints.push(endpoint);
            }
        }
    }

    endpoints
}

/// Reads a multi-valued keyword or name attribute, dropping blank values.
fn keywords(response: &cups_rs::IppResponse, name: &str) -> Vec<String> {
    let Some(attribute) = response.find_attribute(name, None) else {
        return Vec::new();
    };

    let mut values = (0..attribute.count())
        .filter_map(|index| attribute.get_string(index))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();

    values
}

fn probe_failed(error: impl std::fmt::Display) -> ProbeError {
    ProbeError::Failed {
        why: error.to_string(),
    }
}

fn optional_string(response: &cups_rs::IppResponse, name: &str) -> Option<String> {
    response
        .find_attribute(name, None)
        .and_then(|attribute| attribute.get_string(0))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn record_discovery(context: State, application: PrinterApplication) {
    if !context
        .merge_printer_application_discovery(application.clone())
        .await
    {
        return;
    }

    // Join an active round so newly advertised applications are not missed.
    if let Some(generation) = context.join_add_printer_round(&application.id) {
        tokio::spawn(async move {
            super::scan::scan_application(context, application, generation).await;
        });
        return;
    }

    spawn_system_probe(context, application);
}

fn spawn_system_probe(context: State, application: PrinterApplication) {
    tokio::spawn(async move {
        let application_id = application.id.clone();
        let result = get_system_attributes(application.administration_uri()).await;
        apply_probe_result(&context, &application_id, result).await;
    });
}

async fn apply_probe_result(
    context: &State,
    application_id: &str,
    result: Result<SystemProbe, ProbeError>,
) {
    let state;
    let mut probe = None;

    match result {
        Ok(result) => {
            state = probed_state(&result.capabilities);
            probe = Some(result);
        }
        Err(ProbeError::AuthenticationRequired) => {
            state = PrinterApplicationState::AuthenticationRequired;
        }
        Err(ProbeError::Unreachable { why }) => {
            tracing::warn!(
                application_id,
                why,
                "printer application system probe was unreachable"
            );
            state = PrinterApplicationState::Unreachable;
        }
        Err(ProbeError::Failed { why }) => {
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

/// Re-probes an application whose capabilities are missing or were never
/// established.
pub(super) async fn reprobe_if_needed(
    context: &State,
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
    let result = get_system_attributes(application.administration_uri()).await;
    let scan_state = match &result {
        Ok(_) => None,
        Err(ProbeError::AuthenticationRequired) => {
            Some(PrinterApplicationScanState::AuthenticationRequired)
        }
        Err(ProbeError::Unreachable { .. }) => Some(PrinterApplicationScanState::Unreachable),
        Err(ProbeError::Failed { .. }) => Some(PrinterApplicationScanState::Failed),
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

pub(super) fn probe_failure_id(application_id: &str) -> String {
    application_id.to_string()
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

    fn probe(operations: Vec<u16>) -> SystemProbe {
        SystemProbe {
            make_and_model: Some("Example Application".into()),
            endpoints: Vec::new(),
            capabilities: PrinterApplicationCapabilities::from_operations(operations),
        }
    }

    /// Find-Devices, Find-Drivers, and Create-Printer together.
    const AUTOMATIC_OPERATIONS: [u16; 3] = [0x402b, 0x402c, 0x004c];

    #[tokio::test]
    async fn full_capability_support_marks_application_ready() {
        let context = State::new();
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

    #[tokio::test]
    async fn discovery_without_creation_is_marked_discovery_only() {
        let context = State::new();
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
        let context = State::new();
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
        let context = State::new();
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
                ProbeError::AuthenticationRequired,
                PrinterApplicationState::AuthenticationRequired,
            ),
            (
                ProbeError::Unreachable {
                    why: "unreachable".into(),
                },
                PrinterApplicationState::Unreachable,
            ),
            (
                ProbeError::Failed {
                    why: "failed".into(),
                },
                PrinterApplicationState::Failed,
            ),
        ] {
            let context = State::new();
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
