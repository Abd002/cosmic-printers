//! DNS-SD discovery for Printer Applications and printer endpoints.

mod applications;
mod browse;
mod endpoints;

use crate::state::State;

pub(crate) async fn start_printer_application_discovery(context: State) {
    let Some(discovery_lease) = context.try_start_printer_application_discovery() else {
        return;
    };

    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let _discovery_lease = discovery_lease;
        if let Err(error) = browse::run_system_service_browser(context, runtime) {
            tracing::warn!(error = %error, "libcups DNS-SD discovery failed");
        }
    });
}

/// Normalizes case and trailing dots for DNS-SD comparison.
fn normalize(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}
