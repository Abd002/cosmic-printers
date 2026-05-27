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

    pub async fn set_default(&mut self, id: &str, password: String) -> Result<(), Error> {
        if self.context.model.lock().await.printers.is_empty() {
            self.list_printers().await?;
        }

        let printer_uri = self
            .context
            .model
            .lock()
            .await
            .printers
            .iter()
            .find(|printer| printer.id == id)
            .map(|printer| printer.printer_uri.clone())
            .ok_or(Error::PrinterNotFound)?;

        cups_backend::set_default(&printer_uri, password).await?;
        self.list_printers().await?;
        Ok(())
    }

    pub async fn get_jobs(&mut self, name: &str, filter: &str) -> Result<Vec<JobInfo>, Error> {
        cups_backend::get_jobs(name, filter).await
    }

    pub async fn pause_job(&mut self, printer_uri: &str, id: i32) -> Result<(), Error> {
        cups_backend::pause_job(&printer_uri, id).await
    }

    pub async fn resume_job(&mut self, printer_uri: &str, id: i32) -> Result<(), Error> {
        cups_backend::resume_job(&printer_uri, id).await
    }

    pub async fn cancel_job(&mut self, printer_uri: &str, id: i32) -> Result<(), Error> {
        cups_backend::cancel_job(&printer_uri, id).await
    }
}
