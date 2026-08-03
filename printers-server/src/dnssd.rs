use cosmic_settings_printers_core::{
    PrinterApplication, PrinterApplicationState, is_local_address,
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
    let mut browsers = Vec::new();
    for service_type in SYSTEM_SERVICE_TYPES.iter().chain(DEVICE_SERVICE_TYPES) {
        browsers.push(dnssd.browse(service_type, None, browse_sender.clone())?);
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
            while let Some(resolved) = resolver.try_recv()? {
                if services.contains(key) {
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

fn resolved_application(service: DnssdResolveEvent) -> PrinterApplication {
    let txt = service.txt.into_iter().collect::<BTreeMap<_, _>>();
    let system_uuid = txt
        .get("UUID")
        .or_else(|| txt.get("uuid"))
        .filter(|value| !value.is_empty())
        .cloned();
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

    PrinterApplication {
        id: printer_application_id(
            &service.name,
            &service.domain,
            &service.hostname,
            service.port,
        ),
        service_name: service.name,
        service_type: service.service_type,
        domain: service.domain,
        hostname: service.hostname,
        port: service.port,
        addresses: Vec::new(),
        system_uri,
        system_uuid,
        make_and_model,
        operations_supported: Vec::new(),
        txt,
        state: PrinterApplicationState::Discovered,
    }
}

fn printer_application_id(name: &str, domain: &str, hostname: &str, port: u16) -> String {
    format!(
        "dnssd-system:{}:{}:{}:{port}",
        normalize(name),
        normalize(domain),
        normalize(hostname)
    )
}

fn normalize(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_system_builds_secure_application_uri() {
        let application = resolved_application(DnssdResolveEvent {
            name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local.".into(),
            interface_index: 2,
            full_name: "LPrint._ipps-system._tcp.local.".into(),
            hostname: "printer.local".into(),
            port: 8000,
            txt: vec![("UUID".into(), "urn:uuid:test".into())],
        });

        assert_eq!(
            application.system_uri,
            "ipps://printer.local:8000/ipp/system"
        );
        assert_eq!(application.system_uuid.as_deref(), Some("urn:uuid:test"));
    }
}
