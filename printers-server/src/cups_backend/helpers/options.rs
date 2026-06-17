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

/// Reads a trimmed option and treats missing or empty values as absent.
pub(super) fn non_empty_option<'a>(
    options: &'a HashMap<String, String>,
    name: &str,
) -> Option<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
