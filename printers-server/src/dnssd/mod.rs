//! DNS-SD discovery for Printer Applications and printer endpoints.

mod applications;
mod browse;
mod endpoints;

use crate::state::State;

/// How long to wait before browsing again after Avahi went away.
const DISCOVERY_RESTART_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) async fn start_printer_application_discovery(context: State) {
    let Some(discovery_lease) = context.try_start_printer_application_discovery() else {
        return;
    };

    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let _discovery_lease = discovery_lease;
        // Browsing ends when Avahi goes away, so it is started again once it is back.
        loop {
            if let Err(error) = browse::run_system_service_browser(context.clone(), runtime.clone())
            {
                tracing::warn!(error = %error, "libcups DNS-SD discovery failed");
            }
            std::thread::sleep(DISCOVERY_RESTART_DELAY);
        }
    });
}

/// Normalizes case and trailing dots for DNS-SD comparison.
fn normalize(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}
