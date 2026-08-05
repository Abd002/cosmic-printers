//! Shared transport for talking to a Printer Application's IPP System Service.
//!
//! Every operation goes to the same place — the application's `/ipp/system`
//! endpoint — and fails in the same handful of ways, so request construction,
//! status mapping, and timeouts live here rather than in each operation.
//!
//! Raw protocol detail never leaves this module in a public error. A caller gets
//! a [`PaError`] describing what happened in terms the flow can act on.

use cups_rs::{IppOperation, IppRequest, IppResponse, IppStatus, IppTag, IppValueTag};
use std::time::Duration;

use crate::error::BackendError;
use crate::ipp::{CupsResultExt, IppTimeouts, add_requesting_user, send_ipp_request_with_timeouts};

/// How long to allow for connecting to a Printer Application.
///
/// Longer than a printer lookup, because the application may be starting up or
/// busy serving a job.
const CONNECT_TIMEOUT_MS: i32 = 2_000;

/// How long to allow a device scan to run.
///
/// A Printer Application rescans USB, SNMP, and DNS-SD on every
/// `PAPPL-Find-Devices`; there is no cached list to read. An SNMP sweep of a
/// quiet network is the slow part.
const DEVICE_SCAN_TIMEOUT: Duration = Duration::from_secs(45);

/// How long to allow for operations that only consult local state.
const QUERY_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to allow for creating a printer.
///
/// Creation opens the device and loads a driver, so it is slower than a query but
/// bounded well below a device scan.
const CREATE_TIMEOUT: Duration = Duration::from_secs(30);

/// Why an operation against a Printer Application did not produce a result.
#[derive(Debug)]
pub(crate) enum PaError {
    /// The application could not be reached, or stopped responding.
    Unreachable { why: String },
    /// The application wants credentials. Setup has to continue in its own web
    /// interface; this service never collects or forwards a password.
    AuthenticationRequired,
    /// The application refused the request outright. Remote administration is
    /// refused by design unless the application was configured with an
    /// authentication service.
    Forbidden { why: String },
    /// The application does not implement the operation.
    OperationNotSupported,
    /// The application answered with a status that stops the flow.
    Rejected { status: String, why: String },
    /// The application answered, but the response did not follow the protocol.
    Malformed { why: String },
}

impl PaError {
    fn from_status(response: &IppResponse) -> Self {
        let status = response.status();
        let why = status_message(response).unwrap_or_default();

        match status {
            IppStatus::ErrorNotAuthenticated | IppStatus::ErrorNotAuthorized => {
                Self::AuthenticationRequired
            }
            IppStatus::ErrorForbidden => Self::Forbidden { why },
            IppStatus::ErrorOperationNotSupported => Self::OperationNotSupported,
            _ => Self::Rejected {
                status: format!("{status:?} (0x{:04x})", response.status_code()),
                why,
            },
        }
    }

    fn from_backend(error: BackendError) -> Self {
        match error {
            BackendError::DeviceUnreachable { .. } => Self::Unreachable {
                why: error.to_string(),
            },
            BackendError::PermissionDenied { .. } => Self::AuthenticationRequired,
            // A request that produced no response at all is indistinguishable
            // from the peer going away, and is treated as such: retrying a
            // create-printer blindly could produce a second printer.
            other => Self::Unreachable {
                why: other.to_string(),
            },
        }
    }

    pub(crate) fn malformed(why: impl Into<String>) -> Self {
        Self::Malformed { why: why.into() }
    }
}

/// Which timeout an operation should use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OperationCost {
    /// Consults state the application already has.
    Query,
    /// Rescans hardware.
    DeviceScan,
    /// Opens a device and loads a driver.
    Create,
}

impl OperationCost {
    fn timeouts(self) -> IppTimeouts {
        let response = match self {
            Self::Query => QUERY_TIMEOUT,
            Self::DeviceScan => DEVICE_SCAN_TIMEOUT,
            Self::Create => CREATE_TIMEOUT,
        };

        IppTimeouts {
            connect_ms: CONNECT_TIMEOUT_MS,
            response_seconds: response.as_secs_f64(),
        }
    }
}

/// A request being built against a Printer Application's system service.
pub(super) struct PaRequest {
    request: IppRequest,
}

