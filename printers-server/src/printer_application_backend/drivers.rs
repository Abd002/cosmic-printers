//! `PAPPL-Find-Drivers`: does this Printer Application have a driver for this
//! device?
//!
//! This is the step that separates "an application can see the printer" from "an
//! application can drive the printer". Find-Devices does no driver filtering, so
//! an application will report a printer it cannot use; showing it as a
//! configuration candidate would send the user down a path that fails at the
//! end.
//!
//! Driver identifiers are scoped to the application that reported them. A driver
//! name from one application is meaningless to another and is never carried
//! across.

use cups_rs::{IppOperation, IppTag, IppValueTag};

use super::client::{MAX_COLLECTIONS, OperationCost, PaError, PaRequest, bounded, check_status};

/// A driver one Printer Application offers for a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaDriver {
    /// The driver name to pass back as `smi55357-driver`. Only meaningful to the
    /// application that reported it.
    pub(crate) id: String,
    pub(crate) display_name: String,
    /// The device ID pattern the driver matches, when reported.
    pub(crate) supported_device_id: Option<String>,
}

/// Whether a Printer Application can drive a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaDriverMatch {
    /// Support has not been checked.
    Unchecked,
    /// The application has at least one matching driver.
    Supported { drivers: Vec<PaDriver> },
    /// The application already has a printer for this device, so there is nothing
    /// left to set up. Named, so saying so does not need a second lookup.
    AlreadyConfigured { printer_name: String },
    /// The application reported no matching driver.
    Unsupported,
    /// The application wants credentials before it will answer.
    AuthenticationRequired,
    /// The application could not be asked.
    Unavailable,
    /// The application answered, but the response could not be read.
    MalformedResponse,
}

impl PaDriverMatch {
    /// Returns the driver name to send when creating a printer.
    ///
    /// A single matching driver is named explicitly, which is more precise than
    /// `auto` and does not depend on the application re-deriving the same answer.
    /// Several matches ask the application to choose, exactly as picking
    /// "Auto-Detect Driver" in its own web interface does — the application knows
    /// its drivers, and choosing one here would be a guess.
    pub(crate) fn driver_for_creation(&self) -> Option<&str> {
        match self {
            Self::Supported { drivers } => match drivers.as_slice() {
                [only] => Some(only.id.as_str()),
                _ => Some(AUTOMATIC_DRIVER),
            },
            _ => None,
        }
    }
}

impl PaDriverMatch {
    /// Whether this is the application's answer about the device rather than a
    /// failure to obtain one.
    ///
    /// Only an answer is worth remembering, and only a non-answer is worth asking
    /// again: a device the application has no driver for will say so every time,
    /// while a request that timed out says nothing about the device at all.
    pub(crate) fn is_an_answer(&self) -> bool {
        matches!(self, Self::Supported { .. } | Self::Unsupported)
    }
}

/// The value that asks a Printer Application to select the driver itself.
pub(crate) const AUTOMATIC_DRIVER: &str = "auto";

/// How many times to ask about a device before treating silence as the answer.
const ATTEMPTS: u32 = 3;

/// How long to wait before asking again.
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Asks a Printer Application which of its drivers match a device ID.
///
/// A device with no usable device ID cannot be matched: there is nothing to
/// match on, and reporting support anyway would be a guess.
///
/// Asked more than once if the application does not answer. A timed-out or
/// unreadable reply is not evidence about the device, and treating it as one is
/// what makes a printer appear to have a driver and then, after a refresh, not.
pub(crate) fn find_drivers(system_uri: &str, device_id: Option<&str>) -> PaDriverMatch {
    let Some(device_id) = device_id
        .map(str::trim)
        .filter(|device_id| !device_id.is_empty())
    else {
        return PaDriverMatch::Unsupported;
    };

    let mut matched = ask_for_drivers(system_uri, device_id);
    for attempt in 1..ATTEMPTS {
        if matched.is_an_answer() || matches!(matched, PaDriverMatch::AuthenticationRequired) {
            break;
        }

        tracing::debug!(
            attempt,
            ?matched,
            "asking a printer application about a device again"
        );
        std::thread::sleep(RETRY_DELAY);
        matched = ask_for_drivers(system_uri, device_id);
    }

    matched
}

