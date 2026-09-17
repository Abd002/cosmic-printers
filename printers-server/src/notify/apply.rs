//! What each event means for what the app already knows.

use super::events::Event;
use crate::cups;
use crate::state::State;

pub(super) async fn apply(context: &State, event: Event) {
    match event {
        Event::JobsChanged(printer_id) => context.emit_jobs_changed(&printer_id),
        Event::PrinterRemoved(printer_id) => context.remove_available_destination(&printer_id),
        Event::PrinterChanged(printer_id) => reload(context, &printer_id).await,
    }
}

async fn reload(context: &State, printer_id: &str) {
    let Some(printer) = context.available_destination_cached(printer_id).await else {
        // Nothing to re-read yet, so let an enumeration find it.
        cups::refresh_available_destinations(context.clone());
        return;
    };

    match cups::reload_printer(context.clone(), printer).await {
        Ok(printer) => context.update_available_destination(printer),
        Err(error) => {
            tracing::warn!(
                printer_id,
                error = ?error,
                "failed to re-read a printer after an event"
            );
        }
    }
}
