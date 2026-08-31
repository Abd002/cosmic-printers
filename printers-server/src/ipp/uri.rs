//! Reading and rewriting the URIs an IPP conversation is addressed by.

use std::net::IpAddr;
use url::Url;

use cups_rs::config::EncryptionMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NetworkScheme {
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

    pub(super) fn is_ipp(self) -> bool {
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

pub(super) struct ParsedUri {
    pub(super) uri: Url,
    pub(super) scheme: NetworkScheme,
    pub(super) host: String,
    pub(super) port: u16,
}

impl ParsedUri {
    pub(super) fn parse(value: &str) -> Option<Self> {
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

    pub(super) fn resource_path(&self) -> &str {
        let path = self.uri.path();
        if path.is_empty() { "/" } else { path }
    }

    pub(super) fn encryption(&self) -> EncryptionMode {
        if self.scheme == NetworkScheme::Ipps {
            EncryptionMode::Always
        } else {
            EncryptionMode::IfRequested
        }
    }

    pub(super) fn is_local_scheduler(&self) -> bool {
        let resource = self.resource_path();
        self.scheme == NetworkScheme::Ipp
            && self.port == 631
            && is_loopback_host(&self.host)
            && (resource == "/"
                || resource == "/jobs"
                // Scheduler `/admin` requests need the default Unix-socket connection.
                || resource == "/admin"
                || resource == "/admin/"
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

/// Rewrites a URI to address the same service over loopback.
pub(crate) fn loopback_uri(uri: &str) -> Option<String> {
    let parsed = ParsedUri::parse(uri).filter(|parsed| parsed.scheme.is_ipp())?;
    let mut local = parsed.uri.clone();

    local.set_host(Some("localhost")).ok()?;
    // `set_host` drops a port that matches the scheme default, and these rarely use it.
    local.set_port(Some(parsed.port)).ok()?;

    Some(local.to_string())
}

/// Returns the `/ipp/system` URI used for service-level printer operations.
pub(crate) fn system_service_uri(printer_uri: &str) -> Option<String> {
    let parsed = ParsedUri::parse(printer_uri).filter(|parsed| parsed.scheme.is_ipp())?;
    let mut system = parsed.uri.clone();

    system.set_path("/ipp/system");
    system.set_query(None);
    system.set_fragment(None);

    Some(system.to_string())
}

pub(super) fn is_ipp_uri(uri: &str) -> bool {
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

pub(super) fn is_loopback_host(host: &str) -> bool {
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
