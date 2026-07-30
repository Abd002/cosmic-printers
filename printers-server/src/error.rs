use cosmic_settings_printers_core::Error;

pub(crate) type BackendResult<T> = Result<T, BackendError>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum BackendError {
    #[error("CUPS operation failed: {0}")]
    Cups(#[source] cups_rs::Error),

    #[error("failed to enumerate CUPS printers: {0}")]
    FailedToGetPrinters(#[source] cups_rs::Error),

    #[error("printer configuration failed: {0}")]
    Config(#[source] cosmic_config::Error),

    #[error("{operation} failed over D-Bus: {source}")]
    Zbus {
        operation: &'static str,
        #[source]
        source: zbus::Error,
    },

    #[error("blocking task failed: {0}")]
    Join(#[source] tokio::task::JoinError),

    #[error("queue '{queue}' has no device URI")]
    MissingDeviceUri { queue: String },

    #[error("permission denied for '{operation}'")]
    PermissionDenied { operation: String },

    #[error("device '{uri}' is unreachable: {source}")]
    DeviceUnreachable {
        uri: String,
        #[source]
        source: cups_rs::Error,
    },

    #[error("{operation} failed with IPP status {status}")]
    IppStatus { operation: String, status: String },

    #[error("{operation} was rejected: {why}")]
    HelperRejected {
        operation: &'static str,
        why: String,
    },

    #[error("{0}")]
    Internal(String),
}

impl BackendError {
    pub(crate) fn zbus(operation: &'static str, source: zbus::Error) -> Self {
        Self::Zbus { operation, source }
    }
}

impl From<cups_rs::Error> for BackendError {
    fn from(error: cups_rs::Error) -> Self {
        Self::Cups(error)
    }
}

impl From<cosmic_config::Error> for BackendError {
    fn from(error: cosmic_config::Error) -> Self {
        Self::Config(error)
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
            BackendError::Config(source) => Self::ConfigFailed {
                why: source.to_string(),
            },
            BackendError::MissingDeviceUri { queue } => Self::MissingDeviceUri { queue },
            BackendError::PermissionDenied { operation } => Self::PermissionDenied { operation },
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
            BackendError::Zbus { operation, source } => Self::CupsFailed {
                why: format!("{operation}: {source}"),
            },
            BackendError::IppStatus { operation, status } => Self::CupsFailed {
                why: format!("{operation} failed with status {status}"),
            },
            BackendError::HelperRejected { operation, why } => Self::CupsFailed {
                why: format!("{operation}: {why}"),
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
