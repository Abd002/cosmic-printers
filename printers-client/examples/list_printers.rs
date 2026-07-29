use cosmic_settings_printers_client::connect;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = connect().await?;
    let printers = client.printers().await?;

    println!("found {} printer(s)", printers.len());
    for printer in printers {
        println!(
            "{} | {:?} | {} | {}",
            printer.name, printer.status, printer.queue_status, printer.device_uri
        );
    }

    Ok(())
}
