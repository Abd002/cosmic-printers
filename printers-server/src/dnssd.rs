use cosmic_settings_printers_core::{
    PrinterApplication, PrinterApplicationCapabilities, PrinterApplicationId,
    PrinterApplicationState, is_local_address,
};
use cups_rs::{Dnssd, DnssdBrowseEvent, DnssdResolveEvent, DnssdServiceResolver};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

use crate::context::{Context, DnssdDeviceEndpoint};

const SYSTEM_SERVICE_TYPES: &[&str] = &["_ipp-system._tcp", "_ipps-system._tcp"];
const DEVICE_SERVICE_TYPES: &[&str] = &["_ipp._tcp", "_ipps._tcp"];

pub(crate) async fn start_printer_application_discovery(context: Context) {
    let Some(discovery_lease) = context.try_start_printer_application_discovery() else {
        return;
    };

    let runtime = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let _discovery_lease = discovery_lease;
        if let Err(error) = run_system_service_browser(context, runtime) {
            tracing::warn!(error = %error, "libcups DNS-SD discovery failed");
        }
    });
}

fn run_system_service_browser(
    context: Context,
    runtime: tokio::runtime::Handle,
) -> cups_rs::Result<()> {
    let (error_sender, error_receiver) = mpsc::channel();
    let (browse_sender, browse_receiver) = mpsc::channel();
    let dnssd = Dnssd::new(error_sender)?;

    // One service type failing to browse is not a reason to give up the whole
    // context: the others still find applications and printers, and tearing the
    // context down is worse than running with less than all of it.
    let mut browsers = Vec::new();
    for service_type in SYSTEM_SERVICE_TYPES.iter().chain(DEVICE_SERVICE_TYPES) {
        match dnssd.browse(service_type, None, browse_sender.clone()) {
            Ok(browser) => browsers.push(browser),
            Err(error) => {
                tracing::warn!(service_type, %error, "could not browse a DNS-SD service type");
            }
        }
    }
    if browsers.is_empty() {
        return Err(cups_rs::Error::NetworkError(
            "no DNS-SD service type could be browsed".into(),
        ));
    }

    let mut resolvers = HashMap::<ServiceKey, DnssdServiceResolver>::new();
    let mut services = HashSet::new();
    let mut application_ids = HashMap::<ServiceKey, String>::new();

    loop {
        while let Ok(event) = browse_receiver.try_recv() {
            let key = service_key(&event);
            if event.added {
                if services.insert(key.clone()) {
                    match dnssd.resolve_service(&event) {
                        Ok(resolver) => {
                            resolvers.insert(key, resolver);
                        }
                        Err(error) => {
                            tracing::warn!(service_name = event.name, %error, "failed to resolve system service");
                        }
                    }
                }
            } else {
                services.remove(&key);
                resolvers.remove(&key);
                application_ids.remove(&key);
                retain_active_applications(&context, &runtime, &application_ids);
            }
        }

        for (key, resolver) in &mut resolvers {
            // One resolver failing says nothing about the rest, and ending the loop
            // would drop every browser and resolver with it.
            let resolved = match resolver.try_recv() {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::warn!(%error, "could not read a DNS-SD resolution");
                    continue;
                }
            };

            if let Some(resolved) = resolved
                && services.contains(key)
            {
                if is_system_service(&resolved.service.service_type) {
                    let mut application = resolved_application(resolved.service);
                    application.addresses = resolved
                        .addresses
                        .into_iter()
                        .map(|address| address.to_string())
                        .collect();
                    application_ids.insert(key.clone(), application.id.clone());
                    runtime.block_on(crate::printer_application_backend::record_discovery(
                        context.clone(),
                        application,
                    ));
                } else {
                    record_device_resolution(&context, resolved.service, &resolved.addresses);
                }
            }
        }

        while let Ok(message) = error_receiver.try_recv() {
            tracing::warn!(message, "libcups DNS-SD error");
        }

        std::thread::sleep(Duration::from_millis(20));
    }
}

fn is_system_service(service_type: &str) -> bool {
    SYSTEM_SERVICE_TYPES
        .iter()
        .any(|candidate| service_type.eq_ignore_ascii_case(candidate))
}

fn record_device_resolution(
    context: &Context,
    service: DnssdResolveEvent,
    addresses: &[std::net::IpAddr],
) {
    let service_name = normalize(&service.full_name);
    let is_local = addresses.iter().copied().any(is_local_address);
    context.record_dnssd_device_endpoint(
        service_name,
        DnssdDeviceEndpoint {
            hostname: service.hostname,
            port: service.port,
            address: addresses.first().map(ToString::to_string),
            is_local,
        },
    );
}

fn retain_active_applications(
    context: &Context,
    runtime: &tokio::runtime::Handle,
    application_ids: &HashMap<ServiceKey, String>,
) {
    let active_ids = application_ids.values().cloned().collect::<HashSet<_>>();
    runtime.block_on(context.retain_printer_applications(&active_ids));
}

type ServiceKey = (u32, String, String, String);

