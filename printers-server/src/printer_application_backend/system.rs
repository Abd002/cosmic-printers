use cosmic_settings_printers_core::Error;
use cups_rs::{IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

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

pub(super) enum ProbeError {
    AuthenticationRequired,
    Unreachable,
    Failed,
}

pub(super) async fn get_system_attributes(system_uri: String) -> Result<SystemProbe, ProbeError> {
    tokio::task::spawn_blocking(move || get_system_attributes_blocking(&system_uri))
        .await
        .map_err(|_| ProbeError::Failed)?
}

fn get_system_attributes_blocking(system_uri: &str) -> Result<SystemProbe, ProbeError> {
    let mut request = IppRequest::new(IppOperation::GetSystemAttributes)
        .cups_err()
        .map_err(|_| ProbeError::Failed)?;
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "system-uri",
            system_uri,
        )
        .cups_err()
        .map_err(|_| ProbeError::Failed)?;
    add_requesting_user(&mut request).map_err(|_| ProbeError::Failed)?;
    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            SYSTEM_ATTRIBUTES,
        )
        .cups_err()
        .map_err(|_| ProbeError::Failed)?;

    let response = send_ipp_request(request, system_uri).map_err(|error| match error {
        Error::DeviceUnreachable { .. } => ProbeError::Unreachable,
        Error::PermissionDenied { .. } => ProbeError::AuthenticationRequired,
        _ => ProbeError::Failed,
    })?;

    match response.status() {
        status if status.is_successful() => {}
        IppStatus::ErrorNotAuthorized
        | IppStatus::ErrorForbidden
        | IppStatus::ErrorNotAuthenticated => return Err(ProbeError::AuthenticationRequired),
        _ => return Err(ProbeError::Failed),
    }

    let operations = response
        .find_attribute("operations-supported", None)
        .ok_or(ProbeError::Failed)?;
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

fn optional_string(response: &cups_rs::IppResponse, name: &str) -> Option<String> {
    response
        .find_attribute(name, None)
        .and_then(|attribute| attribute.get_string(0))
        .filter(|value| !value.trim().is_empty())
}