impl PaRequest {
    /// Starts a request, addressed to the system service and identifying the
    /// calling user.
    pub(super) fn new(operation: IppOperation, system_uri: &str) -> Result<Self, PaError> {
        let mut request = IppRequest::new(operation)
            .cups_err()
            .map_err(PaError::from_backend)?;
        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                "system-uri",
                system_uri,
            )
            .cups_err()
            .map_err(PaError::from_backend)?;
        add_requesting_user(&mut request).map_err(PaError::from_backend)?;

        Ok(Self { request })
    }

    /// Adds a single-valued string attribute.
    pub(super) fn string(
        mut self,
        group: IppTag,
        value_tag: IppValueTag,
        name: &str,
        value: &str,
    ) -> Result<Self, PaError> {
        self.request
            .add_string(group, value_tag, name, value)
            .cups_err()
            .map_err(PaError::from_backend)?;

        Ok(self)
    }

    /// Adds a multi-valued keyword attribute, skipping an empty list.
    pub(super) fn keywords(mut self, name: &str, values: &[&str]) -> Result<Self, PaError> {
        if values.is_empty() {
            return Ok(self);
        }
        self.request
            .add_strings(IppTag::Operation, IppValueTag::Keyword, name, values)
            .cups_err()
            .map_err(PaError::from_backend)?;

        Ok(self)
    }

    /// Sends the request and checks the status before any attribute is read.
    ///
    /// A response is only returned when the application reported success, so no
    /// caller can accidentally interpret the attributes of a failure.
    pub(super) fn send(
        self,
        system_uri: &str,
        cost: OperationCost,
    ) -> Result<IppResponse, PaError> {
        let response = send_ipp_request_with_timeouts(self.request, system_uri, cost.timeouts())
            .map_err(PaError::from_backend)?;

        if response.status().is_successful() {
            Ok(response)
        } else {
            Err(PaError::from_status(&response))
        }
    }

    /// Sends the request and returns the response whatever the status.
    ///
    /// Used where a failure status carries information the caller needs, such as
    /// the attribute a `Create-Printer` rejection names as unsupported.
    pub(super) fn send_allowing_failure(
        self,
        system_uri: &str,
        cost: OperationCost,
    ) -> Result<IppResponse, PaError> {
        send_ipp_request_with_timeouts(self.request, system_uri, cost.timeouts())
            .map_err(PaError::from_backend)
    }
}

/// Returns the human-readable reason a Printer Application gave for a status.
pub(super) fn status_message(response: &IppResponse) -> Option<String> {
    response
        .find_attribute("status-message", None)
        .and_then(|attribute| attribute.get_string(0))
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

/// Maps a response status to an error without discarding a successful response.
pub(super) fn check_status(response: &IppResponse) -> Result<(), PaError> {
    if response.status().is_successful() {
        Ok(())
    } else {
        Err(PaError::from_status(response))
    }
}

/// Caps how much of a response is read.
///
/// A Printer Application is not hostile, but it is a separate process that can
/// be buggy or wedged, and a scan of a large network can legitimately return a
/// lot. Reading an unbounded amount into memory is not worth the risk.
pub(super) const MAX_COLLECTIONS: usize = 512;

/// Caps the length of a string read out of a response.
pub(super) const MAX_STRING_LENGTH: usize = 1_024;

/// Truncates an over-long value rather than rejecting the whole response.
pub(super) fn bounded(value: String) -> String {
    if value.len() <= MAX_STRING_LENGTH {
        return value;
    }

    let mut end = MAX_STRING_LENGTH;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn over_long_values_are_truncated_on_a_character_boundary() {
        let value = "é".repeat(MAX_STRING_LENGTH);
        let bounded = bounded(value);

        assert!(bounded.len() <= MAX_STRING_LENGTH);
        // Truncating mid-character would have produced invalid UTF-8, which
        // cannot be represented in a String at all, so reaching here with a
        // shorter string is the assertion.
        assert!(bounded.chars().all(|character| character == 'é'));
    }

    #[test]
    fn values_within_the_limit_are_untouched() {
        assert_eq!(bounded("device".to_string()), "device");
    }

    #[test]
    fn device_scans_are_allowed_far_longer_than_queries() {
        assert!(
            OperationCost::DeviceScan.timeouts().response_seconds
                > OperationCost::Query.timeouts().response_seconds
        );
        assert_eq!(
            OperationCost::Query.timeouts().connect_ms,
            CONNECT_TIMEOUT_MS
        );
    }
}
