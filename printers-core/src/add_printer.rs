//! Wire types for the Add Printer flow.
//!
//! Add Printer exists for legacy, non-driverless, and specialty printers that
//! need a Printer Application to drive them. Driverless IPP printers, remote
//! CUPS queues, and printers a Printer Application has already created arrive
//! through the ordinary destination pipeline instead, and never appear here.
//!
//! These types describe *candidates for configuration*, not destinations. A
//! physical printer row is not a destination group, and a Printer Application
//! candidate is not a queue. Nothing here creates a [`crate::PrinterEntry`]:
//! configuration asks a Printer Application to create a printer, and the
//! destination pipeline discovers the result on its own.
//!
//! Device URIs are deliberately absent from every type in this module. A device
//! URI is opaque to everyone except the Printer Application that produced it, so
//! the server resolves a candidate identifier back to the exact URI it recorded
//! rather than accepting one from a client.

use serde::{Deserialize, Serialize};

/// Identifies one round of Add Printer discovery.
///
/// A new generation starts whenever discovery is started or refreshed. Results
/// from an older generation are no longer selectable, because the devices they
/// describe may be gone.
pub type DiscoveryGeneration = u64;

/// How far along a round of discovery is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum AddPrinterDiscoveryState {
    /// No discovery has been started.
    Idle,
    /// At least one Printer Application is still being asked.
    Searching,
    /// Every Printer Application answered.
    Complete,
    /// Every Printer Application finished, but some failed. Results from the
    /// ones that answered are still usable.
    CompleteWithErrors,
}

/// How one Printer Application's scan ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrinterApplicationScanState {
    /// Queued, not yet started.
    Pending,
    /// Currently being asked for devices.
    Searching,
    /// Answered successfully.
    Complete,
    /// Refused the request until credentials are supplied.
    AuthenticationRequired,
    /// Could not be reached.
    Unreachable,
    /// Does not implement device discovery.
    Unsupported,
    /// Answered, but the response could not be used.
    Failed,
}

/// Whether a Printer Application can configure a particular physical printer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PaCandidateState {
    /// Has at least one matching driver and can create the printer directly.
    Ready,
    /// Already has a printer for this device. There is nothing left to set up, and
    /// setting it up again would produce a second queue for one printer.
    AlreadyConfigured,
    /// Sees the device but has no driver for it.
    Unsupported,
    /// Driver support could not be established.
    DriverUnknown,
    /// Would need credentials before it could be asked.
    AuthenticationRequired,
    /// Was reachable during discovery but is not usable now.
    Unavailable,
}

/// One Printer Application that could configure a physical printer.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterApplicationCandidateSummary {
    /// Opaque identifier to send back when configuring through this Printer
    /// Application.
    pub id: String,
    /// Identifier of the owning Printer Application.
    pub printer_application_id: String,
    /// Name to show for the owning Printer Application.
    pub printer_application_name: String,
    /// Whether this Printer Application can actually drive the device.
    pub state: PaCandidateState,
}

/// One physical printer, as one Add Printer row.
///
/// Observations from several Printer Applications that describe the same
/// hardware are collapsed into a single row listing each Printer Application
/// that can configure it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct DiscoveredPhysicalPrinter {
    /// Opaque identifier, stable for as long as the generation lasts.
    pub id: String,
    pub display_name: String,
    pub make_and_model: Option<String>,
    /// The Printer Applications that reported this printer.
    pub candidates: Vec<PrinterApplicationCandidateSummary>,
    /// How confidently the observations were judged to be one printer.
    pub identity_confidence: IdentityConfidenceKind,
}

/// How confidently a physical printer's identity was established.
///
/// The wire form of [`crate::IdentityConfidence`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum IdentityConfidenceKind {
    Strong,
    Medium,
    Weak,
}

impl From<crate::IdentityConfidence> for IdentityConfidenceKind {
    fn from(confidence: crate::IdentityConfidence) -> Self {
        match confidence {
            crate::IdentityConfidence::Strong => Self::Strong,
            crate::IdentityConfidence::Medium => Self::Medium,
            crate::IdentityConfidence::Weak => Self::Weak,
        }
    }
}

/// Diagnostic status of one Printer Application's scan.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrinterApplicationScanStatus {
    pub printer_application_id: String,
    pub printer_application_name: String,
    pub state: PrinterApplicationScanState,
}

/// Reply to starting a round of discovery.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct StartAddPrinterDiscoveryReply {
    /// The generation that was started. Configuration requests must quote it.
    pub generation: DiscoveryGeneration,
}

/// The current state of Add Printer discovery.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct AddPrinterDiscoveryReply {
    pub generation: DiscoveryGeneration,
    pub state: AddPrinterDiscoveryState,
    /// One row per physical printer.
    pub physical_printers: Vec<DiscoveredPhysicalPrinter>,
    pub completed_printer_application_scans: u32,
    pub total_printer_application_scans: u32,
    /// Whether any Printer Application failed to answer usefully.
    pub any_printer_application_failed: bool,
    /// Per-application status, for diagnosing a partial result.
    pub printer_application_scans: Vec<PrinterApplicationScanStatus>,
    /// True when these rows are left over from an earlier generation and are
    /// shown for context only. Cached rows cannot be configured.
    pub cached: bool,
}

/// Asks the server to configure a discovered printer.
///
/// The server resolves the candidate to the owning Printer Application, its
/// validated driver match, and the exact device URI that Printer Application
/// reported. None of those can be supplied by the caller.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ConfigureDiscoveredPrinterRequest {
    /// The generation the selection was made in. A stale generation is
    /// rejected rather than acted on.
    pub discovery_generation: DiscoveryGeneration,
    pub physical_printer_id: String,
    pub candidate_id: String,
    /// Optional display name for the new printer. The queue name is derived by
    /// the server; this only affects the human-readable description.
    pub requested_display_name: Option<String>,
}

/// How a configuration attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub enum PrinterConfigurationState {
    /// The Printer Application is being asked to create the printer.
    Creating,
    /// The printer was created and the destination pipeline has not advertised
    /// it yet.
    AwaitingAdvertisement,
    /// The created printer has been matched to a destination.
    Reconciled,
    /// This device already had a printer in this Printer Application.
    AlreadyConfigured,
    /// Setup has to continue in the Printer Application's own interface.
    ManualActionRequired,
    /// The request was sent but the outcome could not be established.
    UnknownOutcome,
    /// The Printer Application rejected the request.
    Failed,
}

/// The result of a configuration attempt.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ConfigurePrinterReply {
    /// Identifier for polling this attempt.
    pub operation_id: String,
    pub state: PrinterConfigurationState,
    /// The queue name the Printer Application was asked to create.
    pub configured_printer_name: String,
    /// The destination this attempt reconciled to, once one appeared.
    pub destination_id: Option<String>,
    /// Where to continue setup, when the Printer Application offers a page for
    /// it.
    pub web_interface_uri: Option<String>,
}

/// A Printer Application offering manual setup through its own interface.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ManualSetupPrinterApplication {
    pub printer_application_id: String,
    pub display_name: String,
    /// An `http` or `https` page. Never the IPP System Service URI.
    pub web_interface_uri: String,
    pub state: crate::PrinterApplicationState,
}

/// Reply listing Printer Applications that can be set up by hand.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListManualSetupApplicationsReply {
    pub printer_applications: Vec<ManualSetupPrinterApplication>,
}