fn service_key(service: &DnssdBrowseEvent) -> ServiceKey {
    (
        service.interface_index,
        normalize(&service.name),
        normalize(&service.service_type),
        normalize(&service.domain),
    )
}

/// Builds a Printer Application from a resolved DNS-SD system service.
///
/// Identity is the service instance — name, type, domain — and nothing else. The
/// hostname and port are left out on purpose: an application that restarts on a
/// different port, or that becomes reachable on a second network interface, is
/// the same application and must update in place rather than appear twice.
///
/// The `UUID` TXT record is read and discarded. Several Printer Applications on
/// one machine can advertise the same system UUID, so it cannot tell them apart.
fn resolved_application(service: DnssdResolveEvent) -> PrinterApplication {
    let txt = service.txt.into_iter().collect::<BTreeMap<_, _>>();
    let make_and_model = txt.get("ty").filter(|value| !value.is_empty()).cloned();
    let scheme = if service.service_type.starts_with("_ipps") {
        "ipps"
    } else {
        "ipp"
    };
    let system_uri = format!(
        "{scheme}://{}:{}/ipp/system",
        service.hostname, service.port
    );
    let id = PrinterApplicationId::new(&service.name, &service.service_type, &service.domain);
    // The administration page, which is the root of the same endpoint — never the
    // `/ipp/system` path used to talk to the application.
    let web_interface_uri = (!service.hostname.trim().is_empty()).then(|| {
        let web_scheme = if scheme == "ipps" { "https" } else { "http" };
        format!(
            "{web_scheme}://{}:{}/",
            service.hostname.trim().trim_end_matches('.'),
            service.port
        )
    });

    PrinterApplication {
        id: id.as_key(),
        service_name: service.name,
        service_type: service.service_type,
        domain: service.domain,
        hostname: service.hostname,
        port: service.port,
        addresses: Vec::new(),
        system_uri,
        make_and_model,
        web_interface_uri,
        endpoints: Vec::new(),
        capabilities: PrinterApplicationCapabilities::default(),
        txt,
        state: PrinterApplicationState::Discovered,
    }
}

fn normalize(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolution(
        name: &str,
        interface_index: u32,
        hostname: &str,
        port: u16,
        txt: Vec<(String, String)>,
    ) -> DnssdResolveEvent {
        DnssdResolveEvent {
            name: name.into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local.".into(),
            interface_index,
            full_name: format!("{name}._ipps-system._tcp.local."),
            hostname: hostname.into(),
            port,
            txt,
        }
    }

    #[test]
    fn resolved_system_builds_secure_application_uri() {
        let application = resolved_application(resolution(
            "LPrint",
            2,
            "printer.local",
            8000,
            vec![("ty".into(), "LPrint".into())],
        ));

        assert_eq!(
            application.system_uri,
            "ipps://printer.local:8000/ipp/system"
        );
        assert_eq!(application.make_and_model.as_deref(), Some("LPrint"));
    }

    #[test]
    fn two_applications_sharing_a_system_uuid_stay_separate() {
        let uuid = ("UUID".to_string(), "urn:uuid:shared".to_string());
        let first = resolved_application(resolution(
            "LPrint",
            2,
            "localhost",
            8000,
            vec![uuid.clone()],
        ));
        let second = resolved_application(resolution(
            "PostScript Printer Application",
            2,
            "localhost",
            8001,
            vec![uuid],
        ));

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn the_same_service_on_two_interfaces_is_one_application() {
        let first =
            resolved_application(resolution("LPrint", 2, "printer.local", 8000, Vec::new()));
        let second =
            resolved_application(resolution("LPrint", 3, "printer.local", 8000, Vec::new()));

        assert_eq!(first.id, second.id);
    }

    #[test]
    fn a_restart_on_a_new_port_updates_the_same_application() {
        let before =
            resolved_application(resolution("LPrint", 2, "printer.local", 8000, Vec::new()));
        let after =
            resolved_application(resolution("LPrint", 2, "desktop.local", 8631, Vec::new()));

        assert_eq!(before.id, after.id);
        assert_ne!(before.system_uri, after.system_uri);
    }

    /// Losing one interface must not remove an application that is still
    /// advertised on another. Because identity excludes the interface index,
    /// both browse entries map to one id, and the id stays active while any
    /// entry remains.
    #[test]
    fn dropping_one_interface_keeps_an_application_advertised_elsewhere() {
        let mut application_ids = HashMap::<ServiceKey, String>::new();
        let first =
            resolved_application(resolution("LPrint", 2, "printer.local", 8000, Vec::new()));
        let second =
            resolved_application(resolution("LPrint", 3, "printer.local", 8000, Vec::new()));
        let first_key = (
            2,
            "lprint".into(),
            "_ipps-system._tcp".into(),
            "local".into(),
        );
        let second_key = (
            3,
            "lprint".into(),
            "_ipps-system._tcp".into(),
            "local".into(),
        );
        application_ids.insert(first_key.clone(), first.id.clone());
        application_ids.insert(second_key, second.id);

        application_ids.remove(&first_key);

        let active = application_ids.values().cloned().collect::<HashSet<_>>();
        assert_eq!(active, HashSet::from([first.id]));
    }
}
