use cosmic_settings_printers_core::{Error, PrinterEntry, SupplyLevel};

use super::Server;
use crate::cups;
use crate::state::State;

impl Server {
    /// Returns cached supplies immediately and refreshes them in the background.
    /// Changes update the destination through the existing event feed.
    pub async fn printer_supplies(&self, printer_id: &str) -> Result<Vec<SupplyLevel>, Error> {
        let printer = self.printer_entry(printer_id).await?;
        let supplies = printer.supplies();

        reload_behind_the_answer(self.context.clone(), printer);

        Ok(supplies)
    }
}

/// Re-reads one printer without the caller waiting for it.
fn reload_behind_the_answer(context: State, printer: PrinterEntry) {
    tokio::spawn(async move {
        let printer_id = printer.id().to_string();

        match cups::reload_printer(context.clone(), printer).await {
            Ok(updated) => context.update_available_destination(updated),
            Err(error) => tracing::debug!(
                printer_id,
                error = ?error,
                "could not re-read a printer behind an answer about it"
            ),
        }
    });
}
