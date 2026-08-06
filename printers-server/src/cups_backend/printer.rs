use cosmic_settings_printers_core::{PrinterEntry, SupplyLevel, is_local_address};
use cups_rs::create_job;
use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::sync::{LazyLock, Mutex};

use super::helpers::{
    CupsResultExt, PRINTER_ATTRIBUTES, available_destinations, destination_to_printer_entry,
    fill_missing_attrs_from_device_uri, fill_missing_attrs_from_printer_uri, split_queue_instance,
    supplies_from_device,
};
use super::polkit_helper;
use crate::context::Context;
use crate::error::{BackendError, BackendResult};
use crate::ipp::is_local_scheduler_uri;

const TEST_PAGE_PDF: &str = "/usr/share/cups/data/default-testpage.pdf";
/// The attribute CUPS reads to decide where to submit a job.
const PRINTER_URI_SUPPORTED: &str = "printer-uri-supported";

pub fn refresh_available_destinations(context: Context) {
    if let Some(lease) = context.try_start_available_destinations_refresh() {
        let worker_context = context.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            if let Err(error) = run_available_destinations_refresh(worker_context) {
                tracing::warn!(
                    error = ?error,
                    "failed to refresh available printer destinations"
                );
            }
        });
    }
}

fn run_available_destinations_refresh(worker_context: Context) -> BackendResult<()> {
    let callback_context = worker_context.clone();
    let destinations = available_destinations(5000, move |flags, destination| {
        let id = destination.full_name();
        if flags & cups_rs::DEST_FLAGS_REMOVED != 0 {
            callback_context.remove_available_destination(&id);
        } else {
            callback_context
                .merge_available_destination(destination_to_printer_entry(destination.clone()));
        }
    })?;
    let mut printers = destinations
        .into_values()
        .map(|destination| {
            let printer = destination_to_printer_entry(destination.clone());
            (destination, printer)
        })
        .collect::<Vec<_>>();

    fill_printer_attrs(&mut printers, &worker_context);
    Ok(())
}

fn fill_printer_attrs(printers: &mut [(cups_rs::Destination, PrinterEntry)], context: &Context) {
    std::thread::scope(|scope| {
        for (destination, printer) in printers {
            let context = context.clone();
            scope.spawn(move || {
                let result = if printer.printer_uri().is_some_and(is_local_scheduler_uri) {
                    fill_missing_attrs_from_printer_uri(printer, PRINTER_ATTRIBUTES)
                } else if printer.device_uri().is_some() {
                    fill_missing_attrs_from_device_uri(destination, printer, PRINTER_ATTRIBUTES)
                } else {
                    fill_missing_attrs_from_printer_uri(printer, PRINTER_ATTRIBUTES)
                };
                finish_printer_enrichment(&context, printer, result);
            });
        }
    });
}

fn finish_printer_enrichment(
    context: &Context,
    printer: &mut PrinterEntry,
    result: BackendResult<()>,
) {
    match result {
        Ok(()) => {
            resolve_printer_endpoint_locality(printer);
            context.update_available_destination(printer.clone());
        }
        Err(error) => {
            tracing::warn!(
                printer_id = printer.id(),
                error = ?error,
                "failed to load optional printer attributes"
            );
        }
    }
}

fn resolve_printer_endpoint_locality(printer: &mut PrinterEntry) {
    if printer.option("endpoint-is-local").is_some() {
        return;
    }
    let Some(hostname) = printer.hostname() else {
        return;
    };

    static LOCALITY: LazyLock<Mutex<HashMap<String, bool>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = hostname.to_ascii_lowercase();
    let cached = LOCALITY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .copied();
    let is_local = cached.unwrap_or_else(|| {
        let is_local = (hostname, 0)
            .to_socket_addrs()
            .is_ok_and(|mut addresses| addresses.any(|address| is_local_address(address.ip())));
        LOCALITY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, is_local);
        is_local
    });
    printer.set_option("endpoint-is-local", is_local.to_string());
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

