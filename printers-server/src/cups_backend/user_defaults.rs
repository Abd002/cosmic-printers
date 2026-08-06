//! The user's own printing preferences, kept in their `lpoptions` file.
//!
//! These are not administration and need no privilege. libcups reads this file before it
//! asks any server, so a preference here takes effect over whatever the scheduler thinks
//! and can name any destination libcups can enumerate — including one it knows only from
//! DNS-SD, which has no queue for a server to hold an opinion about. That is what makes
//! a default work for a discovered printer, where `CUPS-Set-Default` cannot: it needs a
//! `/printers/<name>` resource to point at, and there is none.

use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::Destinations;

use super::helpers::{CupsResultExt, split_queue_instance};
use crate::error::BackendResult;

/// Lays the user's saved options over what each destination reported for itself.
///
/// libcups applies these last when printing, so they are what a job will actually use —
/// which means they have to be what the page shows too. Without this a paper size the user
/// chose would be saved, take effect, and still be displayed as whatever the queue says.
pub(super) fn apply_saved(printers: &mut [PrinterEntry]) {
    let Ok(saved) = Destinations::load_lpoptions() else {
        return;
    };
    let Ok(saved) = saved.to_vec() else {
        return;
    };

    for printer in printers {
        let (queue, instance) = split_queue_instance(printer.id());
        let Some(entry) = saved
            .iter()
            .find(|entry| entry.name == queue && entry.instance.as_deref() == instance)
        else {
            continue;
        };

        for (option, value) in &entry.options {
            printer.set_option(option, value);
        }
    }
}

/// Records the user's default destination.
pub(super) fn set_default(printer_id: &str) -> BackendResult<()> {
    let (queue, instance) = split_queue_instance(printer_id);

    edit(|destinations| {
        // A destination with nothing saved for it yet has no entry to mark. `cupsAddDest`
        // adds the container for one; it does not create a queue.
        destinations.add_destination(queue, instance)?;
        destinations.set_default_destination(queue, instance)
    })
}

/// Leaves no destination marked as the user's default.
///
/// The saved options stay: the user chose those separately from choosing a default.
pub(super) fn clear_default() -> BackendResult<()> {
    edit(|destinations| {
        destinations.clear_default_destination();
        Ok(())
    })
}

/// Records the user's choice for one option on one destination.
pub(super) fn set_option_default(
    printer_id: &str,
    option: &str,
    values: &[String],
) -> BackendResult<()> {
    let (queue, instance) = split_queue_instance(printer_id);
    // An `lpoptions` line holds one `name=value` per option, which is how libcups spells
    // a multiple-valued one too.
    let value = values.join(",");

    edit(|destinations| destinations.set_destination_option(queue, instance, option, &value))
}

/// Applies one edit to the user's saved destinations.
///
/// `cupsSetDests` rewrites the file from the list it is handed, so the list has to be the
/// whole of what the file said: read all of it, change the one thing, write all of it
/// back.
///
/// Read from the file, never from `cupsGetDests`. That answers with the destinations that
/// exist right now, so saving from it would drop every entry naming a printer that is
/// switched off, and replace the user's own option values with whichever queue happens to
/// answer.
fn edit(change: impl FnOnce(&mut Destinations) -> cups_rs::Result<()>) -> BackendResult<()> {
    let mut destinations = Destinations::load_lpoptions().cups_err()?;

    change(&mut destinations).cups_err()?;
    destinations.save_to_lpoptions().cups_err()
}
