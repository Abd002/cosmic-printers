use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, ConfigureDiscoveredPrinterRequest, ConfigurePrinterReply, Error,
    ListManualSetupApplicationsReply, StartAddPrinterDiscoveryReply,
};

use super::Server;
use crate::printer_app;

impl Server {
    /// Starts asynchronous discovery and returns the generation required by configuration requests.
    pub async fn start_add_printer_discovery(
        &self,
    ) -> Result<StartAddPrinterDiscoveryReply, Error> {
        Ok(printer_app::start_add_printer_discovery(self.context.clone()).await)
    }

    /// Returns the current Add Printer discovery results.
    pub async fn get_add_printer_discovery(&self) -> Result<AddPrinterDiscoveryReply, Error> {
        Ok(self.context.add_printer_discovery_reply())
    }

    /// Configures a discovered printer through the Printer Application chosen.
    pub async fn configure_discovered_printer(
        &self,
        request: ConfigureDiscoveredPrinterRequest,
    ) -> Result<ConfigurePrinterReply, Error> {
        printer_app::configure_discovered_printer(&self.context, request).await
    }

    /// Returns the state of an earlier configuration attempt.
    pub async fn get_printer_configuration(
        &self,
        operation_id: &str,
    ) -> Result<ConfigurePrinterReply, Error> {
        self.context.printer_configuration(operation_id)
    }

    /// Lists Printer Applications that can be set up through their own interface.
    pub async fn list_manual_setup_printer_applications(
        &self,
    ) -> Result<ListManualSetupApplicationsReply, Error> {
        Ok(printer_app::manual_setup_applications(&self.context).await)
    }
}
