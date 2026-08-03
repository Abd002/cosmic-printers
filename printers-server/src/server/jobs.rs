use cosmic_settings_printers_core::{Error, JobInfo};

use super::{Server, service_error};
use crate::cups;

impl Server {
    /// Prints a test page and returns its job ID.
    pub async fn print_test_page(&self, printer_id: &str) -> Result<i32, Error> {
        let printer = self.printer_entry(printer_id).await?;

        cups::print_test_page(printer).await.map_err(service_error)
    }

    /// Lists jobs for a configured printer.
    pub async fn get_jobs(&self, printer_id: &str, filter: &str) -> Result<Vec<JobInfo>, Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups::get_jobs(&printer, filter)
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

        cups::move_job(source, job_id, destination)
            .await
            .map_err(service_error)
    }

    /// Pauses a job.
    pub async fn pause_job(&self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups::pause_job(&printer, job_id)
            .await
            .map_err(service_error)
    }

    /// Resumes a job.
    pub async fn resume_job(&self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups::resume_job(&printer, job_id)
            .await
            .map_err(service_error)
    }

    /// Cancels a job.
    pub async fn cancel_job(&self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer = self.printer_entry(printer_id).await?;
        cups::cancel_job(&printer, job_id)
            .await
            .map_err(service_error)
    }
}
