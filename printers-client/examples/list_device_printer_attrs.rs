//! Dumps attributes collected from CUPS and reachable devices.

use cosmic_settings_printers_client::{PrinterEntry, connect};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = connect().await?;
    client.refresh_available_destinations().await?;

    // Allow the asynchronous device-enrichment pass to finish.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    let printers = client.printers().await?;
    println!("found {} destination(s)", printers.len());
    for printer in &printers {
        print_options(printer);
    }

    Ok(())
}

fn print_options(printer: &PrinterEntry) {
    println!();
    println!("{} ({})", printer.name(), printer.id());
    println!("  device-uri: {:?}", printer.device_uri());
    println!("  endpoint: {:?}", printer.endpoint());

    let mut options = printer.options().collect::<Vec<_>>();
    options.sort_unstable_by_key(|(name, _)| *name);
    println!("  options held by the service:");
    for (name, value) in options {
        println!("    {name}: {value}");
    }
}
