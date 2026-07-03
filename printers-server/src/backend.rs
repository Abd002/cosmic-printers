use cosmic_settings_printers_core::PrinterEntry;

#[derive(Debug)]
pub struct Model {
    pub printers: Vec<PrinterEntry>,
    pub default_printer: Option<String>,
    pub discovered_printers: Vec<PrinterEntry>,
    pub discovery_running: bool,
}

impl Model {
    pub fn new() -> Self {
        Self {
            printers: Vec::new(),
            default_printer: None,
            discovered_printers: Vec::new(),
            discovery_running: false,
        }
    }
}
