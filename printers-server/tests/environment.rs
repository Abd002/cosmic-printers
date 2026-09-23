//! Whether a printer on the network is found at all, and where it answers.
//! Needs the fixtures: `ci/fixtures.sh start && ci/fixtures.sh wait`.

use cosmic_settings_printers_core::PrinterEntry;
use cosmic_settings_printers_server::Server;
use std::time::{Duration, Instant};

/// libcups replaces the spaces in an advertised name.
const PRINTER_A: &str = "CI_Test_Printer_A";
const PRINTER_B: &str = "CI_Test_Printer_B";
const PORT_A: u16 = 8801;

/// Long enough for a browse and a resolve on a slow runner.
const TIMEOUT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(2);

const SETUP: &str = "fixtures are not running: ci/fixtures.sh start && ci/fixtures.sh wait";

/// Refreshes until `ready` accepts what was found, because discovery arrives
/// when it arrives.
async fn printers_until(
    server: &Server,
    ready: impl Fn(&[PrinterEntry]) -> bool,
) -> Vec<PrinterEntry> {
    let deadline = Instant::now() + TIMEOUT;

    loop {
        server
            .refresh_available_destinations()
            .await
            .expect("refreshing destinations failed");

        let printers = server
            .list_printers()
            .await
            .expect("listing printers failed");
        if ready(&printers) {
            return printers;
        }

        assert!(
            Instant::now() < deadline,
            "{SETUP}\nfound: {:?}",
            printers
                .iter()
                .map(|printer| (printer.id(), printer.option("dnssd-port")))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(POLL).await;
    }
}

fn find<'a>(printers: &'a [PrinterEntry], id: &str) -> Option<&'a PrinterEntry> {
    printers.iter().find(|printer| printer.id() == id)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs fixture printers: ci/fixtures.sh start"]
async fn the_fixtures_are_detected() {
    let server = Server::new();

    let printers = printers_until(&server, |printers| {
        find(printers, PRINTER_A).is_some() && find(printers, PRINTER_B).is_some()
    })
    .await;

    // Both, so one stale name cannot carry the test.
    assert!(find(&printers, PRINTER_A).is_some(), "{SETUP}");
    assert!(find(&printers, PRINTER_B).is_some(), "{SETUP}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs fixture printers: ci/fixtures.sh start"]
async fn a_detected_printer_resolves_to_where_it_answers() {
    let server = Server::new();
    server
        .start_printer_application_discovery()
        .await
        .expect("starting discovery failed");

    // Waiting on the port itself, not just its presence: the scheduler's own
    // port answers first, before the printer's real one is resolved.
    let printers = printers_until(&server, |printers| {
        find(printers, PRINTER_A).is_some_and(|printer| printer.port() == Some(PORT_A))
    })
    .await;

    let printer = find(&printers, PRINTER_A).expect(SETUP);
    assert!(printer.hostname().is_some());
    assert_eq!(printer.option("endpoint-is-local"), Some("true"));
}
