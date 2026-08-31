use cosmic_settings_printers_core::Error;

pub(crate) type BackendResult<T> = Result<T, BackendError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BackendError {
    #[error("CUPS operation failed: {0}")]
    Cups(#[source] cups_rs::Error),

    #[error("failed to enumerate CUPS printers: {0}")]
    FailedToGetPrinters(#[source] cups_rs::Error),

    #[error("blocking task failed: {0}")]
    Join(#[source] tokio::task::JoinError),

    #[error("queue '{queue}' has no device URI")]
    MissingDeviceUri { queue: String },

    #[error("permission denied for '{operation}'")]
    PermissionDenied { operation: String },

    #[error("print job {job_id} was not found")]
    JobNotFound { job_id: i32 },

    #[error("print job {job_id} can no longer be moved")]
    JobNotMovable { job_id: i32 },

    #[error("'{operation}' is not supported by the print scheduler")]
    OperationNotSupported { operation: String },

    #[error("invalid print-job destination: {why}")]
    InvalidMoveDestination { why: String },

    #[error("device '{uri}' is unreachable: {source}")]
    DeviceUnreachable {
        uri: String,
        #[source]
        source: cups_rs::Error,
    },

    #[error("{operation} failed with IPP status {status}")]
    IppStatus { operation: String, status: String },

    #[error("no service holds a queue for '{printer}' to administer")]
    NoQueueToAdminister { printer: String },

    #[error("{0}")]
    Internal(String),
}

impl From<cups_rs::Error> for BackendError {
    fn from(error: cups_rs::Error) -> Self {
        Self::Cups(error)
    }
}

impl From<tokio::task::JoinError> for BackendError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::Join(error)
    }
}

impl From<BackendError> for Error {
    fn from(error: BackendError) -> Self {
        match error {
            BackendError::FailedToGetPrinters(source) => Self::FailedToGetPrinters {
                why: source.to_string(),
            },
            BackendError::MissingDeviceUri { queue } => Self::MissingDeviceUri { queue },
            BackendError::PermissionDenied { operation } => Self::PermissionDenied { operation },
            BackendError::JobNotFound { job_id } => Self::JobNotFound { job_id },
            BackendError::JobNotMovable { job_id } => Self::JobNotMovable { job_id },
            BackendError::OperationNotSupported { operation } => {
                Self::OperationNotSupported { operation }
            }
            BackendError::InvalidMoveDestination { why } => Self::InvalidMoveDestination { why },
            BackendError::DeviceUnreachable { uri, source } => Self::DeviceUnreachable {
                why: format!("{uri}: {source}"),
            },
            BackendError::Join(source) => Self::Internal {
                why: source.to_string(),
            },
            BackendError::Internal(why) => Self::Internal { why },
            BackendError::Cups(source) => Self::CupsFailed {
                why: source.to_string(),
            },
            BackendError::IppStatus { operation, status } => Self::CupsFailed {
                why: format!("{operation} failed with status {status}"),
            },
            // Not a failure of the request so much as of the premise: there is nothing
            // holding this destination for an administrative change to reach.
            BackendError::NoQueueToAdminister { printer } => Self::OperationNotSupported {
                operation: format!("administer '{printer}', which no service holds a queue for"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn cups_errors_remain_sources_until_wire_conversion() {
        let backend = BackendError::Cups(cups_rs::Error::Timeout);
        assert!(backend.source().is_some());

        let wire = Error::from(backend);
        assert!(matches!(wire, Error::CupsFailed { .. }));
    }

    #[test]
    fn enumeration_errors_keep_their_public_category() {
        let wire = Error::from(BackendError::FailedToGetPrinters(
            cups_rs::Error::DestinationListFailed,
        ));

        assert!(matches!(wire, Error::FailedToGetPrinters { .. }));
    }
}
