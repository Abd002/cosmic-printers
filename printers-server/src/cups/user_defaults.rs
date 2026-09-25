//! The user's own printing preferences, kept in their `lpoptions` file.
//! libcups applies this file before consulting a server, including for discovered printers without queues.

use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::Destinations;

use super::conversion::is_discovered;
use super::scheduler::split_queue_instance;
use crate::error::{BackendError, BackendResult};
use crate::ipp::CupsResultExt;

/// Overlays user options because libcups applies them after destination defaults.
pub(crate) fn apply_saved(printers: &mut [PrinterEntry]) {
    // A printer without a queue is looked up by enumerating the network, and libcups can
    let saved = printers
        .iter()
        .map(|printer| {
            if is_discovered(printer) {
                return None;
            }
            let (queue, instance) = split_queue_instance(printer.id());
            Destinations::named_destination(queue, instance)
        })
        .collect::<Vec<_>>();

        let chosen_default = saved
        .iter()
        .flatten()
        .find(|dest| dest.is_default)
        .map(|dest| dest.full_name())
        .or_else(Destinations::default_destination_name)
        // libcups finds no default that names a printer without a queue.
        .or_else(|| {
            printers
                .iter()
                .find(|printer| is_discovered(printer) && printer.is_default())
                .map(|printer| printer.id().to_string())
        });

    for (printer, saved) in printers.iter_mut().zip(saved) {
        match &chosen_default {
            Some(chosen) => printer.set_is_default(printer.id() == chosen),
            None => printer.set_is_default(false),
        }

        let Some(saved) = saved else {
            continue;
        };

        for (option, value) in &saved.options {
            printer.set_option(option, value);
        }
    }
}

/// Records the user's default destination.
pub(crate) async fn set_default(
    printer_id: &str,
    known_printers: &[PrinterEntry],
) -> BackendResult<()> {
    let printer_id = printer_id.to_string();
    let known_printers = known_printers.to_vec();

    tokio::task::spawn_blocking(move || set_default_blocking(&printer_id, &known_printers))
        .await
        .map_err(BackendError::Join)?
}

fn set_default_blocking(printer_id: &str, known_printers: &[PrinterEntry]) -> BackendResult<()> {
    let (queue, instance) = split_queue_instance(printer_id);

    edit(known_printers, |destinations| {
        // A destination with nothing saved for it yet has no entry to mark. `cupsAddDest`
        // adds the container for one; it does not create a queue.
        destinations.add_destination(queue, instance)?;
        destinations.set_default_destination(queue, instance)
    })
}

/// Leaves no destination marked as the user's default.
pub(crate) async fn clear_default(known_printers: &[PrinterEntry]) -> BackendResult<()> {
    let known_printers = known_printers.to_vec();

    tokio::task::spawn_blocking(move || clear_default_blocking(&known_printers))
        .await
        .map_err(BackendError::Join)?
}

fn clear_default_blocking(known_printers: &[PrinterEntry]) -> BackendResult<()> {
    edit(known_printers, |destinations| {
        destinations.clear_default_destination();
        Ok(())
    })
}

/// Records the user's choice for one option on one destination.
pub(crate) async fn set_option_default(
    printer_id: &str,
    option: &str,
    values: &[String],
    known_printers: &[PrinterEntry],
) -> BackendResult<()> {
    let printer_id = printer_id.to_string();
    let option = option.to_string();
    let values = values.to_vec();
    let known_printers = known_printers.to_vec();

    tokio::task::spawn_blocking(move || {
        set_option_default_blocking(&printer_id, &option, &values, &known_printers)
    })
    .await
    .map_err(BackendError::Join)?
}

fn set_option_default_blocking(
    printer_id: &str,
    option: &str,
    values: &[String],
    known_printers: &[PrinterEntry],
) -> BackendResult<()> {
    let (queue, instance) = split_queue_instance(printer_id);
    // An `lpoptions` line holds one `name=value` per option, which is how libcups spells
    // a multiple-valued one too.
    let value = values.join(",");

    edit(known_printers, |destinations| {
        destinations.set_destination_option(queue, instance, option, &value)
    })
}

/// Applies one edit to the user's saved destinations.
fn edit(
    known_printers: &[PrinterEntry],
    change: impl FnOnce(&mut Destinations) -> cups_rs::Result<()>,
) -> BackendResult<()> {
    let mut destinations = Destinations::new();
    for printer in known_printers {
        let (queue, instance) = split_queue_instance(printer.id());
        let Some(saved) = Destinations::named_destination(queue, instance) else {
            continue;
        };

        destinations.add_destination(queue, instance).cups_err()?;
        if saved.is_default {
            destinations
                .set_default_destination(queue, instance)
                .cups_err()?;
        }
        for (option, value) in &saved.options {
            destinations
                .set_destination_option(queue, instance, option, value)
                .cups_err()?;
        }
    }

    change(&mut destinations).cups_err()?;
    destinations.save_to_lpoptions().cups_err()?;

    Ok(())
}
