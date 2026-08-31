//! Naming the local CUPS scheduler.
//! Centralizing these URIs keeps routing and scheduler-resource checks consistent.

use cups_rs::{IppRequest, IppResponse};

use crate::error::BackendResult;
use crate::ipp::{
    IppTimeouts, is_local_scheduler_uri, send_on_default_connection, send_to_with_timeouts,
};

/// Where the scheduler keeps its jobs.
pub(super) const CUPS_JOBS_URI: &str = "ipp://localhost/jobs";

/// Where the scheduler takes the operations that predate the system service.
pub(super) const SCHEDULER_ADMIN_URI: &str = "ipp://localhost/admin/";

/// Splits a CUPS destination id into its queue name and optional instance.
pub(super) fn split_queue_instance(printer_id: &str) -> (&str, Option<&str>) {
    printer_id
        .split_once('/')
        .map_or((printer_id, None), |(name, instance)| {
            (name, Some(instance))
        })
}

/// Constructs the local scheduler URI for a queue or printer class.
pub(super) fn local_printer_uri(printer_id: &str, is_class: bool) -> String {
    let queue_name = split_queue_instance(printer_id).0;
    let path = if is_class { "classes" } else { "printers" };

    if queue_name.is_empty() {
        "ipp://localhost/".to_string()
    } else {
        format!("ipp://localhost/{path}/{queue_name}")
    }
}

/// Sends a request to a CUPS destination over its required transport.
pub(super) fn send(request: IppRequest, uri: &str) -> BackendResult<IppResponse> {
    send_with_timeouts(request, uri, IppTimeouts::default())
}

/// Sends a request to a CUPS destination with explicit timeouts.
pub(super) fn send_with_timeouts(
    request: IppRequest,
    uri: &str,
    timeouts: IppTimeouts,
) -> BackendResult<IppResponse> {
    if is_local_scheduler_uri(uri) {
        return send_on_default_connection(request, uri);
    }

    send_to_with_timeouts(request, uri, timeouts)
}
