use cosmic_settings_printers_core::{Error, JobInfo, PrinterEntry};

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

    pub async fn list_discovered_printers(&mut self) -> Result<Vec<PrinterEntry>, Error> {
        cups_backend::list_discovered_printers().await
    }

    pub async fn add_discovered_printer(&mut self, printer_id: &str) -> Result<(), Error> {
        cups_backend::add_discovered_printer(printer_id).await?;
        self.list_printers().await?;
        Ok(())
    }

    pub async fn set_default(&mut self, printer_id: &str) -> Result<(), Error> {
        let printer_uri = self.printer_entry(printer_id).await?.printer_local_uri;

        cups_backend::set_default(printer_id, &printer_uri).await?;
        self.list_printers().await?;
        Ok(())
    }

    pub async fn print_test_page(&mut self, printer_id: &str) -> Result<i32, Error> {
        let printer = self.printer_entry(printer_id).await?;

        cups_backend::print_test_page(printer).await
    }

    pub async fn get_jobs(
        &mut self,
        printer_id: &str,
        filter: &str,
    ) -> Result<Vec<JobInfo>, Error> {
        self.printer_entry(printer_id).await?;
        cups_backend::get_jobs(printer_id, filter).await
    }

    pub async fn pause_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer_uri = self.printer_entry(printer_id).await?.printer_local_uri;
        cups_backend::pause_job(&printer_uri, job_id).await
    }

    pub async fn resume_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer_uri = self.printer_entry(printer_id).await?.printer_local_uri;
        cups_backend::resume_job(&printer_uri, job_id).await
    }

    pub async fn cancel_job(&mut self, printer_id: &str, job_id: i32) -> Result<(), Error> {
        let printer_uri = self.printer_entry(printer_id).await?.printer_local_uri;
        cups_backend::cancel_job(&printer_uri, job_id).await
    }

    async fn printer_entry(&mut self, printer_id: &str) -> Result<PrinterEntry, Error> {
        self.list_printers()
            .await?
            .into_iter()
            .find(|printer| printer.id == printer_id)
            .ok_or(Error::PrinterNotFound)
    }
}
