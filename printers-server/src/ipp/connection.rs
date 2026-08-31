//! Opening a connection and sending a request on it.

use cups_rs::{HttpConnection, IppRequest, IppResponse};

use super::request::CupsResultExt;
use super::uri::ParsedUri;
use crate::error::{BackendError, BackendResult};

/// How long to wait when a peer is expected to answer immediately.
const DEFAULT_CONNECT_TIMEOUT_MS: i32 = 250;

/// How long an individual request may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct IppTimeouts {
    /// Bound on establishing the connection.
    pub connect_ms: i32,
    /// Bound on waiting for the reply, once connected.
    pub response_seconds: f64,
}

impl Default for IppTimeouts {
    fn default() -> Self {
        Self {
            connect_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            response_seconds: 0.0,
        }
    }
}

/// Sends a request to whatever the URI names, over a connection opened for it.
pub(crate) fn send_to(request: IppRequest, uri: &str) -> BackendResult<IppResponse> {
    send_to_with_timeouts(request, uri, IppTimeouts::default())
}

/// Sends a request with explicit timeouts.
pub(crate) fn send_to_with_timeouts(
    request: IppRequest,
    uri: &str,
    timeouts: IppTimeouts,
) -> BackendResult<IppResponse> {
    let uri_parts = parse(uri)?;
    let mut connection = HttpConnection::connect_host_with_encryption(
        &uri_parts.host,
        uri_parts.port,
        uri_parts.resource_path(),
        uri_parts.encryption(),
        Some(timeouts.connect_ms),
    )
    .map_err(|source| BackendError::DeviceUnreachable {
        uri: uri.to_string(),
        source,
    })?;
    if timeouts.response_seconds > 0.0 {
        connection.set_timeout(timeouts.response_seconds);
    }

    request
        .send(&connection, connection.resource_path())
        .cups_err()
}

/// Uses libcups' default connection so the scheduler can authenticate Unix-socket peer credentials.
pub(crate) fn send_on_default_connection(
    request: IppRequest,
    uri: &str,
) -> BackendResult<IppResponse> {
    let uri_parts = parse(uri)?;

    request.send_default(uri_parts.resource_path()).cups_err()
}

fn parse(uri: &str) -> BackendResult<ParsedUri> {
    ParsedUri::parse(uri)
        .filter(|uri| uri.scheme.is_ipp())
        .ok_or_else(|| BackendError::Internal(format!("invalid IPP URI: {uri}")))
}
