use cosmic_settings_printers_core::{Error, PrinterEntry};

use super::{Server, service_error};
use crate::cups;

impl Server {
    /// Lists the currently cached libcups destinations without performing I/O.
    pub async fn list_printers(&self) -> Result<Vec<PrinterEntry>, Error> {
        let mut printers = self.context.available_destinations_cached().await;

        // User defaults are the effective libcups values.
        tokio::task::spawn_blocking(move || {
            cups::apply_saved(&mut printers);
            cups::mark_administrable(&mut printers);
            printers
        })
        .await
        .map_err(|error| Error::Internal {
            why: error.to_string(),
        })
    }

    /// Returns one currently cached destination without refreshing all printers.
    pub async fn get_printer(&self, printer_id: &str) -> Result<PrinterEntry, Error> {
        let printer = self
            .context
            .available_destination_cached(printer_id)
            .await
            .ok_or(Error::PrinterNotFound)?;

        tokio::task::spawn_blocking(move || {
            let mut printers = [printer];
            cups::apply_saved(&mut printers);
            cups::mark_administrable(&mut printers);
            printers.into_iter().next().expect("single printer array")
        })
        .await
        .map_err(|error| Error::Internal {
            why: error.to_string(),
        })
    }

    /// Starts a background libcups refresh of the available destination cache.
    pub async fn refresh_available_destinations(&self) -> Result<(), Error> {
        cups::refresh_available_destinations(self.context.clone());
        Ok(())
    }

    /// Re-reads one changed printer so subsequent reads do not return stale state.
    /// A full refresh is slow and drops overlapping requests.
    async fn reload_into_cache(&self, printer_id: &str) {
        let Ok(printer) = self.printer_entry(printer_id).await else {
            return;
        };

        match cups::reload_printer(self.context.clone(), printer).await {
            Ok(updated) => self.context.update_available_destination(updated),
            Err(error) => tracing::debug!(
                printer_id,
                error = ?error,
                "could not re-read a printer after changing it"
            ),
        }
    }

    /// Deletes a configured printer.
    pub async fn delete_printer(&self, printer_id: &str) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        let outcome = cups::delete_printer(printer).await.map_err(service_error);

        // Drop the cache immediately because a deleted printer cannot be refreshed.
        if outcome.is_ok() {
            self.context.remove_available_destination(printer_id);
        }

        outcome
    }

    /// Enables or disables accepting jobs for a printer.
    pub async fn set_printer_accept_jobs(
        &self,
        printer_id: &str,
        enabled: bool,
        reason: &str,
    ) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        let outcome = cups::set_accept_jobs(printer, enabled, reason.to_string())
            .await
            .map_err(service_error);

        if outcome.is_ok() {
            self.reload_into_cache(printer_id).await;
        }

        outcome
    }

    /// Sets the user's libcups default, including destinations without permanent queues.
    pub async fn set_printer_default(&self, printer_id: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        let previous_default = self
            .list_printers()
            .await?
            .into_iter()
            .find(PrinterEntry::is_default)
            .map(|printer| printer.id().to_string());
        let outcome = cups::set_default(printer_id).await.map_err(service_error);

        if outcome.is_ok() {
            if let Some(previous_default) = previous_default {
                if previous_default != printer_id {
                    self.context
                        .emit_available_destinations_changed(&previous_default);
                }
            }
            self.context
                .emit_available_destinations_changed(printer_id);
        }

        outcome
    }

    /// Leaves this user with no default printer.
    pub async fn clear_printer_default(&self) -> Result<(), Error> {
        let previous_default = self
            .list_printers()
            .await?
            .into_iter()
            .find(PrinterEntry::is_default)
            .map(|printer| printer.id().to_string());
        let outcome = cups::clear_default().await.map_err(service_error);

        if outcome.is_ok() {
            if let Some(previous_default) = previous_default {
                self.context
                    .emit_available_destinations_changed(&previous_default);
            }
        }

        outcome
    }

    /// Returns whether the user's groups allow printer administration.
    pub fn may_administer_printers(&self) -> bool {
        cups::may_administer_printers()
    }

    /// Sets a default printer option value.
    pub async fn set_printer_option_default(
        &self,
        printer_id: &str,
        option: &str,
        values: &[String],
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        let outcome = cups::set_option_default(printer_id, option, values)
            .await
            .map_err(service_error);

        if outcome.is_ok() {
            self.context
                .emit_available_destinations_changed(printer_id);
        }

        outcome
    }

    /// Enables or disables a printer.
    pub async fn set_printer_enabled(&self, printer_id: &str, enabled: bool) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        let outcome = cups::set_enabled(printer, enabled)
            .await
            .map_err(service_error);

        if outcome.is_ok() {
            self.reload_into_cache(printer_id).await;
        }

        outcome
    }

    /// Sets the printer information string.
    pub async fn set_printer_info(&self, printer_id: &str, info: &str) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        let outcome = cups::set_info(printer, info.to_string())
            .await
            .map_err(service_error);

        if outcome.is_ok() {
            self.reload_into_cache(printer_id).await;
        }

        outcome
    }

    /// Sets the printer location.
    pub async fn set_printer_location(
        &self,
        printer_id: &str,
        location: &str,
    ) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        let outcome = cups::set_location(printer, location.to_string())
            .await
            .map_err(service_error);

        if outcome.is_ok() {
            self.reload_into_cache(printer_id).await;
        }

        outcome
    }
}
