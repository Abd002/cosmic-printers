use cosmic_settings_printers_core::{DiscoveredPrinter, Error, JobInfo, PrinterEntry};

use crate::{context::Context, cups_backend};

#[derive(Debug)]
pub struct Server {
    pub context: Context,
}

impl Server {
    pub async fn new(context: Context) -> Self {
        Self { context }
    }

    pub async fn list_printers(&mut self) -> Result<Vec<PrinterEntry>, Error> {
        let printers = cups_backend::list_printers().await?;
        self.context.model.lock().await.printers = printers.clone();
        Ok(printers)
    }

    pub async fn list_discovered_printers(&mut self) -> Result<Vec<DiscoveredPrinter>, Error> {
        cups_backend::list_discovered_printers().await
    }

    pub async fn add_discovered_printer(&mut self, printer_id: &str) -> Result<(), Error> {
        cups_backend::add_discovered_printer(printer_id).await?;
        self.list_printers().await?;
        Ok(())
    }

    pub async fn set_default(&mut self, printer_id: &str) -> Result<(), Error> {
        let printer_uri = self.printer_uri(printer_id).await?;

        cups_backend::set_default(printer_id, &printer_uri).await?;
        self.list_printers().await?;
        Ok(())
    }

    pub async fn print_test_page(&mut self, printer_id: &str) -> Result<i32, Error> {
        if self.context.model.lock().await.printers.is_empty() {
            self.list_printers().await?;
        }

        let printer = self
            .context
            .model
            .lock()
            .await
            .printers
            .iter()
            .find(|printer| printer.id == printer_id)
            .cloned()
            .ok_or(Error::PrinterNotFound)?;

        cups_backend::print_test_page(printer).await
    }

    pub async fn get_jobs(
        &mut self,
        printer_id: &str,
        filter: &str,
    ) -> Result<Vec<JobInfo>, Error> {
        self.printer_uri(printer_id).await?;
        cups_backend::get_jobs(printer_id, filter).await
    }

    pub async fn pause_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer_uri = self.printer_uri(printer_id).await?;
        cups_backend::pause_job(&printer_uri, job_id).await
    }

    pub async fn resume_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer_uri = self.printer_uri(printer_id).await?;
        cups_backend::resume_job(&printer_uri, job_id).await
    }

    pub async fn cancel_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer_uri = self.printer_uri(printer_id).await?;
        cups_backend::cancel_job(&printer_uri, job_id).await
    }

    async fn printer_uri(&mut self, printer_id: &str) -> Result<String, Error> {
        if self.context.model.lock().await.printers.is_empty() {
            self.list_printers().await?;
        }

        self.context
            .model
            .lock()
            .await
            .printers
            .iter()
            .find(|printer| printer.id == printer_id)
            .map(|printer| printer.printer_local_uri.clone())
            .ok_or(Error::PrinterNotFound)
    }
}
