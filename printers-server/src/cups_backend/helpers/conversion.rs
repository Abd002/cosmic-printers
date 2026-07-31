use cosmic_settings_printers_core::{PrinterEntry, PrinterStatus};
use cups_rs::{Destination, PrinterState as CupsPrinterState};

use super::identity::local_printer_uri;
use super::options::is_printer_class;
use crate::ipp::{is_local_scheduler_uri, parse_uri_endpoint, web_page_from_uri};

/// Derives a simple web interface URL from a device URI hostname.
fn web_page_from_device_uri(device_uri: &str) -> Option<String> {
    web_page_from_uri(device_uri)
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
    if !destination.options.contains_key("printer-more-info")
        && let Some(web_page) = web_page_from_device_uri(&device_uri)
    {
        destination
            .options
            .insert("printer-more-info".to_string(), web_page);
    }
    let mut printer = PrinterEntry::new(id, name, destination.is_default, destination.options);
    apply_endpoint(
        &mut printer,
        endpoint_from_uris(&printer_local_uri, &device_uri),
    );
    printer
}

fn endpoint_from_uris(printer_uri: &str, device_uri: &str) -> Option<(String, u16)> {
    if is_local_scheduler_uri(printer_uri) {
        return parse_uri_endpoint(device_uri);
    }

    parse_uri_endpoint(printer_uri).or_else(|| parse_uri_endpoint(device_uri))
}

/// Recomputes derived public fields after new IPP attributes are merged.
pub(super) fn refresh_printer_entry(printer: &mut PrinterEntry) {
    let device_uri = printer.device_uri().unwrap_or_default().to_string();
    if printer.web_page().is_none()
        && let Some(web_page) = web_page_from_device_uri(&device_uri)
    {
        printer.set_option("printer-more-info", web_page);
    }
    apply_endpoint(
        printer,
        endpoint_from_uris(printer.printer_local_uri().unwrap_or_default(), &device_uri),
    );
}

pub(super) fn apply_endpoint(printer: &mut PrinterEntry, endpoint: Option<(String, u16)>) {
    if let Some((host, port)) = endpoint {
        printer.set_option("endpoint-hostname", host);
        printer.set_option("endpoint-port", port.to_string());
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

    #[test]
    fn derives_secure_web_page_from_ipps_device_uri() {
        assert_eq!(
            web_page_from_device_uri("ipps://printer.local:8000/ipp/print").as_deref(),
            Some("https://printer.local:8000/")
        );
    }
}
