use cosmic_settings_printers_core::{EndpointSource, PrinterEntry, PrinterStatus};
use cups_rs::{Destination, PrinterState as CupsPrinterState};

use crate::ipp::{is_local_scheduler_uri, parse_uri_endpoint};

/// Converts a cups-rs destination into the type exposed by the printer API.
pub(in crate::cups) fn destination_to_printer_entry(mut destination: Destination) -> PrinterEntry {
    let queue_status = destination.state().to_string();
    let printer_uri = destination.uri().cloned();
    let device_uri = destination.device_uri().cloned();
    let id = destination.full_name();
    let name = destination
        .info()
        .filter(|info| !info.is_empty())
        .cloned()
        .unwrap_or_else(|| id.clone());
    destination
        .options
        .insert("queue-status".to_string(), queue_status);
    destination.options.insert(
        "printer-state".to_string(),
        match printer_status(&destination) {
            PrinterStatus::Offline => "5",
            PrinterStatus::Ready | PrinterStatus::LowToner => "3",
        }
        .to_string(),
    );
    if !destination.options.contains_key("printer-location")
        && let Some(location) = destination.location()
    {
        destination
            .options
            .insert("printer-location".to_string(), location.clone());
    }
    if !destination.options.contains_key("printer-make-and-model")
        && let Some(model) = destination.make_and_model()
    {
        destination
            .options
            .insert("printer-make-and-model".to_string(), model.clone());
    }
    let mut printer = PrinterEntry::new(id, name, destination.is_default, destination.options);
    apply_endpoint(
        &mut printer,
        endpoint_from_uris(printer_uri.as_deref(), device_uri.as_deref()),
    );
    printer
}

fn endpoint_from_uris(
    printer_uri: Option<&str>,
    device_uri: Option<&str>,
) -> Option<(String, u16)> {
    if printer_uri.is_some_and(is_local_scheduler_uri) {
        return device_uri.and_then(parse_uri_endpoint);
    }

    printer_uri
        .and_then(parse_uri_endpoint)
        .or_else(|| device_uri.and_then(parse_uri_endpoint))
}

/// Recomputes the endpoint after URI attributes are merged.
pub(super) fn refresh_printer_endpoint(printer: &mut PrinterEntry) {
    let device_uri = printer.device_uri().map(str::to_owned);
    apply_endpoint(
        printer,
        endpoint_from_uris(printer.printer_uri(), device_uri.as_deref()),
    );
}

/// Records the printer's web page at the address it was reached on.
pub(super) fn refresh_printer_web_page(printer: &mut PrinterEntry) {
    let Some(web_page) = printer.option("printer-more-info") else {
        return;
    };
    let Some(address) = printer.endpoint_address() else {
        return;
    };
    let Ok(mut page) = url::Url::parse(web_page) else {
        return;
    };

    let names_the_endpoint = page
        .host_str()
        .zip(printer.option("endpoint-hostname"))
        .is_some_and(|(host, endpoint)| host.eq_ignore_ascii_case(endpoint));
    if !names_the_endpoint || !is_mdns_name(page.host_str()) {
        return;
    }

    // Only the host: the port a page is served on is the printer's to choose, and the scheme's
    // default when it names none. The IPP port we connected on has nothing to do with it.
    if page.set_host(Some(&bracketed(address))).is_ok() {
        printer.set_option("web-page", page.as_str());
    }
}

/// Writes an address the way a URI must carry it, which for IPv6 means in brackets.
fn bracketed(address: &str) -> String {
    if address.contains(':') && !address.starts_with('[') {
        return format!("[{address}]");
    }

    address.to_string()
}

/// Returns whether a host is an mDNS name, which is one no ordinary resolver answers.
fn is_mdns_name(host: Option<&str>) -> bool {
    host.is_some_and(|host| {
        host.trim_end_matches('.')
            .to_ascii_lowercase()
            .ends_with(".local")
    })
}

pub(super) fn apply_endpoint(printer: &mut PrinterEntry, endpoint: Option<(String, u16)>) {
    if let Some((host, port)) = endpoint {
        printer.set_option("endpoint-hostname", host);
        printer.set_option("endpoint-port", port.to_string());
        printer.set_endpoint_source(EndpointSource::Uri);
    }
}

