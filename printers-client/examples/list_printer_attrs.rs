use cosmic_settings_printers_client::{CosmicPrintersProxy, connect};
use cosmic_settings_printers_core::PrinterEntry;

#[tokio::main(flavor = "current_thread")]
async fn main() -> zlink::Result<()> {
    let mut client = connect().await?;
    let reply = client.conn.list_printers().await?;

    match reply {
        Ok(reply) => {
            println!("found {} printer(s)", reply.printers.len());

            for printer in reply.printers {
                print_printer(&printer);
            }
        }
        Err(err) => {
            eprintln!("printer service error: {err:?}");
        }
    }

    Ok(())
}

fn print_printer(printer: &PrinterEntry) {
    println!();
    println!("{} ({})", printer.name, printer.id);
    println!("  id: {}", printer.id);
    println!("  name: {}", printer.name);
    println!("  is-default: {}", printer.is_default);
    println!("  printer-local-uri: {}", printer.printer_local_uri);
    println!("  status: {:?}", printer.status);
    println!("  queue-status: {}", printer.queue_status);
    println!("  location: {}", printer.location);
    println!("  model: {}", printer.model);
    println!("  device-uri: {}", printer.device_uri);
    println!("  hostname: {:?}", printer.hostname);
    println!("  port: {:?}", printer.port);
    println!("  web-page: {:?}", printer.web_page);
    println!("  driver-version: {}", printer.driver_version);
    println!("  paper-size-idx: {}", printer.paper_size_idx);
    println!("  print-sides-idx: {}", printer.print_sides_idx);
    println!("  supplies:");
    for supply in &printer.supplies {
        println!("    {}: {}%", supply.name, supply.level_percent);
    }
    println!("  paper-sizes: {}", printer.paper_sizes.join(", "));
    println!("  print-sides: {}", printer.print_sides.join(", "));

    let mut options: Vec<_> = printer.options.iter().collect();
    options.sort_by(|(left, _), (right, _)| left.cmp(right));

    println!("  options:");
    for (name, value) in options {
        println!("    {name}: {value}");
    }
}