fn ask_for_drivers(system_uri: &str, device_id: &str) -> PaDriverMatch {
    match request_drivers(system_uri, device_id) {
        Ok(drivers) => {
            // `auto` is not a driver, it is the request to pick one, so it is not
            // offered as a match of its own.
            let drivers = drivers
                .into_iter()
                .filter(|driver| !driver.id.eq_ignore_ascii_case(AUTOMATIC_DRIVER))
                .collect::<Vec<_>>();

            if drivers.is_empty() {
                PaDriverMatch::Unsupported
            } else {
                PaDriverMatch::Supported { drivers }
            }
        }
        Err(PaError::AuthenticationRequired) => PaDriverMatch::AuthenticationRequired,
        Err(PaError::OperationNotSupported) => PaDriverMatch::Unsupported,
        Err(PaError::Malformed { why }) => {
            tracing::warn!(why, "printer application returned an invalid driver list");
            PaDriverMatch::MalformedResponse
        }
        Err(error) => {
            tracing::debug!(?error, "could not check printer application driver support");
            PaDriverMatch::Unavailable
        }
    }
}

fn request_drivers(system_uri: &str, device_id: &str) -> Result<Vec<PaDriver>, PaError> {
    let response = PaRequest::new(IppOperation::PAPPL_FIND_DRIVERS, system_uri)?
        .string(
            IppTag::Operation,
            IppValueTag::Text,
            "smi55357-device-id",
            device_id,
        )?
        .send_allowing_failure(system_uri, OperationCost::Query)?;

    // An application with no driver for this device answers `not-found` rather
    // than an empty success, exactly as it does for a scan that found nothing.
    // That is an answer, not a failure to answer: reading it as one would report
    // a healthy application as unreachable for every printer it cannot drive.
    if response.status() == cups_rs::IppStatus::ErrorNotFound {
        return Ok(Vec::new());
    }
    check_status(&response)?;

    let mut drivers = Vec::new();

    // As with devices, one collection per driver, so every value of the repeated
    // attribute matters. Duplicates are dropped by driver name.
    for attribute in response.attributes_named("smi55357-driver-col") {
        if attribute.group_tag() != Some(IppTag::System)
            || attribute.value_tag() != IppValueTag::BeginCollection
        {
            continue;
        }

        for collection in attribute.collections().into_iter().take(MAX_COLLECTIONS) {
            let Some(id) = collection.text("smi55357-driver").map(bounded) else {
                continue;
            };
            if drivers.iter().any(|driver: &PaDriver| driver.id == id) {
                continue;
            }
            let display_name = collection
                .text("smi55357-driver-info")
                .map(bounded)
                .unwrap_or_else(|| id.clone());

            drivers.push(PaDriver {
                id,
                display_name,
                supported_device_id: collection.text("smi55357-device-id").map(bounded),
            });

            if drivers.len() >= MAX_COLLECTIONS {
                break;
            }
        }
    }

    Ok(drivers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver(id: &str) -> PaDriver {
        PaDriver {
            id: id.to_string(),
            display_name: id.to_string(),
            supported_device_id: None,
        }
    }

    #[test]
    fn a_single_driver_is_named_explicitly() {
        let matched = PaDriverMatch::Supported {
            drivers: vec![driver("acme-laser")],
        };

        assert_eq!(matched.driver_for_creation(), Some("acme-laser"));
    }

    #[test]
    fn several_drivers_let_the_application_choose() {
        let matched = PaDriverMatch::Supported {
            drivers: vec![driver("acme-laser"), driver("acme-laser-pcl")],
        };

        assert_eq!(matched.driver_for_creation(), Some(AUTOMATIC_DRIVER));
    }

    #[test]
    fn unresolved_states_never_name_a_driver() {
        for state in [
            PaDriverMatch::Unchecked,
            PaDriverMatch::Unsupported,
            PaDriverMatch::AuthenticationRequired,
            PaDriverMatch::Unavailable,
            PaDriverMatch::MalformedResponse,
        ] {
            assert_eq!(state.driver_for_creation(), None, "{state:?}");
        }
    }

    #[test]
    fn a_device_without_an_id_cannot_be_matched() {
        // No request is sent: with nothing to match on, claiming support would be
        // a guess.
        assert_eq!(
            find_drivers("ipp://localhost:1/ipp/system", None),
            PaDriverMatch::Unsupported
        );
        assert_eq!(
            find_drivers("ipp://localhost:1/ipp/system", Some("   ")),
            PaDriverMatch::Unsupported
        );
    }
}
