//! Re-reading what CUPS offers, all of it or one printer.

use cosmic_settings_printers_core::{PrinterEntry, is_local_address};
use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::sync::{LazyLock, Mutex};

use super::conversion::destination_to_printer_entry;
use super::destinations::{available_destinations, raw_destination};
use super::routing::read_printer_attrs;
use crate::error::{BackendError, BackendResult};
use crate::printer_app::{self, OwnedPrinter};
use crate::state::State;

pub fn refresh_available_destinations(context: State) {
    if let Some(lease) = context.try_start_available_destinations_refresh() {
        let worker_context = context.clone();
        tokio::spawn(async move {
            let applications = worker_context.printer_applications_cached().await;

            tokio::task::spawn_blocking(move || {
                let _lease = lease;
                if let Err(error) = run_available_destinations_refresh(worker_context, applications)
                {
                    tracing::warn!(
                        error = ?error,
                        "failed to refresh available printer destinations"
                    );
                }
            });
        });
    }
}

fn run_available_destinations_refresh(
    worker_context: State,
    applications: Vec<cosmic_settings_printers_core::PrinterApplication>,
) -> BackendResult<()> {
    // Asked once for the whole pass: every Printer Application on this machine lists its printers,
    // so each destination can be routed to whoever owns it rather than probed to find out.
    let owned = printer_app::owned_printers(&worker_context, &applications);

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
    // Prune only after a complete enumeration.
    worker_context.retain_available_destinations(&destinations.keys().cloned().collect());

    let mut printers = destinations
        .into_values()
        .map(|destination| {
            let printer = destination_to_printer_entry(destination.clone());
            (destination, printer)
        })
        .collect::<Vec<_>>();

    fill_printer_attrs(&mut printers, &worker_context, &owned);
    Ok(())
}

fn fill_printer_attrs(
    printers: &mut [(cups_rs::Destination, PrinterEntry)],
    context: &State,
    owned: &[OwnedPrinter],
) {
    const MAX_CONCURRENT_ENRICHMENTS: usize = 4;

    for printers in printers.chunks_mut(MAX_CONCURRENT_ENRICHMENTS) {
        std::thread::scope(|scope| {
            for (destination, printer) in printers {
                let context = context.clone();
                scope.spawn(move || {
                    let result = read_printer_attrs(destination, printer, owned);
                    finish_printer_enrichment(&context, printer, result);
                });
            }
        });
    }
}

fn finish_printer_enrichment(
    context: &State,
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

/// Re-reads one printer after a change.
/// A full refresh enumerates every destination and drops overlapping requests, making it unsuitable here.
pub async fn reload_printer(context: State, printer: PrinterEntry) -> BackendResult<PrinterEntry> {
    let applications = context.printer_applications_cached().await;

    tokio::task::spawn_blocking(move || {
        let mut printer = printer;
        let destination = raw_destination(&printer);
        // Routed the same way a refresh routes it, so re-reading one printer cannot reach it
        // differently from the pass that will read it next.
        let owned = printer_app::owned_printers(&context, &applications);

        read_printer_attrs(&destination, &mut printer, &owned)?;
        resolve_printer_endpoint_locality(&mut printer);

        Ok(printer)
    })
    .await
    .map_err(BackendError::Join)?
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
        let context = State::new();
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
