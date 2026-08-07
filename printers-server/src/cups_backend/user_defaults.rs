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

    // A default the user chose settles it for every destination, not only for the one it
    // names: leaving the others as the server reported them would show two defaults, and
    // the server's is the one that has been overruled.
    //
    // With nothing named in the user's file the question passes to libcups, which knows the
    // rest of the order — a system-wide default, then the scheduler's own. Deciding here that
    // there is no default would be wrong twice: it contradicts where a job would actually go,
    // and it hides a default an administrator set for the machine.
    let chosen_default = saved
        .iter()
        .find(|entry| entry.is_default)
        .map(|entry| entry.full_name())
        .or_else(Destinations::default_destination_name);

    for printer in printers {
        match &chosen_default {
            Some(chosen) => printer.set_is_default(printer.id() == chosen),
            None => printer.set_is_default(false),
        }

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
    destinations.save_to_lpoptions().cups_err()?;

    mirror_to_the_legacy_file();
    Ok(())
}

/// Copies what was just saved to the path libcups 2 reads.
///
/// The two libraries do not agree on where a user's saved destinations live: this one honours
/// `XDG_CONFIG_HOME` and so may use `~/.config/cups/lpoptions`, while libcups 2 only ever reads
/// `~/.cups/lpoptions`. Which file gets written therefore depends on the environment this
/// process happened to inherit — and on a desktop most print dialogs, `lp` and `lpstat` are
/// still the other library. A default printer that only half the system honours is not a
/// default, so both are kept in step.
///
/// Failure is not worth failing the operation over: the preference *was* saved where this
/// library will read it, and printing from here will honour it either way.
fn mirror_to_the_legacy_file() {
    let (Ok(written), Some(legacy)) = (
        cups_rs::user_lpoptions_path(),
        cups_rs::legacy_lpoptions_path(),
    ) else {
        return;
    };

    if written == legacy {
        return;
    }

    let copy = std::fs::read(&written).and_then(|contents| {
        if let Some(parent) = std::path::Path::new(&legacy).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&legacy, contents)
    });

    if let Err(error) = copy {
        tracing::debug!(
            from = written,
            to = legacy,
            error = ?error,
            "could not mirror the saved preferences to the path libcups 2 reads"
        );
    }
}
