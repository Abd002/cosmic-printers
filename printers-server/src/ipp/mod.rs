//! IPP mechanics: reading a URI, opening a connection, building a request.

mod connection;
mod request;
mod uri;

pub(crate) use connection::{
    IppTimeouts, send_on_default_connection, send_to, send_to_with_timeouts,
};
pub(crate) use request::{
    CupsResultExt, add_requesting_user, ensure_success, printer_attrs_request,
};
pub(crate) use uri::{
    is_local_scheduler_uri, loopback_uri, parse_uri_endpoint, system_service_uri,
};
