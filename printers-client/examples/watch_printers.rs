use cosmic_settings_printers_client::connect;
use futures_util::StreamExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = connect().await?;
    let mut events = client.printer_events().await?;

    println!("watching printer events");
    while let Some(event) = events.next().await {
        println!("{:?}", event?);
    }

    Ok(())
}
