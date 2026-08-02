use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::create_job;

use super::helpers::{
    CupsResultExt, PRINTER_ATTRIBUTES, available_destinations, destination_to_printer_entry,
    fill_missing_attrs_from_device_uri, fill_missing_attrs_from_printer_uri, split_queue_instance,
};
use super::polkit_helper;
use crate::error::{BackendError, BackendResult};

const TEST_PAGE_PDF: &str = "/usr/share/cups/data/default-testpage.pdf";

pub async fn list_printers() -> BackendResult<Vec<PrinterEntry>> {
    tokio::task::spawn_blocking(|| {
        let destinations = available_destinations(5000)?;
        let mut printers = destinations
            .into_values()
            .map(|destination| {
                let printer = destination_to_printer_entry(destination.clone());
                (destination, printer)
            })
            .collect::<Vec<_>>();

        fill_printer_attrs(&mut printers);

        Ok(printers.into_iter().map(|(_, printer)| printer).collect())
    })
    .await
    .map_err(BackendError::Join)?
}

fn fill_printer_attrs(printers: &mut [(cups_rs::Destination, PrinterEntry)]) {
    std::thread::scope(|scope| {
        for (destination, printer) in printers {
            scope.spawn(move || {
                let result = if printer.printer_uri().is_some() {
                    fill_missing_attrs_from_printer_uri(printer, PRINTER_ATTRIBUTES)
                } else {
                    fill_missing_attrs_from_device_uri(destination, printer, PRINTER_ATTRIBUTES)
                };
                if let Err(error) = result {
                    tracing::warn!(
                        printer_id = printer.id(),
                        error = ?error,
                        "failed to load optional printer attributes"
                    );
                }
            });
        }
    });
}

pub async fn delete_printer(printer_id: &str) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::delete_printer(queue_name).await
}

pub async fn set_printer_accept_jobs(
    printer_id: &str,
    enabled: bool,
    reason: &str,
) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_accept_jobs(queue_name, enabled, reason).await
}

// BUG: This sets the server default but does not clear a user default
// stored in lpoptions, which can continue to override it.
pub async fn set_printer_default(printer_id: &str) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_default(queue_name).await
}

pub async fn set_printer_option_default(
    printer_id: &str,
    option: &str,
    values: &[String],
) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::add_option_default(queue_name, option, values).await
}

pub async fn set_printer_enabled(printer_id: &str, enabled: bool) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_enabled(queue_name, enabled).await
}

pub async fn set_printer_info(printer_id: &str, info: &str) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_info(queue_name, info).await
}

pub async fn set_printer_location(printer_id: &str, location: &str) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_location(queue_name, location).await
}

pub async fn set_printer_shared(printer_id: &str, shared: bool) -> BackendResult<()> {
    let queue_name = split_queue_instance(printer_id).0;
    polkit_helper::set_printer_shared(queue_name, shared).await
}

pub async fn print_test_page(printer: PrinterEntry) -> BackendResult<i32> {
    tokio::task::spawn_blocking(move || {
        let destination = destination_for_print_job(printer);
        let job = create_job(&destination, "Test Page").cups_err()?;

        job.submit_file(TEST_PAGE_PDF, cups_rs::FORMAT_PDF)
            .cups_err()?;

        Ok(job.id)
    })
    .await
    .map_err(BackendError::Join)?
}

/// Converts the normalized printer entry to the raw CUPS type required by `cupsCreateJob`.
fn destination_for_print_job(printer: PrinterEntry) -> cups_rs::Destination {
    let (name, instance) = {
        let (name, instance) = split_queue_instance(printer.id());
        (name.to_string(), instance.map(ToString::to_string))
    };

    cups_rs::Destination {
        name,
        instance,
        is_default: printer.is_default(),
        options: printer
            .options()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    }
}
