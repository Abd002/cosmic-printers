//! Validated HTTP(S) pages for Printer Applications and their printers.

use cosmic_settings_printers_core::{
    ListManualSetupApplicationsReply, ManualSetupPrinterApplication, PrinterApplication,
};

use crate::state::State;
use url::Url;

/// Returns an administration page derived from the verified advertised endpoint.
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

/// Accepts only reported HTTP(S) URIs to prevent launching local URI handlers.
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

/// Lists Printer Applications that can be set up through their own interface.
pub(crate) async fn manual_setup_applications(context: &State) -> ListManualSetupApplicationsReply {
    let printer_applications = context
        .printer_applications_cached()
        .await
        .into_iter()
        .filter_map(|application| {
            let web_interface_uri = application_web_interface(&application)
                .and_then(|uri| validate_web_interface(&uri))?;

            Some(ManualSetupPrinterApplication {
                printer_application_id: application.id.clone(),
                display_name: application
                    .make_and_model
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| application.service_name.clone()),
                web_interface_uri,
                state: application.state,
            })
        })
        .collect();

    ListManualSetupApplicationsReply {
        printer_applications,
    }
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
