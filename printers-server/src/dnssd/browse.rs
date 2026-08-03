//! The browse and resolve loop, and which kind of service each resolution is.

use cups_rs::{Dnssd, DnssdBrowseEvent, DnssdServiceResolver};

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::Duration;

use super::{applications, endpoints, normalize};
use crate::state::State;

const SYSTEM_SERVICE_TYPES: &[&str] = &["_ipp-system._tcp", "_ipps-system._tcp"];
const DEVICE_SERVICE_TYPES: &[&str] = &["_ipp._tcp", "_ipps._tcp"];
const MAX_ACTIVE_RESOLVERS: usize = 8;

pub(super) fn run_system_service_browser(
    context: State,
    runtime: tokio::runtime::Handle,
) -> cups_rs::Result<()> {
    let (error_sender, error_receiver) = mpsc::channel();
    let (browse_sender, browse_receiver) = mpsc::channel();
    let dnssd = Dnssd::new(error_sender)?;

    // Keep the shared context when only one service-type browser fails.
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
                    if resolvers.len() >= MAX_ACTIVE_RESOLVERS {
                        services.remove(&key);
                        tracing::warn!(
                            active_resolvers = resolvers.len(),
                            service_name = event.name,
                            "DNS-SD resolver concurrency limit reached"
                        );
                        continue;
                    }

                    match dnssd.resolve_service(&event) {
                        Ok(resolver) => {
                            resolvers.insert(key, resolver);
                        }
                        // Remove the deduplication key after failure so a later DNS-SD announcement
                        // can retry once Avahi is ready.
                        Err(error) => {
                            services.remove(&key);
                            tracing::warn!(service_name = event.name, %error, "failed to resolve system service");
                        }
                    }
                }
            } else {
                services.remove(&key);
                resolvers.remove(&key);
                application_ids.remove(&key);
                applications::retain_active(&context, &runtime, &application_ids);
            }
        }

        let mut failed_resolvers = Vec::new();

        for (key, resolver) in &mut resolvers {
            // One resolver failing says nothing about the rest, and ending the loop
            // would drop every browser and resolver with it.
            let resolved = match resolver.try_recv() {
                Ok(resolved) => resolved,
                Err(error) => {
                    tracing::warn!(%error, "could not read a DNS-SD resolution");
                    failed_resolvers.push(key.clone());
                    continue;
                }
            };

            if let Some(resolved) = resolved
                && services.contains(key)
            {
                if is_system_service(&resolved.service.service_type) {
                    let mut application = applications::resolved_application(resolved.service);
                    application.addresses = resolved
                        .addresses
                        .into_iter()
                        .map(|address| address.to_string())
                        .collect();
                    application_ids.insert(key.clone(), application.id.clone());
                    runtime.block_on(crate::printer_app::record_discovery(
                        context.clone(),
                        application,
                    ));
                } else {
                    endpoints::record_device_resolution(
                        &context,
                        resolved.service,
                        &resolved.addresses,
                    );
                }
            }
        }

        for key in failed_resolvers {
            resolvers.remove(&key);
            services.remove(&key);
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

pub(super) type ServiceKey = (u32, String, String, String);

fn service_key(service: &DnssdBrowseEvent) -> ServiceKey {
    (
        service.interface_index,
        normalize(&service.name),
        normalize(&service.service_type),
        normalize(&service.domain),
    )
}
