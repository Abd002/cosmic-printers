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

    /// A discovered network/IPP device couldn't be reached directly.
    DeviceUnreachable { why: String },

    /// Reading or writing queue metadata via cosmic-config failed.
    ConfigFailed { why: String },

    /// A blocking CUPS task panicked or was cancelled.
    Internal { why: String },

    /// Catch-all for IPP/CUPS failures that don't fit a category above.
    CupsFailed { why: String },
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
            Error::DeviceUnreachable { why } => write!(f, "device unreachable: {why}"),
            Error::ConfigFailed { why } => write!(f, "printer config error: {why}"),
            Error::Internal { why } => write!(f, "internal error: {why}"),
            Error::CupsFailed { why } => write!(f, "CUPS error: {why}"),
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
