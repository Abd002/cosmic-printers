use cosmic_settings_printers_core::PrinterEntry;
use std::collections::HashSet;

#[derive(Debug)]
pub struct Model {
    pub printers: Vec<PrinterEntry>,
    pub default_printer: Option<String>,
    pub discovered_printers: Vec<PrinterEntry>,
    pub discovery_running: bool,
    pub auto_add_in_progress: HashSet<String>,
}

impl Model {
    pub fn new() -> Self {
        Self {
            printers: Vec::new(),
            default_printer: None,
            discovered_printers: Vec::new(),
            discovery_running: false,
            auto_add_in_progress: HashSet::new(),
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}
