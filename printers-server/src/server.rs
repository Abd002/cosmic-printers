use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, ConfigureDiscoveredPrinterRequest, ConfigurePrinterReply, Error,
    JobInfo, ListManualSetupApplicationsReply, PrinterApplication, PrinterEntry, PrintersEvent,
    StartAddPrinterDiscoveryReply, SupplyLevel,
};
use futures_util::{Stream, StreamExt};
use tokio::sync::broadcast;

use crate::{context::Context, cups_backend, error::BackendError, printer_application_backend};

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

    /// Lists the currently cached libcups destinations without performing I/O.
    pub async fn list_printers(&self) -> Result<Vec<PrinterEntry>, Error> {
        Ok(self.context.available_destinations_cached().await)
    }

    /// Starts a background libcups refresh of the available destination cache.
    pub async fn refresh_available_destinations(&self) -> Result<(), Error> {
        cups_backend::refresh_available_destinations(self.context.clone());
        Ok(())
    }

    /// Starts long-running DNS-SD discovery of local Printer Applications.
    pub async fn start_printer_application_discovery(&self) -> Result<(), Error> {
        crate::dnssd::start_printer_application_discovery(self.context.clone()).await;
        Ok(())
    }

    /// Lists the currently cached Printer Applications.
    pub async fn list_printer_applications(&self) -> Result<Vec<PrinterApplication>, Error> {
        Ok(self.context.printer_applications_cached().await)
    }

    /// Starts a round of Add Printer discovery and returns its generation.
    ///
    /// Returns immediately; results arrive per Printer Application and are read
    /// with [`Server::get_add_printer_discovery`]. Every configuration request
    /// must quote the generation returned here, so a selection made against
    /// results that have since been replaced is refused rather than acted on.
    pub async fn start_add_printer_discovery(
        &self,
    ) -> Result<StartAddPrinterDiscoveryReply, Error> {
        Ok(printer_application_backend::start_add_printer_discovery(self.context.clone()).await)
    }

    /// Returns the current Add Printer discovery results.
    pub async fn get_add_printer_discovery(&self) -> Result<AddPrinterDiscoveryReply, Error> {
        Ok(self.context.add_printer_discovery_reply())
    }

    /// Configures a discovered printer through the Printer Application chosen.
    ///
    /// The reply says the printer was created, not that a destination exists yet.
    /// The Printer Application advertises the new printer, the ordinary
    /// destination pipeline discovers it, and the attempt then reconciles to that
    /// destination — poll [`Server::get_printer_configuration`] to see it happen.
    pub async fn configure_discovered_printer(
        &self,
        request: ConfigureDiscoveredPrinterRequest,
    ) -> Result<ConfigurePrinterReply, Error> {
        printer_application_backend::configure_discovered_printer(&self.context, request).await
    }

    /// Returns the state of an earlier configuration attempt.
    pub async fn get_printer_configuration(
        &self,
        operation_id: &str,
    ) -> Result<ConfigurePrinterReply, Error> {
        printer_application_backend::printer_configuration(&self.context, operation_id)
    }

    /// Lists Printer Applications that can be set up through their own interface.
    ///
    /// Includes applications that found no devices and ones that need
    /// credentials, since those are exactly the ones a user needs to open when
    /// Add Printer came up empty.
    pub async fn list_manual_setup_printer_applications(
        &self,
    ) -> Result<ListManualSetupApplicationsReply, Error> {
        Ok(printer_application_backend::manual_setup_applications(&self.context).await)
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

    /// Asks a printer what supplies it has and how full they are.
    pub async fn printer_supplies(&self, printer_id: &str) -> Result<Vec<SupplyLevel>, Error> {
        let printer = self.printer_entry(printer_id).await?;

        cups_backend::printer_supplies(printer)
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
