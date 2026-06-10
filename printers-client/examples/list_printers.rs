use cosmic_settings_printers_client::{CosmicPrintersProxy, connect};

#[tokio::main(flavor = "current_thread")]
async fn main() -> zlink::Result<()> {
    let mut client = connect().await?;
    let reply = client.conn.list_printers().await?;

    match reply {
        Ok(reply) => {
            println!("found {} printer(s)", reply.printers.len());
            for printer in reply.printers {
                println!(
                    "{} | {:?} | {} | {}",
                    printer.name, printer.status, printer.queue_status, printer.device_uri
                );
            }
        }
        Err(err) => {
            eprintln!("printer service error: {err:?}");
        }
    }

    Ok(())
}
