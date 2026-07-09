use cosmic_settings_printers_core::Error;
use cups_rs::{
    HttpConnection, IppOperation, IppRequest, IppResponse, IppStatus, IppTag, IppValueTag,
};

use super::options::{is_local_scheduler_uri, parse_uri_endpoint, uri_resource_path};

const LOCAL_CUPS_SOCKET: &str = "/run/cups/cups.sock";

pub(in crate::cups_backend) struct LocalSocketGuard {
    previous: String,
}

impl LocalSocketGuard {
    pub(in crate::cups_backend) fn engage() -> Result<Self, Error> {
        let previous = cups_rs::config::get_server();
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).cups_err()?;
        Ok(Self { previous })
    }
}

impl Drop for LocalSocketGuard {
    fn drop(&mut self) {
        let _ = cups_rs::config::set_server(Some(&self.previous));
    }
}

pub(in crate::cups_backend) trait CupsResultExt<T> {
    fn cups_err(self) -> Result<T, Error>;
}

impl<T, E: std::fmt::Display> CupsResultExt<T> for std::result::Result<T, E> {
    fn cups_err(self) -> Result<T, Error> {
        self.map_err(|error| Error::CupsFailed {
            why: error.to_string(),
        })
    }
}

/// Adds the current CUPS user to an IPP request.
pub(in crate::cups_backend) fn add_requesting_user(request: &mut IppRequest) -> Result<(), Error> {
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Name,
            "requesting-user-name",
            &cups_rs::config::get_user(),
        )
        .cups_err()
}

/// Converts an IPP response status into the backend result.
pub(in crate::cups_backend) fn ensure_success(
    response: &IppResponse,
    operation: &str,
) -> Result<(), Error> {
    let status = response.status();
    if status.is_successful() {
        Ok(())
    } else {
        match status {
            IppStatus::ErrorNotAuthorized
            | IppStatus::ErrorForbidden
            | IppStatus::ErrorNotAuthenticated => Err(Error::PermissionDenied {
                operation: operation.to_string(),
            }),
            _ => Err(Error::CupsFailed {
                why: format!("{operation} failed with status {status:?}"),
            }),
        }
    }
}

pub(in crate::cups_backend) fn is_ipp_uri(uri: &str) -> bool {
    uri.starts_with("ipp://") || uri.starts_with("ipps://")
}

fn ipp_endpoint(uri: &str) -> Result<(String, u16, String), Error> {
    if !is_ipp_uri(uri) {
        return Err(Error::Internal {
            why: format!("not an IPP URI: {uri}"),
        });
    }

    let (host, port) = parse_uri_endpoint(uri).ok_or_else(|| Error::Internal {
        why: format!("invalid IPP URI endpoint: {uri}"),
    })?;
    let resource = uri_resource_path(uri).ok_or_else(|| Error::Internal {
        why: format!("invalid IPP URI resource: {uri}"),
    })?;

    Ok((host, port, resource))
}

pub(in crate::cups_backend) fn send_ipp_request_to_printer_uri(
    request: IppRequest,
    printer_uri: &str,
) -> Result<IppResponse, Error> {
    let (_, _, resource) = ipp_endpoint(printer_uri)?;

    if is_local_scheduler_uri(printer_uri) {
        request.send_default(&resource).cups_err()
    } else {
        let (host, port, resource) = ipp_endpoint(printer_uri)?;
        let connection =
            HttpConnection::connect_host(&host, port, &resource, Some(250)).map_err(|error| {
                Error::DeviceUnreachable {
                    why: format!("{printer_uri}: {error}"),
                }
            })?;
        request
            .send(&connection, connection.resource_path())
            .cups_err()
    }
}

/// Builds a raw Get-Printer-Attributes request for selected attribute names.
pub(super) fn printer_attrs_request(
    printer_uri: &str,
    requested_attrs: &[&str],
) -> Result<IppRequest, Error> {
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
