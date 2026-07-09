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

        metadata::retain_for_configured_queues(printers.keys().map(String::as_str))?;
        metadata::apply(&mut printers)?;
        fill_printer_attrs(printers.values_mut());

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

pub async fn delete_printer(printer_id: &str) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::delete_printer(queue_name).await?;
    metadata::remove(queue_name)
}

pub async fn set_printer_accept_jobs(
    printer_id: &str,
    enabled: bool,
    reason: &str,
) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_accept_jobs(queue_name, enabled, reason).await
}

// BUG: This sets the server default but does not clear a user default
// stored in lpoptions, which can continue to override it.
pub async fn set_printer_default(printer_id: &str) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_default(queue_name).await
}

pub async fn set_printer_option_default(
    printer_id: &str,
    option: &str,
    values: &[String],
) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::add_option_default(queue_name, option, values).await
}

pub async fn set_printer_enabled(printer_id: &str, enabled: bool) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_enabled(queue_name, enabled).await
}

pub async fn set_printer_info(printer_id: &str, info: &str) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_info(queue_name, info).await
}

pub async fn set_printer_location(printer_id: &str, location: &str) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_location(queue_name, location).await
}

pub async fn set_printer_shared(printer_id: &str, shared: bool) -> Result<(), Error> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_shared(queue_name, shared).await
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
