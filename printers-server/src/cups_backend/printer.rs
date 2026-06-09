use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::{
    Destination, IppOperation, IppRequest, IppTag, IppValueTag, create_job, enum_destinations,
};
use std::collections::HashMap;

use super::helpers::{
    LOCAL_CUPS_SOCKET, add_requesting_user, destination_to_printer_entry, ensure_success,
    fill_missing_attrs,
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
    let destinations = tokio::task::spawn_blocking(|| {
        let mut destinations = HashMap::<String, Destination>::new();

        enum_destinations(
            cups_rs::DEST_FLAGS_NONE,
            250,
            None,
            0,
            0,
            &mut |flags, destination, destinations: &mut HashMap<String, Destination>| {
                let id = destination.full_name();

                if flags & cups_rs::DEST_FLAGS_REMOVED != 0 {
                    destinations.remove(&id);
                } else {
                    destinations.insert(id, destination.clone());
                }

                true
            },
            &mut destinations,
        )
        .map_err(|_| Error::CupsFailed)?;

        for destination in destinations.values_mut() {
            if fill_missing_attrs(destination, PRINTER_ATTRIBUTES).is_err() {
                eprintln!(
                    "failed to load optional attributes for printer {}",
                    destination.full_name()
                );
            }
        }

        Ok::<HashMap<String, Destination>, Error>(destinations)
    })
    .await
    .map_err(|_| Error::CupsFailed)??;

    Ok(destinations
        .into_values()
        .map(destination_to_printer_entry)
        .collect())
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
