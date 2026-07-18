use cosmic_settings_printers_core::Error;
use cups_rs::{
    HttpConnection, IppOperation, IppRequest, IppResponse, IppStatus, IppTag, IppValueTag,
    config::EncryptionMode,
};
use std::net::IpAddr;

pub(crate) trait CupsResultExt<T> {
    fn cups_err(self) -> Result<T, Error>;
}

impl<T, E: std::fmt::Display> CupsResultExt<T> for std::result::Result<T, E> {
    fn cups_err(self) -> Result<T, Error> {
        self.map_err(|error| Error::CupsFailed {
            why: error.to_string(),
        })
    }
}

pub(crate) fn add_requesting_user(request: &mut IppRequest) -> Result<(), Error> {
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Name,
            "requesting-user-name",
            &cups_rs::config::get_user(),
        )
        .cups_err()
}

pub(crate) fn ensure_success(response: &IppResponse, operation: &str) -> Result<(), Error> {
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

pub(crate) fn is_ipp_uri(uri: &str) -> bool {
    uri.starts_with("ipp://") || uri.starts_with("ipps://")
}

pub(crate) fn parse_uri_endpoint(uri: &str) -> Option<(String, u16)> {
    let (scheme, rest) = uri.split_once("://")?;
    let authority = rest.split('/').next()?.rsplit('@').next()?.trim();
    if authority.is_empty() {
        return None;
    }

    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ipp" | "ipps" => 631,
        "http" => 80,
        "https" => 443,
        _ => return None,
    };

    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let port = authority
            .get(end + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Some((host.to_ascii_lowercase(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse::<u16>().ok()),
        _ => (authority, Some(default_port)),
    };

    Some((host.to_ascii_lowercase(), port?))
}

pub(crate) fn uri_resource_path(uri: &str) -> Option<String> {
    let (_, rest) = uri.split_once("://")?;
    let path = rest
        .find('/')
        .map(|index| &rest[index..])
        .unwrap_or("/")
        .split(['?', '#'])
        .next()
        .unwrap_or("/");

    Some(if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    })
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);

    bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn is_local_scheduler_uri(uri: &str) -> bool {
    let Some((host, _)) = parse_uri_endpoint(uri) else {
        return false;
    };
    let resource = uri_resource_path(uri).unwrap_or_default();

    is_loopback_host(&host)
        && (resource == "/"
            || resource.starts_with("/printers/")
            || resource.starts_with("/classes/"))
}

fn encryption_for_uri(uri: &str) -> EncryptionMode {
    if uri.starts_with("ipps://") {
        EncryptionMode::Always
    } else {
        EncryptionMode::IfRequested
    }
}

pub(crate) fn send_ipp_request(request: IppRequest, uri: &str) -> Result<IppResponse, Error> {
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

    if is_local_scheduler_uri(uri) {
        request.send_default(&resource).cups_err()
    } else {
        let connection = HttpConnection::connect_host_with_encryption(
            &host,
            port,
            &resource,
            encryption_for_uri(uri),
            Some(250),
        )
        .map_err(|error| Error::DeviceUnreachable {
            why: format!("{uri}: {error}"),
        })?;
        request
            .send(&connection, connection.resource_path())
            .cups_err()
    }
}

pub(crate) fn printer_attrs_request(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_ipp_endpoint_and_resource() {
        let uri = "ipps://printer.local:8000/ipp/system";
        assert_eq!(
            parse_uri_endpoint(uri),
            Some(("printer.local".to_string(), 8000))
        );
        assert_eq!(uri_resource_path(uri).as_deref(), Some("/ipp/system"));
    }

    #[test]
    fn requires_tls_for_ipps() {
        assert_eq!(
            encryption_for_uri("ipps://printer.local/ipp/system"),
            EncryptionMode::Always
        );
        assert_eq!(
            encryption_for_uri("ipp://printer.local/ipp/print"),
            EncryptionMode::IfRequested
        );
    }
}
