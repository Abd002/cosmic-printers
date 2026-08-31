//! The methods the daemon exposes, one file per subject.

mod add_printer;
mod applications;
mod events;
mod jobs;
mod printers;
mod supplies;

use cosmic_settings_printers_core::{Error, PrinterEntry};

use crate::error::BackendError;
use crate::state::State;

#[derive(Debug)]
/// The server-side implementation of the COSMIC printers interface.
pub struct Server {
    context: State,
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
            context: State::new(),
        }
    }

    async fn printer_entry(&self, printer_id: &str) -> Result<PrinterEntry, Error> {
        self.get_printer(printer_id).await
    }
}

fn service_error(error: BackendError) -> Error {
    tracing::warn!(error = ?error, "printer backend request failed");
    error.into()
}
