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
    println!("  printer-uri: {:?}", printer.printer_uri());
    println!("  device-uri: {:?}", printer.device_uri());
    println!("  hostname: {:?}", printer.hostname());
    println!("  port: {:?}", printer.port());
    println!("  web-page: {:?}", printer.web_page());
    for supply in printer.supplies() {
        println!("    {}: {}%", supply.name, supply.level_percent);
    }
    println!("  options:");

    let mut options = printer.options().collect::<Vec<_>>();
    options.sort_unstable_by_key(|(name, _)| *name);
    for (name, value) in options {
        println!("    {name}: {value}");
    }
}
