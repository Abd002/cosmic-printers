mod attributes;
mod conversion;
mod destinations;
mod identity;

pub(super) use crate::ipp::{CupsResultExt, add_requesting_user, ensure_success, send_ipp_request};
pub(super) use attributes::{PRINTER_ATTRIBUTES, fill_missing_attrs};
pub(super) use destinations::available_destinations;
pub(super) use identity::{local_printer_uri, split_queue_instance};
