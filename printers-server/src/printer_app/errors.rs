//! Turning what went wrong with a Printer Application into what a client can act on.

use cosmic_settings_printers_core::{Error, PrinterApplication};

use super::client::PaError;
use super::round::ResolveError;

pub(super) fn resolve_error(
    error: ResolveError,
    request: &cosmic_settings_printers_core::ConfigureDiscoveredPrinterRequest,
) -> Error {
    match error {
        ResolveError::NotStarted => Error::AddPrinterDiscoveryNotStarted,
        ResolveError::Expired { generation } => Error::AddPrinterDiscoveryExpired { generation },
        ResolveError::PrinterNotFound => Error::DiscoveredPhysicalPrinterNotFound {
            printer_id: request.physical_printer_id.clone(),
        },
        ResolveError::CandidateNotFound => Error::PrinterApplicationCandidateNotFound {
            candidate_id: request.candidate_id.clone(),
        },
    }
}

pub(super) fn configuration_error(
    application: &PrinterApplication,
    error: PaError,
    web_interface_uri: Option<String>,
) -> Error {
    let application_id = application.id.clone();

    match error {
        PaError::AuthenticationRequired => {
            Error::PrinterApplicationAuthenticationRequired { application_id }
        }
        PaError::Forbidden { why } => Error::PrinterConfigurationManualActionRequired {
            application_id,
            web_interface_uri,
            why,
        },
        PaError::Unreachable { .. } => Error::PrinterApplicationUnavailable { application_id },
        PaError::OperationNotSupported => Error::PrinterApplicationOperationNotSupported {
            application_id,
            operation: "Create-Printer".to_string(),
        },
        PaError::Rejected { status, why } => Error::PrinterConfigurationRejected {
            application_id,
            status,
            why,
        },
        PaError::Malformed { why } => Error::MalformedPrinterApplicationResponse {
            application_id,
            operation: "Create-Printer".to_string(),
            why,
        },
    }
}
