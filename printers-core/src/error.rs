use zlink::{ReplyError, introspect};

#[derive(Debug, PartialEq, ReplyError, introspect::ReplyError)]
#[zlink(interface = "com.system76.CosmicSettings.Printers")]
pub enum Error {
    /// `printer_id` doesn't match any queue in the current snapshot.
    PrinterNotFound,

    /// No queue is currently marked as the system default.
    NoDefaultPrinter,

    /// Could not enumerate queues from the CUPS scheduler at all.
    FailedToGetPrinters { why: String },

    /// A destination has no device URI to act on.
    MissingDeviceUri { queue: String },

    /// CUPS rejected the request because the caller isn't authenticated or authorized.
    PermissionDenied { operation: String },

    /// The requested print job no longer exists.
    JobNotFound { job_id: i32 },

    /// The print job is in a final state and cannot be moved.
    JobNotMovable { job_id: i32 },

    /// The connected scheduler does not implement the requested operation.
    OperationNotSupported { operation: String },

    /// The selected destination cannot be used for this move.
    InvalidMoveDestination { why: String },

    /// A discovered network/IPP device couldn't be reached directly.
    DeviceUnreachable { why: String },

    /// A blocking CUPS task panicked or was cancelled.
    Internal { why: String },

    /// Catch-all for IPP/CUPS failures that don't fit a category above.
    CupsFailed { why: String },

    /// Add Printer results were requested before discovery was started.
    AddPrinterDiscoveryNotStarted,

    /// The request quoted a discovery generation that is no longer current, so
    /// the devices it described may be gone. Start discovery again.
    AddPrinterDiscoveryExpired { generation: u64 },

    /// No physical printer with this id exists in the current generation.
    DiscoveredPhysicalPrinterNotFound { printer_id: String },

    /// No Printer Application candidate with this id belongs to that printer.
    PrinterApplicationCandidateNotFound { candidate_id: String },

    /// The Printer Application is no longer being advertised.
    PrinterApplicationNotFound { application_id: String },

    /// The Printer Application is known but could not be reached.
    PrinterApplicationUnavailable { application_id: String },

    /// The Printer Application requires credentials. Setup continues in its own
    /// web interface; this service never collects or forwards a password.
    PrinterApplicationAuthenticationRequired { application_id: String },

    /// The Printer Application does not implement an operation the flow needs.
    PrinterApplicationOperationNotSupported {
        application_id: String,
        operation: String,
    },

    /// This device already has a printer in that Printer Application.
    DiscoveredPrinterAlreadyConfigured {
        application_id: String,
        printer_name: String,
    },

    /// The Printer Application refused to create the printer.
    PrinterConfigurationRejected {
        application_id: String,
        status: String,
        why: String,
    },

    /// The request was sent but the outcome could not be established, and it was
    /// not retried because that could create a second printer.
    PrinterConfigurationUnknownOutcome {
        application_id: String,
        printer_name: String,
    },

    /// The printer cannot be created automatically; setup has to continue in the
    /// Printer Application's own interface.
    PrinterConfigurationManualActionRequired {
        application_id: String,
        web_interface_uri: Option<String>,
        why: String,
    },

