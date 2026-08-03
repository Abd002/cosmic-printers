//! Building a request, and reading whether the answer was one.

use cups_rs::{IppOperation, IppRequest, IppResponse, IppStatus, IppTag, IppValueTag};

use super::uri::is_ipp_uri;
use crate::error::{BackendError, BackendResult};

pub(crate) trait CupsResultExt<T> {
    fn cups_err(self) -> BackendResult<T>;
}

impl<T> CupsResultExt<T> for cups_rs::Result<T> {
    fn cups_err(self) -> BackendResult<T> {
        self.map_err(BackendError::Cups)
    }
}

pub(crate) fn add_requesting_user(request: &mut IppRequest) -> BackendResult<()> {
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Name,
            "requesting-user-name",
            &cups_rs::config::get_user(),
        )
        .cups_err()
}

pub(crate) fn ensure_success(response: &IppResponse, operation: &str) -> BackendResult<()> {
    let status = response.status();
    if status.is_successful() {
        Ok(())
    } else {
        match status {
            IppStatus::ErrorNotAuthorized
            | IppStatus::ErrorForbidden
            | IppStatus::ErrorNotAuthenticated => Err(BackendError::PermissionDenied {
                operation: operation.to_string(),
            }),
            _ => Err(BackendError::IppStatus {
                operation: operation.to_string(),
                status: format!("{status:?}"),
            }),
        }
    }
}

pub(crate) fn printer_attrs_request(
    printer_uri: &str,
    requested_attrs: &[&str],
) -> BackendResult<IppRequest> {
    if !is_ipp_uri(printer_uri) {
        return Err(BackendError::Internal(format!(
            "invalid IPP URI: {printer_uri}"
        )));
    }
    let mut request = IppRequest::new(IppOperation::GetPrinterAttributes).cups_err()?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            printer_uri,
        )
        .cups_err()?;
    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            requested_attrs,
        )
        .cups_err()?;

    Ok(request)
}