/// Maps CUPS state and toner reasons to the UI printer status.
fn printer_status(destination: &Destination) -> PrinterStatus {
    if destination
        .state_reasons()
        .iter()
        .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
    {
        return PrinterStatus::LowToner;
    }

    match destination.state() {
        CupsPrinterState::Idle | CupsPrinterState::Processing => PrinterStatus::Ready,
        CupsPrinterState::Stopped | CupsPrinterState::Unknown => PrinterStatus::Offline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn destination(options: &[(&str, &str)]) -> Destination {
        Destination {
            name: "Test".to_string(),
            instance: None,
            is_default: false,
            options: options
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
        }
    }

    #[test]
    fn leaves_unreported_destination_uris_absent() {
        let printer = destination_to_printer_entry(destination(&[]));

        assert_eq!(printer.printer_uri(), None);
        assert_eq!(printer.device_uri(), None);
    }

    #[test]
    fn preserves_reported_destination_uris() {
        let printer = destination_to_printer_entry(destination(&[
            ("printer-uri-supported", "ipps://printer.local/ipp/print"),
            ("device-uri", "ipp://printer.local/ipp/print"),
        ]));

        assert_eq!(
            printer.printer_uri(),
            Some("ipps://printer.local/ipp/print")
        );
        assert_eq!(printer.device_uri(), Some("ipp://printer.local/ipp/print"));
    }

    #[test]
    fn preserves_unknown_destination_options() {
        let printer =
            destination_to_printer_entry(destination(&[("vendor-example-option", "opaque-value")]));

        assert_eq!(
            printer.option("vendor-example-option"),
            Some("opaque-value")
        );
    }

    #[test]
    fn stores_endpoint_from_remote_printer_uri() {
        let endpoint = endpoint_from_uris(
            Some("ipps://DESKTOP-96VEKVC-2.local:8880/ipp/print"),
            Some("ipps://Abd._ipps._tcp.local/"),
        );

        assert_eq!(
            endpoint,
            Some(("desktop-96vekvc-2.local".to_string(), 8880))
        );
    }

    #[test]
    fn skips_local_scheduler_uri_and_uses_device_uri() {
        let endpoint = endpoint_from_uris(
            Some("ipp://localhost/printers/Abd"),
            Some("ipp://localhost:60001/ipp/print"),
        );

        assert_eq!(endpoint, Some(("localhost".to_string(), 60001)));
    }

    #[test]
    fn leaves_endpoint_absent_when_no_network_uri_is_available() {
        let endpoint = endpoint_from_uris(
            Some("ipp://localhost/printers/Usb"),
            Some("usb://HP/DeskJet"),
        );

        assert_eq!(endpoint, None);
    }

    #[test]
    fn uses_device_uri_when_printer_uri_is_absent() {
        let endpoint = endpoint_from_uris(None, Some("ipp://printer.local:631/ipp/print"));

        assert_eq!(endpoint, Some(("printer.local".to_string(), 631)));
    }

    #[test]
    fn leaves_endpoint_absent_when_both_uris_are_absent() {
        assert_eq!(endpoint_from_uris(None, None), None);
    }

    fn with_web_page(options: &[(&str, &str)]) -> PrinterEntry {
        let mut printer = PrinterEntry::new(
            "Printer",
            "Printer",
            false,
            options
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
        );
        refresh_printer_web_page(&mut printer);
        printer
    }

    #[test]
    fn an_mdns_page_is_offered_at_the_address_it_answered_on() {
        let printer = with_web_page(&[
            (
                "printer-more-info",
                "https://NPI24DE62.local:443/hp/device/info.html?tab=Networking",
            ),
            ("endpoint-hostname", "npi24de62.local"),
            ("endpoint-address", "192.168.0.181"),
        ]);

        assert_eq!(
            printer.web_page(),
            Some("https://192.168.0.181/hp/device/info.html?tab=Networking")
        );
    }

    #[test]
    fn a_page_keeps_a_port_the_scheme_does_not_imply() {
        let printer = with_web_page(&[
            (
                "printer-more-info",
                "http://NPIFA1A52.local:443/airprint_mob_status.htm",
            ),
            ("endpoint-hostname", "npifa1a52.local"),
            ("endpoint-address", "192.168.0.201"),
        ]);

        assert_eq!(
            printer.web_page(),
            Some("http://192.168.0.201:443/airprint_mob_status.htm")
        );
    }

    #[test]
    fn a_page_without_a_port_gains_none() {
        let printer = with_web_page(&[
            ("printer-more-info", "http://NPIDDF19F.local/status.htm"),
            ("endpoint-hostname", "npiddf19f.local"),
            ("endpoint-address", "192.168.0.72"),
            ("endpoint-port", "443"),
        ]);

        assert_eq!(printer.web_page(), Some("http://192.168.0.72/status.htm"));
    }

    #[test]
    fn a_resolvable_name_keeps_its_hostname() {
        let printer = with_web_page(&[
            ("printer-more-info", "https://printer.example.com/status"),
            ("endpoint-hostname", "printer.example.com"),
            ("endpoint-address", "192.0.2.10"),
        ]);

        assert_eq!(
            printer.web_page(),
            Some("https://printer.example.com/status")
        );
    }

    #[test]
    fn a_page_on_another_host_is_left_alone() {
        let printer = with_web_page(&[
            ("printer-more-info", "http://print-server.local/queue"),
            ("endpoint-hostname", "npiddf19f.local"),
            ("endpoint-address", "192.168.0.72"),
        ]);

        assert_eq!(printer.web_page(), Some("http://print-server.local/queue"));
    }

    #[test]
    fn a_page_stands_as_it_is_without_an_address() {
        let printer = with_web_page(&[
            ("printer-more-info", "http://NPIDDF19F.local/status.htm"),
            ("endpoint-hostname", "npiddf19f.local"),
        ]);

        assert_eq!(
            printer.web_page(),
            Some("http://NPIDDF19F.local/status.htm")
        );
    }

    #[test]
    fn an_ipv6_address_is_bracketed() {
        let printer = with_web_page(&[
            ("printer-more-info", "http://printer.local:8000/status"),
            ("endpoint-hostname", "printer.local"),
            ("endpoint-address", "2001:db8::1"),
        ]);

        assert_eq!(printer.web_page(), Some("http://[2001:db8::1]:8000/status"));
    }
}
