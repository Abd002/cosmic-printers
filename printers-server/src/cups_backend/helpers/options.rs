use std::collections::HashMap;

/// Checks the CUPS printer-type bitmask for the class flag.
pub(super) fn is_printer_class(options: &HashMap<String, String>) -> bool {
    options
        .get("printer-type")
        .and_then(|printer_type| printer_type.parse::<u32>().ok())
        .is_some_and(|printer_type| printer_type & cups_rs::PRINTER_CLASS != 0)
}

/// Splits a comma-separated CUPS option into trimmed values.
pub(super) fn option_values(options: &HashMap<String, String>, name: &str) -> Vec<String> {
    options
        .get(name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(in crate::cups_backend) fn queue_name_from_printer_uri(uri: &str) -> Option<String> {
    let path = uri.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?.trim();

    (!name.is_empty()).then(|| name.to_string())
}
