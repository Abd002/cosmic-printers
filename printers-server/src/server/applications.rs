use cosmic_settings_printers_core::{Error, PrinterApplication};

use super::Server;

impl Server {
    /// Starts long-running DNS-SD discovery of local Printer Applications.
    pub async fn start_printer_application_discovery(&self) -> Result<(), Error> {
        crate::dnssd::start_printer_application_discovery(self.context.clone()).await;
        Ok(())
    }

    /// Lists the currently cached Printer Applications.
    pub async fn list_printer_applications(&self) -> Result<Vec<PrinterApplication>, Error> {
        Ok(self.context.printer_applications_cached().await)
    }
}
