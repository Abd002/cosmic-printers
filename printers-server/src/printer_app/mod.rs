//! Setting up a printer through the Printer Application that can drive it.

mod client;
mod configure;
mod devices;
mod drivers;
mod errors;
mod identity;
mod printers;
mod probe;
pub(crate) mod reconcile;
mod round;
mod scan;
mod web;

pub(crate) use configure::configure_discovered_printer;
pub(crate) use drivers::PaDriverMatch;
pub(crate) use identity::PaConfigurationCandidate;
pub(crate) use printers::{ConfiguredPrinter, owned_printers};
pub(crate) use probe::record_discovery;
pub(crate) use reconcile::{OwnedPrinter, PendingConfigurationState, PendingPaConfiguration};
pub(crate) use round::{AddPrinterDiscovery, DiscoveryGeneration, ResolveError};
pub(crate) use scan::start_add_printer_discovery;
pub(crate) use web::manual_setup_applications;