    /// A Printer Application's response did not follow the protocol.
    MalformedPrinterApplicationResponse {
        application_id: String,
        operation: String,
        why: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::PrinterNotFound => f.write_str("printer not found"),
            Error::NoDefaultPrinter => f.write_str("no default printer is set"),
            Error::FailedToGetPrinters { why } => write!(f, "failed to list printers: {why}"),
            Error::MissingDeviceUri { queue } => write!(f, "queue '{queue}' has no device URI"),
            Error::PermissionDenied { operation } => {
                write!(f, "permission denied for '{operation}'")
            }
            Error::JobNotFound { job_id } => write!(f, "print job {job_id} was not found"),
            Error::JobNotMovable { job_id } => {
                write!(f, "print job {job_id} can no longer be moved")
            }
            Error::OperationNotSupported { operation } => {
                write!(f, "'{operation}' is not supported by the print scheduler")
            }
            Error::InvalidMoveDestination { why } => {
                write!(f, "invalid print-job destination: {why}")
            }
            Error::DeviceUnreachable { why } => write!(f, "device unreachable: {why}"),
            Error::Internal { why } => write!(f, "internal error: {why}"),
            Error::CupsFailed { why } => write!(f, "CUPS error: {why}"),
            Error::AddPrinterDiscoveryNotStarted => {
                f.write_str("printer discovery has not been started")
            }
            Error::AddPrinterDiscoveryExpired { generation } => write!(
                f,
                "printer discovery results from generation {generation} are no longer current"
            ),
            Error::DiscoveredPhysicalPrinterNotFound { printer_id } => {
                write!(f, "discovered printer '{printer_id}' was not found")
            }
            Error::PrinterApplicationCandidateNotFound { candidate_id } => {
                write!(
                    f,
                    "printer application candidate '{candidate_id}' was not found"
                )
            }
            Error::PrinterApplicationNotFound { application_id } => {
                write!(f, "printer application '{application_id}' was not found")
            }
            Error::PrinterApplicationUnavailable { application_id } => {
                write!(f, "printer application '{application_id}' is unavailable")
            }
            Error::PrinterApplicationAuthenticationRequired { application_id } => write!(
                f,
                "printer application '{application_id}' requires authentication"
            ),
            Error::PrinterApplicationOperationNotSupported {
                application_id,
                operation,
            } => write!(
                f,
                "printer application '{application_id}' does not support '{operation}'"
            ),
            Error::DiscoveredPrinterAlreadyConfigured {
                application_id,
                printer_name,
            } => write!(
                f,
                "printer application '{application_id}' already has a printer '{printer_name}' for this device"
            ),
            Error::PrinterConfigurationRejected {
                application_id,
                status,
                why,
            } => write!(
                f,
                "printer application '{application_id}' rejected the printer ({status}): {why}"
            ),
            Error::PrinterConfigurationUnknownOutcome {
                application_id,
                printer_name,
            } => write!(
                f,
                "printer application '{application_id}' did not confirm whether '{printer_name}' was created"
            ),
            Error::PrinterConfigurationManualActionRequired {
                application_id,
                why,
                ..
            } => write!(
                f,
                "printer application '{application_id}' needs manual setup: {why}"
            ),
            Error::MalformedPrinterApplicationResponse {
                application_id,
                operation,
                why,
            } => write!(
                f,
                "printer application '{application_id}' returned an invalid '{operation}' response: {why}"
            ),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::Error;
    use serde::Deserialize;

    #[test]
    fn payload_reply_errors_round_trip() {
        let original = Error::CupsFailed {
            why: "CUPS-Create-Local-Printer failed with status ErrorInternalError".to_string(),
        };

        let json = serde_json::to_string(&original).unwrap();

        assert_eq!(
            json,
            r#"{"error":"com.system76.CosmicSettings.Printers.CupsFailed","parameters":{"why":"CUPS-Create-Local-Printer failed with status ErrorInternalError"}}"#
        );

        let decoded: Error = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn payload_reply_errors_decode_through_untagged_reply_envelope() {
        #[derive(Debug, Deserialize)]
        #[serde(untagged)]
        enum ReplyMsg {
            Error(Error),
            Reply(zlink::reply::Reply<()>),
        }

        let json = r#"{"error":"com.system76.CosmicSettings.Printers.CupsFailed","parameters":{"why":"boom"}}"#;
        let decoded: ReplyMsg = serde_json::from_str(json).unwrap();

        match decoded {
            ReplyMsg::Error(Error::CupsFailed { why }) => assert_eq!(why, "boom"),
            other => panic!("unexpected decoded reply: {other:?}"),
        }

        let reply_json = r#"{}"#;
        let decoded: ReplyMsg = serde_json::from_str(reply_json).unwrap();
        match decoded {
            ReplyMsg::Reply(reply) => assert!(reply.parameters().is_none()),
            other => panic!("unexpected decoded reply: {other:?}"),
        }
    }
}
