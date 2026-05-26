use cosmic_settings_printers_core::{Error, PrinterEntry, PrinterStatus};
use cups_rs::{
    Destination, IppOperation, IppRequest, IppTag, IppValueTag, PrinterState as CupsPrinterState,
    enum_destinations,
};

const PRINTER_ATTRIBUTES: &[&str] = &[
    "printer-state",
    "printer-state-message",
    "printer-state-reasons",
    "printer-is-accepting-jobs",
    "printer-type",
    "printer-location",
    "printer-info",
    "printer-make-and-model",
    "device-uri",
    "marker-colors",
    "marker-levels",
    "marker-names",
    "marker-types",
    "media-default",
    "media-supported",
    "sides-default",
    "sides-supported",
];

pub async fn list_printers() -> Result<Vec<PrinterEntry>, Error> {
    let destinations = tokio::task::spawn_blocking(|| {
        let mut destinations = Vec::new();

        enum_destinations(
            cups_rs::DEST_FLAGS_NONE,
            250,
            None,
            0,
            0,
            &mut |flags, dest, dests: &mut Vec<Destination>| {
                if (flags & cups_rs::DEST_FLAGS_REMOVED) == 0 {
                    dests.push(dest.clone());
                }
                true
            },
            &mut destinations,
        )
        .map_err(|_| Error::CupsFailed)?;

        for destination in &mut destinations {
            fill_missing_attrs(destination, PRINTER_ATTRIBUTES)?;
        }

        Ok::<Vec<Destination>, Error>(destinations)
    })
    .await
    .map_err(|_| Error::CupsFailed)??;

    Ok(destinations
        .into_iter()
        .map(destination_to_printer_entry)
        .collect())
}

fn fill_missing_attrs(destination: &mut Destination, attrs: &[&str]) -> Result<(), Error> {
    let mut missing = Vec::new();
    for attr in attrs {
        if !destination.options.contains_key(*attr) {
            missing.push(*attr);
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let printer_uri = destination
        .uri()
        .cloned()
        .unwrap_or_else(|| local_printer_uri(destination));

    let request = printer_attrs_request(&printer_uri, &missing)?;
    let response = request.send_default("/").map_err(|_| Error::CupsFailed)?;

    if !response.status().is_successful() {
        return Err(Error::CupsFailed);
    }

    for name in missing {
        let Some(attr) = response.find_attribute(name, None) else {
            continue;
        };

        let values = attr_values(name, attr);
        if values.is_empty() {
            continue;
        }

        destination
            .options
            .insert(name.to_string(), values.join(","));
    }

    Ok(())
}

fn printer_attrs_request(printer_uri: &str, requested_attrs: &[&str]) -> Result<IppRequest, Error> {
    let mut request =
        IppRequest::new(IppOperation::GetPrinterAttributes).map_err(|_| Error::CupsFailed)?;

    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            printer_uri,
        )
        .map_err(|_| Error::CupsFailed)?;

    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            requested_attrs,
        )
        .map_err(|_| Error::CupsFailed)?;

    Ok(request)
}

fn attr_values(name: &str, attr: cups_rs::IppAttribute) -> Vec<String> {
    if name == "printer-is-accepting-jobs" {
        let mut values = Vec::new();
        for index in 0..attr.count() {
            values.push(attr.get_boolean(index).to_string());
        }
        return values;
    }

    let mut values = Vec::new();
    for index in 0..attr.count() {
        let Some(value) = attr.get_string(index) else {
            continue;
        };

        let value = value.trim();
        if !value.is_empty() {
            values.push(value.to_string());
        }
    }

    if !values.is_empty() {
        return values;
    }

    for index in 0..attr.count() {
        values.push(attr.get_integer(index).to_string());
    }

    values
}

fn local_printer_uri(destination: &Destination) -> String {
    let path = if is_printer_class(destination) {
        "classes"
    } else {
        "printers"
    };

    format!("ipp://localhost/{path}/{}", destination.name)
}

fn is_printer_class(destination: &Destination) -> bool {
    let Some(printer_type) = destination.options.get("printer-type") else {
        return false;
    };

    let Ok(printer_type) = printer_type.parse::<u32>() else {
        return false;
    };

    printer_type & cups_rs::PRINTER_CLASS != 0
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
        options: destination.options,
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
