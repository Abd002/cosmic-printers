use cosmic_settings_printers_core::{Error, PrinterEntry};
use cups_rs::create_job;

use super::helpers::{
    CupsResultExt, PRINTER_ATTRIBUTES, configured_printers, fill_missing_attrs,
    split_queue_instance,
};
use super::{metadata, polkit_helper};

const TEST_PAGE_PDF: &str = "/usr/share/cups/data/default-testpage.pdf";

pub async fn list_printers() -> Result<Vec<PrinterEntry>, Error> {
    tokio::task::spawn_blocking(|| {
        let mut printers = configured_printers(250)?;

        fill_printer_attrs(printers.values_mut());
        metadata::apply(&mut printers)?;

        Ok::<Vec<PrinterEntry>, Error>(printers.into_values().collect())
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

fn fill_printer_attrs<'a>(printers: impl Iterator<Item = &'a mut PrinterEntry>) {
    std::thread::scope(|scope| {
        for printer in printers {
            scope.spawn(move || {
                if fill_missing_attrs(printer, PRINTER_ATTRIBUTES).is_err() {
                    eprintln!(
                        "failed to load optional attributes for printer {}",
                        printer.id
                    );
                }
            });
        }
    });
}

// BUG: This sets the server default but does not clear a user default
// stored in lpoptions, which can continue to override it.
pub async fn set_default(printer_id: &str, _printer_uri: &str) -> Result<(), Error> {
    let (queue_name, _instance) = split_queue_instance(printer_id);
    polkit_helper::set_default(queue_name.to_string()).await
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
        let (name, instance) = split_queue_instance(&printer.id);
        (name.to_string(), instance.map(ToString::to_string))
    };

    cups_rs::Destination {
        name,
        instance,
        is_default: printer.is_default,
        options: printer.options,
    }
}
