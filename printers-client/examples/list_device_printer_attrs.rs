use cups_rs::{
    ConnectionFlags, Destination, IppAttribute, IppOperation, IppRequest, IppTag, IppValueTag,
    get_all_destinations,
};

const PRINTER_ATTRIBUTES: &[&str] = &[
    "printer-uri-supported",
    "printer-more-info",
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
    "printer-uuid",
    "device-uuid",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut destinations = get_all_destinations()?;
    println!("found {} CUPS destination(s)", destinations.len());

    for destination in &mut destinations {
        println!();
        println!("{} ({})", destination.info().unwrap_or(&destination.name), destination.full_name());
        println!("  device-uri: {:?}", destination.device_uri());

        match fill_missing_attrs_from_device_uri(destination, PRINTER_ATTRIBUTES) {
            Ok(()) => print_options(destination),
            Err(error) => println!("  device query failed: {error}"),
        }
    }

    Ok(())
}

/// Fetches attributes missing from the CUPS destination directly from its
/// underlying device URI.
fn fill_missing_attrs_from_device_uri(
    destination: &mut Destination,
    attrs: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let missing = attrs
        .iter()
        .copied()
        .filter(|name| !destination.options.contains_key(*name))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    let device_uri = destination
        .device_uri()
        .cloned()
        .ok_or("destination has no device-uri")?;
    if !device_uri.starts_with("ipp://") && !device_uri.starts_with("ipps://") {
        return Err(format!("device URI is not IPP: {device_uri}").into());
    }

    let connection = destination.connect(ConnectionFlags::Device, Some(5000), None)?;
    let mut request = IppRequest::new(IppOperation::GetPrinterAttributes)?;
    request.add_string(
        IppTag::Operation,
        IppValueTag::Uri,
        "printer-uri",
        &device_uri,
    )?;
    request.add_strings(
        IppTag::Operation,
        IppValueTag::Keyword,
        "requested-attributes",
        &missing,
    )?;

    let response = request.send(&connection, connection.resource_path())?;
    if !response.status().is_successful() {
        return Err(format!("Get-Printer-Attributes returned {:?}", response.status()).into());
    }

    for name in missing {
        let Some(attribute) = response.find_attribute(name, None) else {
            continue;
        };
        let values = attribute_values(name, attribute);
        if !values.is_empty() {
            destination.options.insert(name.to_string(), values.join(","));
        }
    }

    Ok(())
}

fn attribute_values(name: &str, attribute: IppAttribute) -> Vec<String> {
    if name == "printer-is-accepting-jobs" {
        return (0..attribute.count())
            .map(|index| attribute.get_boolean(index).to_string())
            .collect();
    }

    let strings = (0..attribute.count())
        .filter_map(|index| attribute.get_string(index))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if !strings.is_empty() {
        return strings;
    }

    (0..attribute.count())
        .map(|index| attribute.get_integer(index).to_string())
        .collect()
}

fn print_options(destination: &Destination) {
    let mut options = destination.options.iter().collect::<Vec<_>>();
    options.sort_unstable_by_key(|(name, _)| *name);
    println!("  options after direct device query:");
    for (name, value) in options {
        println!("    {name}: {value}");
    }
}
