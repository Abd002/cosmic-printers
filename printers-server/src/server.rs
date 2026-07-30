use cosmic_settings_printers_core::{
    Error, JobInfo, PrinterApplication, PrinterEntry, PrintersEvent,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::broadcast;

use crate::{context::Context, cups_backend};

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
    pub async fn list_printers(&mut self) -> Result<Vec<PrinterEntry>, Error> {
        let printers = cups_backend::list_printers().await?;
        self.context.set_printers(printers.clone()).await;
        Ok(printers)
    }

    /// Lists printers discovered from network discovery.
    pub async fn list_discovered_printers(&mut self) -> Result<Vec<PrinterEntry>, Error> {
        cups_backend::list_discovered_printers(self.context.clone()).await
    }

    /// Lists discovered Printer Applications.
    pub async fn list_printer_applications(&mut self) -> Result<Vec<PrinterApplication>, Error> {
        cups_backend::start_discovery(self.context.clone()).await;
        Ok(self.context.list_printer_applications().await)
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

    /// Adds a discovered printer to the configured CUPS queues.
    pub async fn add_discovered_printer(&mut self, printer_id: &str) -> Result<(), Error> {
        let printer = self
            .context
            .discovered_printer(printer_id)
            .await
            .ok_or(Error::PrinterNotFound)?;
        let actual_queue_name = cups_backend::add_discovered_printer(printer).await?;
        self.context
            .update_discovered_printer(printer_id, |printer| {
                printer.id = actual_queue_name;
            })
            .await;

        self.list_printers().await?;
        Ok(())
    }

    /// Deletes a configured printer.
    pub async fn delete_printer(&mut self, printer_id: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::delete_printer(printer_id).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Enables or disables accepting jobs for a printer.
    pub async fn set_printer_accept_jobs(
        &mut self,
        printer_id: &str,
        enabled: bool,
        reason: &str,
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_accept_jobs(printer_id, enabled, reason).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Sets the system default printer.
    pub async fn set_printer_default(&mut self, printer_id: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_default(printer_id).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Sets a default printer option value.
    pub async fn set_printer_option_default(
        &mut self,
        printer_id: &str,
        option: &str,
        values: &[String],
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_option_default(printer_id, option, values).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Enables or disables a printer.
    pub async fn set_printer_enabled(
        &mut self,
        printer_id: &str,
        enabled: bool,
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_enabled(printer_id, enabled).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Sets the printer information string.
    pub async fn set_printer_info(&mut self, printer_id: &str, info: &str) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_info(printer_id, info).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Sets the printer location.
    pub async fn set_printer_location(
        &mut self,
        printer_id: &str,
        location: &str,
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_location(printer_id, location).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Enables or disables printer sharing.
    pub async fn set_printer_shared(
        &mut self,
        printer_id: &str,
        shared: bool,
    ) -> Result<(), Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::set_printer_shared(printer_id, shared).await?;
        self.list_printers().await?;
        Ok(())
    }

    /// Prints a test page and returns its job ID.
    pub async fn print_test_page(&mut self, printer_id: &str) -> Result<i32, Error> {
        let printer = self.printer_entry(printer_id).await?;

        cups_backend::print_test_page(printer).await
    }

    /// Lists jobs for a configured printer.
    pub async fn get_jobs(
        &mut self,
        printer_id: &str,
        filter: &str,
    ) -> Result<Vec<JobInfo>, Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::get_jobs(&printer, filter).await
    }

    /// Pauses a job.
    pub async fn pause_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::pause_job(&printer, job_id).await
    }

    /// Resumes a job.
    pub async fn resume_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::resume_job(&printer, job_id).await
    }

    /// Cancels a job.
    pub async fn cancel_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups_backend::cancel_job(&printer, job_id).await
    }

    async fn printer_entry(&mut self, printer_id: &str) -> Result<PrinterEntry, Error> {
        self.list_printers()
            .await?
            .into_iter()
            .find(|printer| printer.id == printer_id)
            .ok_or(Error::PrinterNotFound)
    }
}
