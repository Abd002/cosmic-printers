mod attributes;
mod conversion;
mod destinations;
mod identity;

pub(super) use crate::ipp::{CupsResultExt, add_requesting_user, ensure_success, send_ipp_request};
pub(super) use attributes::{
    PRINTER_ATTRIBUTES, fill_missing_attrs_from_device_uri, fill_missing_attrs_from_printer_uri,
    printer_uri_from_parts, request_scheme,
};
pub(super) use conversion::destination_to_printer_entry;
pub(super) use destinations::available_destinations;
pub(super) use identity::split_queue_instance;
