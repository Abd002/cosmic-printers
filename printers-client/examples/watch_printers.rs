use cosmic_settings_printers_client::{CosmicPrintersProxy, connect};
use futures_util::StreamExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = connect().await?;
    let mut events = client.conn.watch_printers().await?;

    println!("watching printer events");
    while let Some(event) = events.next().await {
        println!("{:?}", event?);
    }

    Ok(())
}
