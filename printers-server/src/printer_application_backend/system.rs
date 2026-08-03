use cups_rs::{IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

use crate::error::BackendError;
use crate::ipp::{CupsResultExt, add_requesting_user, send_ipp_request};

const SYSTEM_ATTRIBUTES: &[&str] = &[
    "system-uuid",
    "system-name",
    "system-make-and-model",
    "operations-supported",
    "system-xri-supported",
];

pub(super) struct SystemProbe {
    pub system_uuid: Option<String>,
    pub make_and_model: Option<String>,
    pub operations_supported: Vec<u16>,
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
        BackendError::DeviceUnreachable { .. } => ProbeError::Unreachable {
            why: error.to_string(),
        },
        BackendError::PermissionDenied { .. } => ProbeError::AuthenticationRequired,
        _ => probe_failed(error),
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

    Ok(SystemProbe {
        system_uuid: optional_string(&response, "system-uuid"),
        make_and_model: optional_string(&response, "system-make-and-model"),
        operations_supported,
    })
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
        .filter(|value| !value.trim().is_empty())
}
