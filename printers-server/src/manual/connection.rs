//! Which device URI a Printer Application is given for a typed address.

use cosmic_settings_printers_core::PrinterApplication;

use super::inspect::{EndpointKind, EndpointReport, FoundPrinter, RawPort, host_in_uri};

/// What a Printer Application is asked to create for a typed address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Connection {
    pub(super) device_uri: String,
    /// The printer's own device ID, which lets the application pick pcl or ps for it.
    pub(super) device_id: Option<String>,
    /// What the new printer is named after.
    pub(super) name: String,
}

/// Picks the first way to reach the printer that the application takes.
pub(super) fn for_application(
    report: &EndpointReport,
    application: &PrinterApplication,
) -> Option<Connection> {
    let printer = match (report.kind, report.printers.as_slice()) {
        (EndpointKind::RawOnly | EndpointKind::Silent, _) | (_, []) => None,
        (_, [only]) => Some(only),
        _ => return None,
    };

    // Only a scheme the application lists is offered; one that lists none takes nothing here.
    let device_uri = routes(report, printer).into_iter().find(|uri| {
        application
            .capabilities
            .device_uri_schemes_supported
            .iter()
            .any(|scheme| scheme == scheme_of(uri))
    });

    device_uri.map(|device_uri| Connection {
        device_uri,
        device_id: printer.and_then(|printer| printer.device_id.clone()),
        name: printer
            .map(|printer| printer.name.clone())
            .unwrap_or_else(|| format!("printer_{}", report.target.host)),
    })
}

/// The ways to reach the printer, best first: its own IPP URIs, then each raw port that answered,
/// the typed one first.
fn routes(report: &EndpointReport, printer: Option<&FoundPrinter>) -> Vec<String> {
    let host = host_in_uri(&report.target.host);

    let mut routes = Vec::new();
    if let Some(printer) = printer {
        routes.extend(printer.uris.iter().cloned());
        if !routes.contains(&printer.uri) {
            routes.push(printer.uri.clone());
        }
    }
    routes.extend(report.raw_ports_open.iter().map(|raw| match raw {
        RawPort::AppSocket(port) => format!("socket://{host}:{port}"),
        // LPD's own port stays implied, as in every LPD URI.
        RawPort::Lpd(515) => format!("lpd://{host}/raw"),
        RawPort::Lpd(port) => format!("lpd://{host}:{port}/raw"),
    }));

    routes
}

