//! The Printer Applications that can set up a printer at a typed address manually.

use cosmic_settings_printers_core::{ManualSetupPrinterApplication, PrinterApplication};

use super::connection::{self, Connection};
use super::inspect::EndpointReport;
use crate::printer_app;
use crate::state::State;

/// Lists the Printer Applications that can create the printer at the address themselves.
/// Any other one could only send the user to its web interface to type the address again.
pub(super) async fn reaching(
    context: &State,
    report: &EndpointReport,
) -> Vec<ManualSetupPrinterApplication> {
    context
        .printer_applications_cached()
        .await
        .iter()
        .filter(|application| connection(application, report).is_some())
        .filter_map(printer_app::manual_setup_application)
        .collect()
}

/// What the application would be asked to create, when it can create the printer itself.
pub(super) fn connection(
    application: &PrinterApplication,
    report: &EndpointReport,
) -> Option<Connection> {
    if !application.supports_automatic_configuration() {
        return None;
    }

    connection::for_application(report, application)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::inspect::{EndpointKind, RawPort};
    use crate::manual::target::parse_target;
    use cosmic_settings_printers_core::{PrinterApplicationCapabilities, PrinterApplicationState};
    use std::collections::BTreeMap;

    fn raw_only() -> EndpointReport {
        EndpointReport {
            target: parse_target("192.0.2.10").expect("a target"),
            kind: EndpointKind::RawOnly,
            printers: Vec::new(),
            raw_ports_open: vec![RawPort::AppSocket(9100)],
        }
    }

    fn application(hostname: &str, create_printer: bool, schemes: &[&str]) -> PrinterApplication {
        PrinterApplication {
            id: "app".into(),
            service_name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: hostname.into(),
            port: 8000,
            addresses: Vec::new(),
            system_uri: "ipps://localhost:8000/ipp/system".into(),
            make_and_model: None,
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: PrinterApplicationCapabilities {
                find_devices: true,
                find_drivers: true,
                create_printer,
                device_uri_schemes_supported: schemes.iter().map(ToString::to_string).collect(),
                ..PrinterApplicationCapabilities::default()
            },
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Ready,
        }
    }

    #[test]
    fn a_local_application_taking_the_scheme_creates_the_printer() {
        let connection = connection(&application("localhost", true, &["socket"]), &raw_only());

        assert_eq!(
            connection.map(|connection| connection.device_uri),
            Some("socket://192.0.2.10:9100".to_string())
        );
    }

    #[test]
    fn an_application_on_another_host_is_hidden() {
        assert_eq!(
            connection(&application("office.local", true, &["socket"]), &raw_only()),
            None
        );
    }

    #[test]
    fn an_application_without_create_printer_is_hidden() {
        assert_eq!(
            connection(&application("localhost", false, &["socket"]), &raw_only()),
            None
        );
    }

    #[test]
    fn an_application_not_taking_the_scheme_is_hidden() {
        assert_eq!(
            connection(&application("localhost", true, &["usb"]), &raw_only()),
            None
        );
        assert_eq!(
            connection(&application("localhost", true, &[]), &raw_only()),
            None
        );
    }
}
