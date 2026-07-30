use cosmic_settings_printers_core::{PrinterApplication, PrinterEntry};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub(crate) struct Model {
    pub(crate) printers: Vec<PrinterEntry>,
    pub(crate) discovered_printers: Vec<PrinterEntry>,
    pub(crate) printer_applications: HashMap<String, PrinterApplication>,
    pub(crate) discovery_running: bool,
    pub(crate) auto_add_in_progress: HashSet<String>,
}

impl Model {
    pub(crate) fn new() -> Self {
        Self {
            printers: Vec::new(),
            discovered_printers: Vec::new(),
            printer_applications: HashMap::new(),
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
