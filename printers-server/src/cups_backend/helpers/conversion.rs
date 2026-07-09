use cosmic_settings_printers_core::{PrinterEntry, PrinterStatus};
use cups_rs::{Destination, PrinterState as CupsPrinterState};

use super::identity::local_printer_uri;
use super::options::{is_loopback_host, is_printer_class, option_values, parse_uri_endpoint};

/// Derives a simple web interface URL from a device URI hostname.
fn web_page_from_device_uri(device_uri: &str) -> Option<String> {
    let (hostname, _) = parse_uri_endpoint(device_uri)?;
    Some(format!("http://{hostname}"))
}

/// Converts a cups-rs destination into the type exposed by the printer API.
pub(super) fn destination_to_printer_entry(mut destination: Destination) -> PrinterEntry {
    let queue_status = destination.state().to_string();
    let printer_local_uri = destination.uri().cloned().unwrap_or_else(|| {
        local_printer_uri(&destination.name, is_printer_class(&destination.options))
    });
    let device_uri = destination.device_uri().cloned().unwrap_or_default();
    destination
        .options
        .entry("printer-uri-supported".to_string())
        .or_insert_with(|| printer_local_uri.clone());
    destination
        .options
        .entry("device-uri".to_string())
        .or_insert_with(|| device_uri.clone());
    let endpoint = endpoint_from_uris(&printer_local_uri, &device_uri);
    let id = destination.full_name();
    let name = destination
        .info()
        .filter(|info| !info.is_empty())
        .cloned()
        .unwrap_or_else(|| id.clone());
    let paper_sizes = option_values(&destination.options, "media-supported");
    let print_sides = option_values(&destination.options, "sides-supported");
    let web_page = destination
        .options
        .get("printer-more-info")
        .filter(|url| !url.trim().is_empty())
        .cloned()
        .or_else(|| {
            destination
                .options
                .get("device-uri")
                .and_then(|device_uri| web_page_from_device_uri(device_uri))
        });

    PrinterEntry {
        id,
        name,
        is_default: destination.is_default,
        printer_local_uri,
        status: printer_status(&destination),
        queue_status,
        location: destination.location().cloned().unwrap_or_default(),
        model: destination.make_and_model().cloned().unwrap_or_default(),
        device_uri,
        hostname: endpoint.as_ref().map(|(host, _)| host.clone()),
        port: endpoint.map(|(_, port)| port),
        web_page,
        driver_version: String::new(),
        paper_size_idx: 0,
        print_sides_idx: 0,
        options: destination.options,
        supplies: Vec::new(),
        paper_sizes,
        print_sides,
    }
}

fn endpoint_from_uris(printer_uri: &str, device_uri: &str) -> Option<(String, u16)> {
    if is_local_scheduler_uri(printer_uri) {
        return parse_uri_endpoint(device_uri);
    }

    parse_uri_endpoint(printer_uri).or_else(|| parse_uri_endpoint(device_uri))
}

fn is_local_scheduler_uri(uri: &str) -> bool {
    let Some((host, port)) = parse_uri_endpoint(uri) else {
        return false;
    };

    port == 631
        && is_loopback_host(&host)
        && (uri.contains("/printers/") || uri.contains("/classes/"))
}

/// Recomputes derived public fields after new IPP attributes are merged.
pub(super) fn refresh_printer_entry(printer: &mut PrinterEntry) {
    printer.device_uri = printer
        .options
        .get("device-uri")
        .cloned()
        .unwrap_or_default();
    printer.location = printer
        .options
        .get("printer-location")
        .cloned()
        .unwrap_or_default();
    printer.model = printer
        .options
        .get("printer-make-and-model")
        .cloned()
        .unwrap_or_default();
    printer.web_page = printer
        .options
        .get("printer-more-info")
        .filter(|url| !url.trim().is_empty())
        .cloned()
        .or_else(|| web_page_from_device_uri(&printer.device_uri));
    apply_endpoint(
        printer,
        endpoint_from_uris(&printer.printer_local_uri, &printer.device_uri),
    );
    printer.paper_sizes = option_values(&printer.options, "media-supported");
    printer.print_sides = option_values(&printer.options, "sides-supported");
    printer.status = printer_status_from_options(printer);
}

pub(super) fn apply_endpoint(printer: &mut PrinterEntry, endpoint: Option<(String, u16)>) {
    printer.hostname = endpoint.as_ref().map(|(host, _)| host.clone());
    printer.port = endpoint.map(|(_, port)| port);
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

/// Maps normalized printer options to the UI printer status after IPP refreshes.
fn printer_status_from_options(printer: &PrinterEntry) -> PrinterStatus {
    if option_values(&printer.options, "printer-state-reasons")
        .iter()
        .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
    {
        return PrinterStatus::LowToner;
    }

    match printer.options.get("printer-state").map(String::as_str) {
        Some("5") => PrinterStatus::Offline,
        Some("3" | "4") => PrinterStatus::Ready,
        _ => printer.status.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_endpoint_from_remote_printer_uri() {
        let endpoint = endpoint_from_uris(
            "ipps://DESKTOP-96VEKVC-2.local:8880/ipp/print",
            "ipps://Abd._ipps._tcp.local/",
        );

        assert_eq!(
            endpoint,
            Some(("desktop-96vekvc-2.local".to_string(), 8880))
        );
    }

    #[test]
    fn skips_local_scheduler_uri_and_uses_device_uri() {
        let endpoint = endpoint_from_uris(
            "ipp://localhost/printers/Abd",
            "ipp://localhost:60001/ipp/print",
        );

        assert_eq!(endpoint, Some(("localhost".to_string(), 60001)));
    }

    #[test]
    fn leaves_endpoint_absent_when_no_network_uri_is_available() {
        let endpoint = endpoint_from_uris("ipp://localhost/printers/Usb", "usb://HP/DeskJet");

        assert_eq!(endpoint, None);
    }
}
