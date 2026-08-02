use cups_rs::{
    HttpConnection, IppOperation, IppRequest, IppResponse, IppStatus, IppTag, IppValueTag,
    config::EncryptionMode,
};
use std::net::IpAddr;
use url::Url;

use crate::error::{BackendError, BackendResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NetworkScheme {
    Ipp,
    Ipps,
    Http,
    Https,
}

impl NetworkScheme {
    fn parse(scheme: &str) -> Option<Self> {
        if scheme.eq_ignore_ascii_case("ipp") {
            Some(Self::Ipp)
        } else if scheme.eq_ignore_ascii_case("ipps") {
            Some(Self::Ipps)
        } else if scheme.eq_ignore_ascii_case("http") {
            Some(Self::Http)
        } else if scheme.eq_ignore_ascii_case("https") {
            Some(Self::Https)
        } else {
            None
        }
    }

    fn default_port(self) -> u16 {
        match self {
            Self::Ipp | Self::Ipps => 631,
            Self::Http => 80,
            Self::Https => 443,
        }
    }

    fn is_ipp(self) -> bool {
        matches!(self, Self::Ipp | Self::Ipps)
    }

    #[cfg(test)]
    fn web_scheme(self) -> &'static str {
        match self {
            Self::Ipp | Self::Http => "http",
            Self::Ipps | Self::Https => "https",
        }
    }
}

struct ParsedUri {
    uri: Url,
    scheme: NetworkScheme,
    host: String,
    port: u16,
}

impl ParsedUri {
    fn parse(value: &str) -> Option<Self> {
        let uri = Url::parse(value).ok()?;
        let scheme = NetworkScheme::parse(uri.scheme())?;
        let host = uri.host_str()?.to_ascii_lowercase();
        if host.is_empty() {
            return None;
        }
        let port = uri.port().unwrap_or_else(|| scheme.default_port());

        Some(Self {
            uri,
            scheme,
            host,
            port,
        })
    }

    fn resource_path(&self) -> &str {
        let path = self.uri.path();
        if path.is_empty() { "/" } else { path }
    }

    fn encryption(&self) -> EncryptionMode {
        if self.scheme == NetworkScheme::Ipps {
            EncryptionMode::Always
        } else {
            EncryptionMode::IfRequested
        }
    }

    fn is_local_scheduler(&self) -> bool {
        let resource = self.resource_path();
        self.scheme == NetworkScheme::Ipp
            && self.port == 631
            && is_loopback_host(&self.host)
            && (resource == "/"
                || resource == "/jobs"
                || resource.starts_with("/printers/")
                || resource.starts_with("/classes/"))
    }

    #[cfg(test)]
    fn web_page(&self) -> Option<String> {
        let mut web_page = Url::parse("http://localhost/").ok()?;
        web_page.set_scheme(self.scheme.web_scheme()).ok()?;
        web_page.set_host(self.uri.host_str()).ok()?;
        web_page.set_port(self.uri.port()).ok()?;
        Some(web_page.to_string())
    }
}

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

pub(crate) fn is_ipp_uri(uri: &str) -> bool {
    ParsedUri::parse(uri).is_some_and(|uri| uri.scheme.is_ipp())
}

pub(crate) fn parse_uri_endpoint(uri: &str) -> Option<(String, u16)> {
    let uri = ParsedUri::parse(uri)?;
    Some((uri.host, uri.port))
}

#[cfg(test)]
fn web_page_from_uri(uri: &str) -> Option<String> {
    ParsedUri::parse(uri)?.web_page()
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

pub(crate) fn is_local_scheduler_uri(uri: &str) -> bool {
    ParsedUri::parse(uri).is_some_and(|uri| uri.is_local_scheduler())
}

pub(crate) fn send_ipp_request(request: IppRequest, uri: &str) -> BackendResult<IppResponse> {
    let uri_parts = ParsedUri::parse(uri)
        .filter(|uri| uri.scheme.is_ipp())
        .ok_or_else(|| BackendError::Internal(format!("invalid IPP URI: {uri}")))?;
    let resource = uri_parts.resource_path();

    if uri_parts.is_local_scheduler() {
        request.send_default(resource).cups_err()
    } else {
        let connection = HttpConnection::connect_host_with_encryption(
            &uri_parts.host,
            uri_parts.port,
            resource,
            uri_parts.encryption(),
            Some(250),
        )
        .map_err(|source| BackendError::DeviceUnreachable {
            uri: uri.to_string(),
            source,
        })?;
        request
            .send(&connection, connection.resource_path())
            .cups_err()
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
        assert_eq!(
            ParsedUri::parse(uri).unwrap().resource_path(),
            "/ipp/system"
        );
    }

    #[test]
    fn recognizes_schemes_case_insensitively() {
        assert!(is_ipp_uri("IPP://printer.local/ipp/print"));
        assert!(is_ipp_uri("IPPS://printer.local/ipp/print"));
    }

    #[test]
    fn rejects_invalid_ports_and_unbracketed_ipv6() {
        assert_eq!(
            parse_uri_endpoint("ipp://printer.local:not-a-port/ipp/print"),
            None
        );
        assert_eq!(parse_uri_endpoint("ipp://2001:db8::1/ipp/print"), None);
    }

    #[test]
    fn parses_bracketed_ipv6() {
        assert_eq!(
            parse_uri_endpoint("ipps://[2001:db8::1]:8631/ipp/print"),
            Some(("[2001:db8::1]".to_string(), 8631))
        );
    }

    #[test]
    fn requires_tls_for_ipps_case_insensitively() {
        assert_eq!(
            ParsedUri::parse("IPPS://printer.local/ipp/system")
                .unwrap()
                .encryption(),
            EncryptionMode::Always
        );
        assert_eq!(
            ParsedUri::parse("IPP://printer.local/ipp/print")
                .unwrap()
                .encryption(),
            EncryptionMode::IfRequested
        );
    }

    #[test]
    fn detects_only_local_scheduler_resources_on_the_cups_port() {
        assert!(is_local_scheduler_uri("ipp://localhost/printers/example"));
        assert!(is_local_scheduler_uri("ipp://127.0.0.1/"));
        assert!(is_local_scheduler_uri("ipp://localhost/jobs"));
        assert!(!is_local_scheduler_uri("ipps://localhost/printers/example"));
        assert!(!is_local_scheduler_uri(
            "ipp://localhost:8000/printers/example"
        ));
        assert!(!is_local_scheduler_uri("ipp://localhost:8000/ipp/print"));
    }

    #[test]
    fn derives_web_scheme_from_ipp_security() {
        assert_eq!(
            web_page_from_uri("ipp://printer.local:8000/ipp/print").as_deref(),
            Some("http://printer.local:8000/")
        );
        assert_eq!(
            web_page_from_uri("ipps://printer.local:8000/ipp/print").as_deref(),
            Some("https://printer.local:8000/")
        );
    }
}
