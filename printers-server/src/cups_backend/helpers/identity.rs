use cosmic_settings_printers_core::PrinterEntry;

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
