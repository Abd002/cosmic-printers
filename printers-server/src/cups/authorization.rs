//! Whether an administrative change could reach a printer, and whether this user could make one.

use cosmic_settings_printers_core::PrinterEntry;

use super::administration;

/// Marks each destination with whether an administrative change could reach it.
pub fn mark_administrable(printers: &mut [PrinterEntry]) {
    let user_may_administer = administration::user_may_administer();

    for printer in printers.iter_mut() {
        let administrable =
            administration::can_be_administered(printer, user_may_administer);

        printer.set_option("can-administer", administrable.to_string());
    }
}

/// Checks the group membership used by scheduler Unix-socket authorization.
pub fn may_administer_printers() -> bool {
    administration::user_may_administer()
}