/// Returns the supplies a printer reports, asking the printer itself.
///
/// A print queue only carries supply levels once it has printed something, so asking
/// the queue would report nothing for a printer that has been set up and not yet used.
/// What the queue last said is the fallback, and the only source at all for a printer
/// with no endpoint to ask — one attached over USB, or a virtual destination.
///
/// A printer that cannot be reached reports no supplies rather than failing: a page is
/// not broken because a printer is asleep, and no supplies simply shows no supplies.
pub async fn printer_supplies(printer: PrinterEntry) -> BackendResult<Vec<SupplyLevel>> {
    tokio::task::spawn_blocking(move || {
        let reported = match supplies_from_device(&raw_destination(&printer), &printer) {
            Ok(supplies) => supplies,
            Err(error) => {
                tracing::debug!(
                    printer_id = printer.id(),
                    error = ?error,
                    "could not ask a printer for its supplies"
                );
                Vec::new()
            }
        };

        if reported.is_empty() {
            return Ok(printer.supplies());
        }

        Ok(reported)
    })
    .await
    .map_err(BackendError::Join)?
}

pub async fn print_test_page(printer: PrinterEntry) -> BackendResult<i32> {
    tokio::task::spawn_blocking(move || {
        let destination = destination_for_print_job(printer);
        let job = create_job(&destination, "Test Page").cups_err()?;

        // The job exists from here on, so a document that cannot be sent leaves an
        // empty one behind unless it is withdrawn. Report the original failure.
        if let Err(error) = job.submit_file(TEST_PAGE_PDF, cups_rs::FORMAT_PDF) {
            if let Err(why) = job.cancel() {
                tracing::warn!(
                    job_id = job.id,
                    error = ?why,
                    "failed to cancel a test page that could not be sent"
                );
            }
            return Err(BackendError::Cups(error));
        }

        Ok(job.id)
    })
    .await
    .map_err(BackendError::Join)?
}

/// Converts the normalized printer entry to the raw CUPS type required by
/// `cupsCreateDestJob`.
///
/// A `printer-uri-supported` that is not the local scheduler's is left out. CUPS
/// submits on the default connection whatever the destination is, so it reads that
/// attribute as a path on the scheduler — and for a destination that answers
/// elsewhere the scheduler has no such path, which fails the job before it exists.
/// Left out, CUPS resolves the device URI instead and makes a queue for it on
/// demand, which is what puts the job somewhere the queue view can find it.
fn destination_for_print_job(printer: PrinterEntry) -> cups_rs::Destination {
    let scheduler_holds_the_queue = printer.printer_uri().is_some_and(is_local_scheduler_uri);
    let mut destination = raw_destination(&printer);

    if !scheduler_holds_the_queue {
        destination.options.remove(PRINTER_URI_SUPPORTED);
    }

    destination
}

/// Converts the normalized printer entry to the raw CUPS type, as it stands.
fn raw_destination(printer: &PrinterEntry) -> cups_rs::Destination {
    let (name, instance) = split_queue_instance(printer.id());

    cups_rs::Destination {
        name: name.to_string(),
        instance: instance.map(ToString::to_string),
        is_default: printer.is_default(),
        options: printer
            .options()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::EndpointSource;
    use std::collections::HashMap;

    fn printer(options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            "printer",
            "Printer",
            false,
            options
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[tokio::test]
    async fn failed_enrichment_preserves_previous_connected_endpoint() {
        let context = Context::new();
        let mut connected = printer(&[
            ("endpoint-hostname", "printer.local"),
            ("endpoint-port", "8000"),
            ("endpoint-source", "connected"),
        ]);
        connected.set_endpoint_source(EndpointSource::Connected);
        context.update_available_destination(connected);

        let mut fresh = printer(&[
            ("endpoint-hostname", "printer._ipps._tcp.local"),
            ("endpoint-port", "631"),
            ("endpoint-source", "uri"),
        ]);
        finish_printer_enrichment(
            &context,
            &mut fresh,
            Err(BackendError::Internal("offline".into())),
        );

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("printer.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), None);
        assert_eq!(cached[0].endpoint_source(), Some(EndpointSource::Connected));
    }
}
