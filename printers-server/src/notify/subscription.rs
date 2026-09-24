//! Asking the local server to keep a list of the changes worth reporting.

use cups_rs::{IppOperation, IppRequest, IppTag, IppValueTag};

use crate::error::{BackendError, BackendResult};
use crate::ipp::{
    CupsResultExt, add_requesting_user, ensure_success, send_on_default_connection,
    system_service_uri,
};

const SCHEDULER_URI: &str = "ipp://localhost/";

const CREATE_PRINTER_SUBSCRIPTIONS: u16 = 22;
const CREATE_SYSTEM_SUBSCRIPTIONS: u16 = 88;
const RENEW_SUBSCRIPTION: u16 = 26;

/// Long enough that renewing is rare, short enough that a subscription left behind
/// by a crash expires on its own.
pub(super) const LEASE_SECONDS: i32 = 3600;

/// The events worth waking the UI for. A server may add whatever it groups with them.
const EVENTS: &[&str] = &[
    "job-created",
    "job-completed",
    "job-progress",
    "job-state-changed",
    "job-stopped",
    "printer-added",
    "printer-created",
    "printer-deleted",
    "printer-modified",
    "printer-state-changed",
];

#[derive(Clone)]
pub(super) struct Subscription {
    pub(super) id: i32,
    uri: String,
    uri_attribute: &'static str,
}

impl Subscription {
    /// Subscribes to the local server, whichever kind it is.
    /// CUPS 2 keeps subscriptions on the scheduler and CUPS 3 on its system service,
    /// and neither answers the other's request, so the one that works settles it.
    pub(super) fn create() -> BackendResult<Self> {
        let scheduler_error =
            match Self::create_on(SCHEDULER_URI, "printer-uri", CREATE_PRINTER_SUBSCRIPTIONS) {
                Ok(subscription) => return Ok(subscription),
                Err(error) => error,
            };

        let Some(system_uri) = system_service_uri(SCHEDULER_URI) else {
            return Err(scheduler_error);
        };

        Self::create_on(&system_uri, "system-uri", CREATE_SYSTEM_SUBSCRIPTIONS)
            .map_err(|_| scheduler_error)
    }

    fn create_on(uri: &str, uri_attribute: &'static str, operation: u16) -> BackendResult<Self> {
        let mut request = IppRequest::new(IppOperation::Other(operation)).cups_err()?;

        request
            .add_string(IppTag::Operation, IppValueTag::Uri, uri_attribute, uri)
            .cups_err()?;
        add_requesting_user(&mut request)?;
        request
            .add_string(
                IppTag::Subscription,
                IppValueTag::Keyword,
                "notify-pull-method",
                "ippget",
            )
            .cups_err()?;
        request
            .add_strings(
                IppTag::Subscription,
                IppValueTag::Keyword,
                "notify-events",
                EVENTS,
            )
            .cups_err()?;
        request
            .add_integer(
                IppTag::Subscription,
                IppValueTag::Integer,
                "notify-lease-duration",
                LEASE_SECONDS,
            )
            .cups_err()?;

        let response = send_on_default_connection(request, uri)?;
        ensure_success(&response, "Create-Subscription")?;

        let id = response
            .find_attribute("notify-subscription-id", None)
            .map(|attribute| attribute.get_integer(0))
            .ok_or_else(|| {
                BackendError::Internal("subscription created without an id".to_string())
            })?;

        Ok(Self {
            id,
            uri: uri.to_string(),
            uri_attribute,
        })
    }

    pub(super) fn uri(&self) -> &str {
        &self.uri
    }

    pub(super) fn uri_attribute(&self) -> &'static str {
        self.uri_attribute
    }

    pub(super) fn renew(&self) -> BackendResult<()> {
        let mut request = IppRequest::new(IppOperation::Other(RENEW_SUBSCRIPTION)).cups_err()?;

        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                self.uri_attribute,
                &self.uri,
            )
            .cups_err()?;
        request
            .add_integer(
                IppTag::Operation,
                IppValueTag::Integer,
                "notify-subscription-id",
                self.id,
            )
            .cups_err()?;
        add_requesting_user(&mut request)?;
        request
            .add_integer(
                IppTag::Subscription,
                IppValueTag::Integer,
                "notify-lease-duration",
                LEASE_SECONDS,
            )
            .cups_err()?;

        let response = send_on_default_connection(request, &self.uri)?;

        ensure_success(&response, "Renew-Subscription")
    }
}
