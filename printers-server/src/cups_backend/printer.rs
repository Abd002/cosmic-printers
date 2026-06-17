use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::{IppOperation, IppRequest, IppTag, IppValueTag, create_job};

use super::helpers::{
    CupsResultExt, LocalSocketGuard, PRINTER_ATTRIBUTES, add_requesting_user, configured_printers,
    ensure_success, fill_missing_attrs, printer_id_parts,
};
use super::metadata;

const TEST_PAGE_PDF: &str = "/usr/share/cups/data/default-testpage.pdf";

pub async fn list_printers() -> Result<Vec<PrinterEntry>, Error> {
    tokio::task::spawn_blocking(|| {
        let mut printers = configured_printers(250)?;
        metadata::apply(&mut printers)?;

        for printer in printers.values_mut() {
            if fill_missing_attrs(printer, PRINTER_ATTRIBUTES).is_err() {
                eprintln!(
                    "failed to load optional attributes for printer {}",
                    printer.id
                );
            }
        }

        Ok::<Vec<PrinterEntry>, Error>(printers.into_values().collect())
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

pub async fn set_default(printer_uri: &str) -> Result<(), Error> {
    let printer_uri = printer_uri.to_string();

    tokio::task::spawn_blocking(move || {
        // BUG: This sets the server default but does not clear a user default
        // stored in lpoptions, which can continue to override it.
        let mut request = IppRequest::new(IppOperation::CupsSetDefault).cups_err()?;
        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                "printer-uri",
                &printer_uri,
            )
            .cups_err()?;
        add_requesting_user(&mut request)?;

        let _guard = LocalSocketGuard::engage()?;
        request
            .send_default("/admin/")
            .cups_err()
            .and_then(|response| ensure_success(&response, "CUPS-Set-Default"))
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

pub async fn print_test_page(printer: PrinterEntry) -> Result<i32, Error> {
    tokio::task::spawn_blocking(move || {
        let destination = destination_for_print_job(printer);
        let job = create_job(&destination, "Test Page").cups_err()?;

        job.submit_file(TEST_PAGE_PDF, cups_rs::FORMAT_PDF)
            .cups_err()?;

        Ok(job.id)
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

/// Converts the normalized printer entry to the raw CUPS type required by `cupsCreateJob`.
fn destination_for_print_job(printer: PrinterEntry) -> cups_rs::Destination {
    let (name, instance) = {
        let (name, instance) = printer_id_parts(&printer);
        (name.to_string(), instance.map(ToString::to_string))
    };

    cups_rs::Destination {
        name,
        instance,
        is_default: printer.is_default,
        options: printer.options,
    }
}
