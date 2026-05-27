use cosmic_settings_printers_client::{CosmicPrintersProxy, connect};

#[tokio::main(flavor = "current_thread")]
async fn main() -> zlink::Result<()> {
    let mut client = connect().await?;
    let reply = client.conn.list_printers().await?;

    match reply {
        Ok(reply) => {
            println!("found {} printer(s)", reply.printers.len());

            for printer in reply.printers {
                println!();
                println!("{} ({})", printer.name, printer.id);
                println!("  web-page: {:?}", printer.web_page);

                let mut options: Vec<_> = printer.options.into_iter().collect();
                options.sort_by(|(left, _), (right, _)| left.cmp(right));

                for (name, value) in options {
                    println!("  {name}: {value}");
                }
            }
        }
        Err(err) => {
            eprintln!("printer service error: {err:?}");
        }
    }

    Ok(())
}
