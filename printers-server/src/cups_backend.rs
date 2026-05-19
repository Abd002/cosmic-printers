use cosmic_settings_printers_core::{Error, PrinterEntry, PrinterStatus};
use cups_rs::{Destination, PrinterState as CupsPrinterState, get_all_destinations};

pub async fn list_printers() -> Result<Vec<PrinterEntry>, Error> {
    let destinations = tokio::task::spawn_blocking(get_all_destinations)
        .await
        .map_err(|_| Error::CupsFailed)?
        .map_err(|_| Error::CupsFailed)?;

    Ok(destinations
        .into_iter()
        .map(destination_to_printer_entry)
        .collect())
}

fn destination_to_printer_entry(destination: Destination) -> PrinterEntry {
    let status = printer_status(&destination);
    let queue_status = destination.state().to_string();
    let id = destination.full_name();
    let name = destination
        .info()
        .filter(|info| !info.is_empty())
        .cloned()
        .unwrap_or_else(|| id.clone());

    PrinterEntry {
        id,
        name,
        status,
        queue_status,
        location: destination.location().cloned().unwrap_or_default(),
        model: destination.make_and_model().cloned().unwrap_or_default(),
        device_name: destination.device_uri().cloned().unwrap_or_default(),
        driver_version: String::new(),
        paper_size_idx: 0,
        print_sides_idx: 0,
        supplies: Vec::new(),
    }
}

fn printer_status(destination: &Destination) -> PrinterStatus {
    if destination
        .state_reasons()
        .iter()
        .any(|reason| reason.contains("toner-low") || reason.contains("toner-empty"))
    {
        return PrinterStatus::LowToner;
    }

    match destination.state() {
        CupsPrinterState::Idle | CupsPrinterState::Processing => PrinterStatus::Ready,
        CupsPrinterState::Stopped | CupsPrinterState::Unknown => PrinterStatus::Offline,
    }
}