fn scheme_of(uri: &str) -> &str {
    uri.split(':').next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::target::parse_target;
    use cosmic_settings_printers_core::{PrinterApplicationCapabilities, PrinterApplicationState};
    use std::collections::BTreeMap;

    fn printer(uri: &str) -> FoundPrinter {
        FoundPrinter {
            name: "Office".into(),
            uri: uri.into(),
            uris: vec![uri.into()],
            make_and_model: None,
            info: None,
            device_id: Some("MFG:HP;MDL:LaserJet 4050;CMD:PCL,POSTSCRIPT;".into()),
            formats: Vec::new(),
            ipp_features: Vec::new(),
        }
    }

    fn report(
        input: &str,
        kind: EndpointKind,
        printers: Vec<FoundPrinter>,
        raw_ports_open: Vec<RawPort>,
    ) -> EndpointReport {
        EndpointReport {
            target: parse_target(input).expect("a target"),
            kind,
            printers,
            raw_ports_open,
        }
    }

    fn application(schemes: &[&str]) -> PrinterApplication {
        PrinterApplication {
            id: "app".into(),
            service_name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: "localhost".into(),
            port: 8000,
            addresses: vec!["127.0.0.1".into()],
            system_uri: "ipps://localhost:8000/ipp/system".into(),
            make_and_model: None,
            web_interface_uri: None,
            endpoints: Vec::new(),
            capabilities: PrinterApplicationCapabilities {
                device_uri_schemes_supported: schemes.iter().map(ToString::to_string).collect(),
                ..PrinterApplicationCapabilities::default()
            },
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Ready,
        }
    }

    #[test]
    fn the_printers_own_ipp_uri_comes_before_appsocket() {
        let report = report(
            "192.0.2.10",
            EndpointKind::IppPrinter,
            vec![printer("ipp://192.0.2.10:631/ipp/print")],
            vec![RawPort::AppSocket(9100)],
        );
        let connection =
            for_application(&report, &application(&["ipp", "socket"])).expect("a connection");

        assert_eq!(connection.device_uri, "ipp://192.0.2.10:631/ipp/print");
        assert_eq!(connection.name, "Office");
        assert!(connection.device_id.is_some());
    }

    #[test]
    fn an_application_without_ipp_gets_appsocket_and_the_printers_device_id() {
        let report = report(
            "192.0.2.10",
            EndpointKind::IppPrinter,
            vec![printer("ipp://192.0.2.10:631/ipp/print")],
            vec![RawPort::AppSocket(9100)],
        );
        let connection =
            for_application(&report, &application(&["socket", "usb"])).expect("a connection");

        assert_eq!(connection.device_uri, "socket://192.0.2.10:9100");
        assert!(connection.device_id.is_some());
    }

    #[test]
    fn a_raw_only_printer_is_named_after_its_host() {
        let report = report(
            "192.0.2.10",
            EndpointKind::RawOnly,
            Vec::new(),
            vec![RawPort::AppSocket(9100)],
        );
        let connection = for_application(&report, &application(&["socket"])).expect("a connection");

        assert_eq!(connection.device_uri, "socket://192.0.2.10:9100");
        assert_eq!(connection.name, "printer_192.0.2.10");
        assert_eq!(connection.device_id, None);
    }

    #[test]
    fn a_print_server_port_that_was_typed_is_used_before_9100() {
        let report = report(
            "192.0.2.10:9101",
            EndpointKind::RawOnly,
            Vec::new(),
            vec![RawPort::AppSocket(9101), RawPort::AppSocket(9100)],
        );

        assert_eq!(
            for_application(&report, &application(&["socket"]))
                .map(|connection| connection.device_uri),
            Some("socket://192.0.2.10:9101".to_string())
        );
    }

    #[test]
    fn an_ipv6_host_is_bracketed_in_a_device_uri() {
        let report = report(
            "[fe80::1]:9100",
            EndpointKind::RawOnly,
            Vec::new(),
            vec![RawPort::AppSocket(9100)],
        );

        assert_eq!(
            for_application(&report, &application(&["socket"]))
                .map(|connection| connection.device_uri),
            Some("socket://[fe80::1]:9100".to_string())
        );
    }

    #[test]
    fn lpd_is_used_only_when_its_port_answers_and_the_application_takes_it() {
        let report = report(
            "192.0.2.10",
            EndpointKind::RawOnly,
            Vec::new(),
            vec![RawPort::AppSocket(9100), RawPort::Lpd(515)],
        );

        assert_eq!(
            for_application(&report, &application(&["lpd"]))
                .map(|connection| connection.device_uri),
            Some("lpd://192.0.2.10/raw".to_string())
        );
    }

    #[test]
    fn an_application_listing_no_matching_scheme_gets_nothing() {
        let report = report(
            "192.0.2.10",
            EndpointKind::RawOnly,
            Vec::new(),
            vec![RawPort::AppSocket(9100)],
        );

        assert_eq!(for_application(&report, &application(&["usb"])), None);
        assert_eq!(for_application(&report, &application(&[])), None);
    }

    #[test]
    fn several_printers_give_no_connection() {
        let report = report(
            "192.0.2.10",
            EndpointKind::CupsServer,
            vec![
                printer("ipp://192.0.2.10:631/printers/a"),
                printer("ipp://192.0.2.10:631/printers/b"),
            ],
            Vec::new(),
        );

        assert_eq!(for_application(&report, &application(&["ipp"])), None);
    }
}
