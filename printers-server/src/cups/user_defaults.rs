//! The user's own printing preferences, kept in their `lpoptions` file.
//! libcups applies this file before consulting a server, including for discovered printers without queues.

use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::Destinations;

use super::scheduler::split_queue_instance;
use crate::error::{BackendError, BackendResult};
use crate::ipp::CupsResultExt;

/// Overlays user options because libcups applies them after destination defaults.
pub(crate) fn apply_saved(printers: &mut [PrinterEntry]) {
    let Ok(saved) = Destinations::load_lpoptions() else {
        return;
    };
    let Ok(saved) = saved.to_vec() else {
        return;
    };

    // A user default overrides every server-reported default. Without one, libcups resolves the
    // system-wide or scheduler default, so leave that result intact.
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
pub(crate) async fn set_default(printer_id: &str) -> BackendResult<()> {
    let printer_id = printer_id.to_string();

    tokio::task::spawn_blocking(move || set_default_blocking(&printer_id))
        .await
        .map_err(BackendError::Join)?
}

fn set_default_blocking(printer_id: &str) -> BackendResult<()> {
    let (queue, instance) = split_queue_instance(printer_id);

    edit(|destinations| {
        // A destination with nothing saved for it yet has no entry to mark. `cupsAddDest`
        // adds the container for one; it does not create a queue.
        destinations.add_destination(queue, instance)?;
        destinations.set_default_destination(queue, instance)
    })
}

/// Leaves no destination marked as the user's default.
pub(crate) async fn clear_default() -> BackendResult<()> {
    tokio::task::spawn_blocking(clear_default_blocking)
        .await
        .map_err(BackendError::Join)?
}

fn clear_default_blocking() -> BackendResult<()> {
    edit(|destinations| {
        destinations.clear_default_destination();
        Ok(())
    })
}

/// Records the user's choice for one option on one destination.
pub(crate) async fn set_option_default(
    printer_id: &str,
    option: &str,
    values: &[String],
) -> BackendResult<()> {
    let printer_id = printer_id.to_string();
    let option = option.to_string();
    let values = values.to_vec();

    tokio::task::spawn_blocking(move || set_option_default_blocking(&printer_id, &option, &values))
        .await
        .map_err(BackendError::Join)?
}

fn set_option_default_blocking(
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
fn edit(change: impl FnOnce(&mut Destinations) -> cups_rs::Result<()>) -> BackendResult<()> {
    let mut destinations = Destinations::load_lpoptions().cups_err()?;

    change(&mut destinations).cups_err()?;
    destinations.save_to_lpoptions().cups_err()?;

    mirror_to_the_legacy_file();
    Ok(())
}

/// Copies what was just saved to the path libcups 2 reads.
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
