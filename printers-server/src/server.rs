use cosmic_settings_printers_core::{
    Error, JobInfo, PrinterApplication, PrinterEntry, PrintersEvent,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::broadcast;

use crate::{context::Context, cups_backend, error::BackendError};

#[derive(Debug)]
/// The server-side implementation of the COSMIC printers interface.
pub struct Server {
    context: Context,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    /// Creates a printer service with an empty in-memory context.
    pub fn new() -> Self {
        Self {
            context: Context::new(),
        }
    }

    /// Lists the configured CUPS printers.
    pub async fn list_printers(&self) -> Result<Vec<PrinterEntry>, Error> {
        let mut printers = cups_backend::list_printers().await.map_err(service_error)?;
        let discovered = self.context.discovered_printers_cached().await;
        cups_backend::attach_discovered_metadata(&mut printers, &discovered);
        Ok(printers)
    }

    /// Starts a background DNS-SD discovery refresh when one is not already running.
    pub async fn start_discovery(&self) -> Result<(), Error> {
        cups_backend::start_discovery(self.context.clone()).await;
        Ok(())
    }

    /// Lists the currently cached discovered printers.
    pub async fn list_discovered_printers(&self) -> Result<Vec<PrinterEntry>, Error> {
        Ok(self.context.discovered_printers_cached().await)
    }

    /// Lists the currently cached Printer Applications.
    pub async fn list_printer_applications(&self) -> Result<Vec<PrinterApplication>, Error> {
        Ok(self.context.printer_applications_cached().await)
    }

    /// Streams printer and discovery changes.
    pub fn watch_printers(
        &self,
    ) -> impl Stream<Item = zlink::Reply<PrintersEvent>> + Unpin + use<> {
        let receiver = self.context.subscribe_events();

        futures_util::stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        return Some((
                            zlink::Reply::new(Some(event)).set_continues(Some(true)),
                            receiver,
                        ));
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .boxed()
    }

    /// Deletes a configured printer.
    pub async fn delete_printer(&self, printer_id: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::delete_printer(printer_id)
            .await
            .map_err(service_error)
    }

    /// Enables or disables accepting jobs for a printer.
    pub async fn set_printer_accept_jobs(
        &self,
        printer_id: &str,
        enabled: bool,
        reason: &str,
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_accept_jobs(printer_id, enabled, reason)
            .await
            .map_err(service_error)
    }

    /// Sets the system default printer.
    pub async fn set_printer_default(&self, printer_id: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_default(printer_id)
            .await
            .map_err(service_error)
    }

    /// Sets a default printer option value.
    pub async fn set_printer_option_default(
        &self,
        printer_id: &str,
        option: &str,
        values: &[String],
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_option_default(printer_id, option, values)
            .await
            .map_err(service_error)
    }

    /// Enables or disables a printer.
    pub async fn set_printer_enabled(&self, printer_id: &str, enabled: bool) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_enabled(printer_id, enabled)
            .await
            .map_err(service_error)
    }

    /// Sets the printer information string.
    pub async fn set_printer_info(&self, printer_id: &str, info: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_info(printer_id, info)
            .await
            .map_err(service_error)
    }

    /// Sets the printer location.
    pub async fn set_printer_location(
        &self,
        printer_id: &str,
        location: &str,
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_location(printer_id, location)
            .await
            .map_err(service_error)
    }

    /// Enables or disables printer sharing.
    pub async fn set_printer_shared(&self, printer_id: &str, shared: bool) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_shared(printer_id, shared)
            .await
            .map_err(service_error)
    }

    /// Prints a test page and returns its job ID.
    pub async fn print_test_page(&self, printer_id: &str) -> Result<i32, Error> {
        let printer = self.printer_entry(printer_id).await?;

        cups_backend::print_test_page(printer)
            .await
            .map_err(service_error)
    }

    /// Lists jobs for a configured printer.
    pub async fn get_jobs(&self, printer_id: &str, filter: &str) -> Result<Vec<JobInfo>, Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::get_jobs(&printer, filter)
            .await
            .map_err(service_error)
    }

    /// Moves a job between configured destinations on the local CUPS scheduler.
    pub async fn move_job(
        &self,
        source_printer_id: &str,
        job_id: i32,
        destination_printer_id: &str,
    ) -> Result<(), Error> {
        if source_printer_id == destination_printer_id {
            return Err(Error::InvalidMoveDestination {
                why: "source and destination queues are the same".to_string(),
            });
        }

        let printers = self.list_printers().await?;
        let source = printers
            .iter()
            .find(|printer| printer.id() == source_printer_id)
            .ok_or(Error::PrinterNotFound)?;
        let destination = printers
            .iter()
            .find(|printer| printer.id() == destination_printer_id)
            .ok_or(Error::PrinterNotFound)?;

        cups_backend::move_job(source, job_id, destination)
            .await
            .map_err(service_error)
    }

    /// Pauses a job.
    pub async fn pause_job(&self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::pause_job(&printer, job_id)
            .await
            .map_err(service_error)
    }

    /// Resumes a job.
    pub async fn resume_job(&self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::resume_job(&printer, job_id)
            .await
            .map_err(service_error)
    }

    /// Cancels a job.
    pub async fn cancel_job(&self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::cancel_job(&printer, job_id)
            .await
            .map_err(service_error)
    }

    async fn printer_entry(&self, printer_id: &str) -> Result<PrinterEntry, Error> {
        self.list_printers()
            .await?
            .into_iter()
            .find(|printer| printer.id() == printer_id)
            .ok_or(Error::PrinterNotFound)
    }
}

fn service_error(error: BackendError) -> Error {
    tracing::warn!(error = ?error, "printer backend request failed");
    error.into()
}
