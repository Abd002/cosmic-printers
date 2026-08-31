//! Shared printer UI for COSMIC Settings and the standalone application.

#![warn(missing_docs)]

pub mod backend;
// Keep this before modules that use `fl!` or `slab!`.
#[macro_use]
mod localize;

/// Printer discovery and setup UI.
pub mod add_printer;
/// Printer details and settings UI.
pub mod details;
/// Printer list UI.
pub mod list;
/// Print queue UI.
pub mod queue;

mod icons;
pub mod state_reason;
mod style;
mod widgets;

/// Localized strings required by UI hosts.
pub mod strings {
    /// Returns the Printers page title.
    #[must_use]
    pub fn printers() -> String {
        fl!("printers")
    }

    /// Returns the printer details page title.
    #[must_use]
    pub fn printer_details() -> String {
        fl!("printer-details")
    }

    /// Returns the printer details page description.
    #[must_use]
    pub fn printer_details_description() -> String {
        fl!("printer-details-description")
    }

    /// Returns the print queue title.
    #[must_use]
    pub fn printer_queue() -> String {
        fl!("printer-queue")
    }

    /// Returns the print queue description.
    #[must_use]
    pub fn printer_queue_description() -> String {
        fl!("printer-queue-description")
    }

    /// Returns the default printer section title.
    #[must_use]
    pub fn default_printer() -> String {
        fl!("default-printer")
    }

    /// Returns the printer information section title.
    #[must_use]
    pub fn printer_information() -> String {
        fl!("printer-information")
    }

    /// Returns the printing preferences section title.
    #[must_use]
    pub fn printing_preferences() -> String {
        fl!("printing-preferences")
    }

    /// Returns the supplies section title.
    #[must_use]
    pub fn supplies() -> String {
        fl!("supplies")
    }

    /// Returns the remove printer section title.
    #[must_use]
    pub fn remove_printer() -> String {
        fl!("remove-printer")
    }

    /// Returns the default printer row label.
    #[must_use]
    pub fn set_as_default_printer() -> String {
        fl!("set-as-default-printer")
    }

    /// The searchable labels of the rows in the printer-information section.
    #[must_use]
    pub fn printer_information_rows() -> [String; 4] {
        [
            fl!("location"),
            fl!("model"),
            fl!("device-name"),
            fl!("driver-version"),
        ]
    }

    /// The searchable labels of the rows in the preferences section.
    #[must_use]
    pub fn printing_preferences_rows() -> [String; 2] {
        [fl!("paper-size"), fl!("print-sides")]
    }
}

pub use backend::{Backend, BackendError, EventFeed};
pub use details::Request;
pub use localize::{localizer, select_languages};
