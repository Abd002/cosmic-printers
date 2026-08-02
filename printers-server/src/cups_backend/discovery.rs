use cosmic_settings_printers_core::PrinterEntry;

use super::helpers::{PRINTER_ATTRIBUTES, fill_attrs_from_device};
use crate::avahi::discovered_printers_match;
use crate::context::Context;

pub(crate) async fn start_discovery(context: Context) {
    let Some(discovery_lease) = context.try_start_discovery() else {
        return;
    };

    tokio::spawn(async move {
        let _discovery_lease = discovery_lease;
        match crate::avahi::discover_printers_into_cache(context.clone()).await {
            Ok(summary) => {
                tracing::debug!(
                    services_seen = summary.services_seen,
                    printers_resolved = summary.printers_resolved,
                    applications_resolved = summary.applications_resolved,
                    warnings = summary.warnings,
                    "printer discovery refresh completed"
                );
            }
            Err(error) => {
                tracing::warn!(error = ?error, "printer discovery refresh failed");
            }
        }
        fill_cached_discovered_attrs(context).await;
    });
}

async fn fill_cached_discovered_attrs(context: Context) {
    let printers = context.discovered_printers_cached().await;

    let printers = match tokio::task::spawn_blocking(move || {
        printers
            .into_iter()
            .map(|mut printer| {
                if printer.device_uri().is_some() {
                    match fill_attrs_from_device(&mut printer, PRINTER_ATTRIBUTES) {
                        Ok(()) => printer.set_option(
                            "cosmic-discovery-detail-state".to_string(),
                            "enriched".to_string(),
                        ),
                        Err(error) => tracing::warn!(
                            printer_id = printer.id(),
                            error = ?error,
                            "failed to enrich discovered printer"
                        ),
                    }
                }
                printer
            })
            .collect::<Vec<PrinterEntry>>()
    })
    .await
    {
        Ok(printers) => printers,
        Err(error) => {
            tracing::warn!(error = ?error, "discovered printer enrichment task failed");
            return;
        }
    };

    context
        .merge_discovered_printers_by(printers, discovered_printers_match)
        .await;
}
