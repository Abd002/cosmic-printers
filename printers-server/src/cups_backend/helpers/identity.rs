/// Splits a CUPS destination id into its queue name and optional instance.
pub(in crate::cups_backend) fn split_queue_instance(printer_id: &str) -> (&str, Option<&str>) {
    printer_id
        .split_once('/')
        .map_or((printer_id, None), |(name, instance)| {
            (name, Some(instance))
        })
}

/// Constructs the local scheduler URI for a queue or printer class.
pub(in crate::cups_backend) fn local_printer_uri(printer_id: &str, is_class: bool) -> String {
    let queue_name = split_queue_instance(printer_id).0;
    let path = if is_class { "classes" } else { "printers" };

    if queue_name.is_empty() {
        "ipp://localhost/".to_string()
    } else {
        format!("ipp://localhost/{path}/{queue_name}")
    }
}
