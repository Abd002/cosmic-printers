use cosmic_settings_printers_core::{PrinterApplicationCapabilities, SystemEndpoint};
use cups_rs::{IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

use crate::error::BackendError;
use crate::ipp::{CupsResultExt, add_requesting_user, send_ipp_request};

/// Attributes worth asking a Printer Application about.
///
/// `system-uuid` is deliberately not requested. Several Printer Applications on
/// one machine can report the same value, so it cannot identify one and there is
/// no other use for it here.
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

    let response = send_ipp_request(request, system_uri).map_err(|error| match error {
        BackendError::PermissionDenied { .. } => ProbeError::AuthenticationRequired,
        // Anything else at this point happened while talking to the application:
        // the request was built successfully, so a failure here is the connection
        // refusing, timing out, or closing before answering. All of those mean
        // "try again later", not "this application is broken".
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

/// Reads the endpoints from `system-xri-supported`.
///
/// Each value is a collection describing one way to reach the system service,
/// with the URI plus the authentication and transport security it expects. An
/// entry with no usable URI is dropped rather than guessed at.
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
