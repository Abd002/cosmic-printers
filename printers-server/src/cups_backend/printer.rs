use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::{Destination, IppOperation, IppRequest, IppTag, IppValueTag, create_job};

use super::helpers::{
    LOCAL_CUPS_SOCKET, add_requesting_user, configured_destinations, destination_to_printer_entry,
    discovered_destinations, ensure_success, fill_missing_attrs, printer_status_with_discovery,
};

const TEST_PAGE_PDF: &str = "/usr/share/cups/data/default-testpage.pdf";

const PRINTER_ATTRIBUTES: &[&str] = &[
    "printer-more-info",
    "printer-state",
    "printer-state-message",
    "printer-state-reasons",
    "printer-is-accepting-jobs",
    "printer-type",
    "printer-location",
    "printer-info",
    "printer-make-and-model",
    "device-uri",
    "marker-colors",
    "marker-levels",
    "marker-names",
    "marker-types",
    "media-default",
    "media-supported",
    "sides-default",
    "sides-supported",
    "printer-uuid",
];

pub async fn list_printers() -> Result<Vec<PrinterEntry>, Error> {
    tokio::task::spawn_blocking(|| {
        let mut destinations = configured_destinations(250)?;
        let discovered = discovered_destinations(250)?;

        for destination in destinations.values_mut() {
            if fill_missing_attrs(destination, PRINTER_ATTRIBUTES).is_err() {
                eprintln!(
                    "failed to load optional attributes for printer {}",
                    destination.full_name()
                );
            }
        }

        let printers = destinations
            .into_values()
            .map(|destination| {
                let status = printer_status_with_discovery(&destination, discovered.values());
                destination_to_printer_entry(destination, status)
            })
            .collect();

        Ok::<Vec<PrinterEntry>, Error>(printers)
    })
    .await
    .map_err(|_| Error::CupsFailed)?
}

pub async fn set_default(printer_uri: &str) -> Result<(), Error> {
    let printer_uri = printer_uri.to_string();

    tokio::task::spawn_blocking(move || {
        // BUG: This sets the server default but does not clear a user default
        // stored in lpoptions, which can continue to override it.
        let mut request =
            IppRequest::new(IppOperation::CupsSetDefault).map_err(|_| Error::CupsFailed)?;
        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                "printer-uri",
                &printer_uri,
            )
            .map_err(|_| Error::CupsFailed)?;
        add_requesting_user(&mut request)?;

        let previous_server = cups_rs::config::get_server();

        // Use the local socket so CUPS can authorize lpadmin users with PeerCred.
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).map_err(|_| Error::CupsFailed)?;

        let result = request
            .send_default("/admin/")
            .map_err(|_| Error::CupsFailed)
            .and_then(|response| ensure_success(response, "CUPS-Set-Default"));

        cups_rs::config::set_server(Some(&previous_server)).map_err(|_| Error::CupsFailed)?;
        result
    })
    .await
    .map_err(|_| Error::CupsFailed)?
}

pub async fn print_test_page(destination: Destination) -> Result<i32, Error> {
    tokio::task::spawn_blocking(move || {
        let job = create_job(&destination, "Test Page").map_err(|_| Error::CupsFailed)?;

        job.submit_file(TEST_PAGE_PDF, cups_rs::FORMAT_PDF)
            .map_err(|_| Error::CupsFailed)?;

        Ok(job.id)
    })
    .await
    .map_err(|_| Error::CupsFailed)?
}
