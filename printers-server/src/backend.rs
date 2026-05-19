use cosmic_settings_printers_core::PrinterEntry;

#[derive(Debug)]
pub struct Model {
    pub printers: Vec<PrinterEntry>,
    pub default_printer: Option<String>,
}

impl Model {
    pub fn new() -> Self {
        Self {
            printers: Vec::new(),
            default_printer: None,
        }
    }
}
