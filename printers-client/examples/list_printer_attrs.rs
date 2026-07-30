use cosmic_settings_printers_client::connect;
use cosmic_settings_printers_core::PrinterEntry;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = connect().await?;
    let printers = client.printers().await?;

    println!("found {} printer(s)", printers.len());
    for printer in printers {
        print_printer(&printer);
    }

    Ok(())
}

fn print_printer(printer: &PrinterEntry) {
    println!();
    println!("{} ({})", printer.name(), printer.id());
    println!("  id: {}", printer.id());
    println!("  name: {}", printer.name());
    println!("  is-default: {}", printer.is_default());
    println!("  printer-local-uri: {:?}", printer.printer_local_uri());
    println!("  status: {:?}", printer.status());
    println!("  queue-status: {:?}", printer.queue_status());
    println!("  location: {:?}", printer.location());
    println!("  model: {:?}", printer.model());
    println!("  device-uri: {:?}", printer.device_uri());
    println!("  hostname: {:?}", printer.hostname());
    println!("  port: {:?}", printer.port());
    println!("  web-page: {:?}", printer.web_page());
    println!("  driver-version: {:?}", printer.driver_version());
    println!("  supplies:");
    for supply in printer.supplies() {
        println!("    {}: {}%", supply.name, supply.level_percent);
    }
    println!("  paper-sizes: {}", printer.paper_sizes().join(", "));
    println!("  print-sides: {}", printer.print_sides().join(", "));
}
