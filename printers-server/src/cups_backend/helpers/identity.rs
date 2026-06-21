use cosmic_settings_printers_core::{DeviceIdentity, PrinterEntry};

use super::options::non_empty_option;

/// Splits a CUPS destination id into its queue name and optional instance.
pub(in crate::cups_backend) fn split_queue_instance(printer_id: &str) -> (&str, Option<&str>) {
    printer_id
        .split_once('/')
        .map_or((printer_id, None), |(name, instance)| {
            (name, Some(instance))
        })
}

/// Returns the CUPS queue name portion of a printer entry id.
pub(in crate::cups_backend) fn printer_queue_name(printer: &PrinterEntry) -> &str {
    split_queue_instance(&printer.id).0
}

/// Checks whether two printer entries refer to the same physical device.
pub(in crate::cups_backend) fn printers_match(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    if printer_identity(left).matches(&printer_identity(right)) {
        return true;
    }

    cups_browsed_name_matches(left, right)
}

pub(in crate::cups_backend) fn printer_identity(printer: &PrinterEntry) -> DeviceIdentity {
    DeviceIdentity::new(
        non_empty_option(&printer.options, "device-uuid"),
        printer
            .hostname
            .as_ref()
            .zip(printer.port)
            .map(|(host, port)| (host.clone(), port)),
        non_empty_option(&printer.options, "device-uri"),
        non_empty_option(&printer.options, "printer-uri-supported"),
    )
}

/// Matches a cups-browsed queue with its DNS-SD destination by queue name.
fn cups_browsed_name_matches(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    (left.options.contains_key("cups-browsed") || right.options.contains_key("cups-browsed"))
        && left.id.eq_ignore_ascii_case(&right.id)
}
