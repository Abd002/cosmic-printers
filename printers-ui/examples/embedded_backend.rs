//! Lists printers through the embedded backend without starting a renderer.

use cosmic_printers_ui::Backend;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Force the embedded path even when a daemon is available.
    let backend = Backend::embedded();

    // Both calls start asynchronous work and return before caches are populated.
    match backend.refresh_available_destinations().await {
        Ok(()) => println!("refresh started"),
        Err(why) => println!("refresh_available_destinations failed: {why:?}"),
    }

    match backend.start_printer_application_discovery().await {
        Ok(()) => println!("discovery started"),
        Err(why) => println!("start_printer_application_discovery failed: {why:?}"),
    }

    // This one-shot diagnostic waits instead of maintaining an event subscription.
    println!("waiting for the refresh and the browse...");
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    match backend.printers().await {
        Ok(printers) => {
            println!("{} destination(s):", printers.len());
            for printer in &printers {
                println!(
                    "  {:<28} {:<12} {}{}",
                    printer.name(),
                    format!("{:?}", printer.status()),
                    printer.id(),
                    if printer.is_default() {
                        "  (default)"
                    } else {
                        ""
                    },
                );
            }
        }
        Err(why) => println!("printers failed: {why:?}"),
    }

    match backend.printer_applications().await {
        Ok(applications) => {
            println!("{} printer application(s):", applications.len());
            for application in &applications {
                println!(
                    "  {:<28} {}",
                    application.service_name,
                    application.administration_uri()
                );
            }
        }
        Err(why) => println!("printer_applications failed: {why:?}"),
    }
}
