//! Web addresses for a Printer Application and the printers it created.
//!
//! Three different URIs are easy to confuse, and only one of them is safe to open
//! in a browser:
//!
//! - The IPP System Service, `ipp(s)://host:port/ipp/system`. Used to talk to the
//!   application; never opened.
//! - The application's own administration page, `http(s)://host:port/`. This is
//!   what Manual Setup opens.
//! - A configured printer's page, reported by that printer as
//!   `printer-more-info`.
//!
//! Only `http` and `https` are ever returned, so a URI a Printer Application
//! reported cannot become a request to open something else.

use cosmic_settings_printers_core::PrinterApplication;
use url::Url;

/// Returns the Printer Application's administration page.
///
/// Derived from the advertised endpoint rather than from a reported attribute,
/// because the endpoint is what discovery actually verified.
///
/// A local application gets a plain page on loopback. It advertises
/// `_ipps-system._tcp` but negotiates TLS by HTTP upgrade rather than serving it,
/// so a browser opening `https://` on that port gets nothing at all; and its own
/// hostname needs mDNS resolution the browser may not have, while loopback always
/// resolves. On loopback the traffic never leaves the machine either.
///
/// A remote application keeps its advertised host and the scheme its service type
/// implies. Its pages cannot be verified from here, and moving a remote
/// conversation off TLS is not something to do on a guess.
pub(crate) fn application_web_interface(application: &PrinterApplication) -> Option<String> {
    if application.is_local() {
        return Some(format!("http://localhost:{}/", application.port));
    }

    let host = application.hostname.trim().trim_end_matches('.');
    if host.is_empty() {
        return None;
    }

    let scheme = if application.system_uri.starts_with("ipps") {
        "https"
    } else {
        "http"
    };
    let mut url = Url::parse(&format!("{scheme}://placeholder/")).ok()?;
    url.set_host(Some(host)).ok()?;
    url.set_port(Some(application.port)).ok()?;

    Some(url.to_string())
}

/// Accepts a web URI a Printer Application reported, or rejects it.
///
/// A reported value is data from another process, so it is parsed and its scheme
/// checked rather than passed through. Anything that is not `http` or `https` is
/// refused, including schemes that would launch something locally.
pub(crate) fn validate_web_interface(uri: &str) -> Option<String> {
    let url = Url::parse(uri.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().is_none_or(str::is_empty) {
        return None;
    }

    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::{PrinterApplicationCapabilities, PrinterApplicationState};
    use std::collections::BTreeMap;

    fn application(system_uri: &str, hostname: &str, port: u16) -> PrinterApplication {
        let addresses = if cosmic_settings_printers_core::host_is_local(hostname) {
            vec!["127.0.0.1".into()]
        } else {
            vec!["192.0.2.10".into()]
        };

        PrinterApplication {
            id: "app".into(),
            service_name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: hostname.into(),
            port,
            addresses,
            system_uri: system_uri.into(),
            make_and_model: None,
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: PrinterApplicationCapabilities::default(),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Ready,
        }
    }

    /// A local application advertises the secure service type but serves plain
    /// pages, so its page is plain and on loopback.
    #[test]
    fn a_local_application_gets_a_plain_loopback_page() {
        let application = application("ipps://desktop.local:8000/ipp/system", "localhost", 8000);

        assert_eq!(
            application_web_interface(&application).as_deref(),
            Some("http://localhost:8000/")
        );
    }

    #[test]
    fn a_remote_secure_service_keeps_its_host_and_scheme() {
        let application = application(
            "ipps://printer.local:8000/ipp/system",
            "printer.local",
            8000,
        );

        assert_eq!(
            application_web_interface(&application).as_deref(),
            Some("https://printer.local:8000/")
        );
    }

    /// The page is the root, not the system service path: opening `/ipp/system`
    /// in a browser is not a setup interface.
    #[test]
    fn the_page_is_never_the_system_service_path() {
        let application = application(
            "ipps://printer.local:8000/ipp/system",
            "printer.local",
            8000,
        );
        let page = application_web_interface(&application).expect("a page");

        assert!(!page.contains("/ipp/system"));
        assert_ne!(page, application.system_uri);
    }

    #[test]
    fn an_application_without_a_hostname_has_no_page() {
        let application = application("ipp://:8000/ipp/system", "  ", 8000);

        assert_eq!(application_web_interface(&application), None);
    }

    #[test]
    fn only_web_schemes_are_accepted() {
        assert!(validate_web_interface("http://printer.local:8000/").is_some());
        assert!(validate_web_interface(" https://printer.local/admin ").is_some());
        assert!(validate_web_interface("ipp://printer.local/ipp/system").is_none());
        assert!(validate_web_interface("file:///etc/passwd").is_none());
        assert!(validate_web_interface("javascript:alert(1)").is_none());
        assert!(validate_web_interface("not a uri").is_none());
    }
}
