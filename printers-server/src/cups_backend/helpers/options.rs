use std::collections::HashMap;

/// Checks the CUPS printer-type bitmask for the class flag.
pub(super) fn is_printer_class(options: &HashMap<String, String>) -> bool {
    options
        .get("printer-type")
        .and_then(|printer_type| printer_type.parse::<u32>().ok())
        .is_some_and(|printer_type| printer_type & cups_rs::PRINTER_CLASS != 0)
}

pub(in crate::cups_backend) fn queue_name_from_printer_uri(uri: &str) -> Option<String> {
    let path = uri.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?.trim();

    (!name.is_empty()).then(|| name.to_string())
}
