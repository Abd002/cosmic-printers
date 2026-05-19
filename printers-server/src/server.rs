use cosmic_settings_printers_core::{Error, PrinterEntry};

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
}
