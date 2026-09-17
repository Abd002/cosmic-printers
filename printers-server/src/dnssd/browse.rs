//! The browse and resolve loop, and which kind of service each resolution is.

use cups_rs::{Dnssd, DnssdBrowseEvent, DnssdServiceResolver};

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, TryRecvError};
use std::time::{Duration, Instant};

use super::{applications, endpoints, normalize};
use crate::state::State;

const SYSTEM_SERVICE_TYPES: &[&str] = &["_ipp-system._tcp", "_ipps-system._tcp"];
const DEVICE_SERVICE_TYPES: &[&str] = &["_ipp._tcp", "_ipps._tcp"];
const MAX_ACTIVE_RESOLVERS: usize = 10;
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

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

    // The browsers hold every remaining sender, so the channel closing means they have all gone.
    drop(browse_sender);

    let mut resolvers = HashMap::<ServiceKey, DnssdServiceResolver>::new();
    let mut started = HashMap::<ServiceKey, Instant>::new();
    // One browse event arrives per interface and IP protocol, so a service is gone
    // only once the last of them goes away.
    let mut services = HashMap::<ServiceKey, u32>::new();
    let mut application_ids = HashMap::<ServiceKey, String>::new();
    let mut endpoint_names = HashMap::<ServiceKey, String>::new();
    let mut pending_resolutions = VecDeque::<DnssdBrowseEvent>::new();

    loop {
        loop {
            let event = match browse_receiver.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Disconnected) => {
                    return Err(cups_rs::Error::NetworkError(
                        "every DNS-SD browser stopped".into(),
                    ));
                }
                Err(TryRecvError::Empty) if resolvers.len() < MAX_ACTIVE_RESOLVERS => {
                    match pending_resolutions.pop_front() {
                        Some(event) => event,
                        None => break,
                    }
                }
                Err(TryRecvError::Empty) => break,
            };

            let key = service_key(&event);

            if event.added {
                *services.entry(key.clone()).or_default() += 1;

                if resolvers.contains_key(&key)
                    || pending_resolutions
                        .iter()
                        .any(|pending| service_key(pending) == key)
                {
                    continue;
                }

                if resolvers.len() >= MAX_ACTIVE_RESOLVERS {
                    tracing::warn!(
                        active_resolvers = resolvers.len(),
                        service_name = event.name,
                        "DNS-SD resolver concurrency limit reached"
                    );
                    pending_resolutions.push_back(event);
                    continue;
                }

                match dnssd.resolve_service(&event) {
                    Ok(resolver) => {
                        started.insert(key.clone(), Instant::now());
                        resolvers.insert(key, resolver);
                    }
                    Err(error) => {
                        tracing::warn!(service_name = event.name, %error, "failed to resolve system service");
                    }
                }
            } else {
                let Some(count) = services.get_mut(&key) else {
                    continue;
                };
                *count = count.saturating_sub(1);
                if *count > 0 {
                    continue;
                }

                services.remove(&key);
                resolvers.remove(&key);
                started.remove(&key);
                pending_resolutions.retain(|pending| service_key(pending) != key);

                if let Some(service_name) = endpoint_names.remove(&key) {
                    endpoints::forget_device_resolution(&context, &service_name);
                }

                if application_ids.remove(&key).is_some() {
                    applications::retain_active(&context, &runtime, &application_ids);
                }
            }
        }

        let mut failed_resolvers = Vec::new();
        let mut completed_resolvers = Vec::new();

        for (key, resolver) in &mut resolvers {
            // One resolver failing says nothing about the rest, and ending the loop
            // would drop every browser and resolver with it.
            let timed_out = started
                .get(key)
                .is_some_and(|at| at.elapsed() >= RESOLVE_TIMEOUT);

            let resolved = match resolver.try_recv() {
                Ok(None) if timed_out => {
                    completed_resolvers.push(key.clone());
                    continue;
                }
                Ok(None) => continue,
                // The host and its TXT record arrive before any address does, so an
                // answer without one is not final until the deadline says it is.
                Ok(Some(resolved)) if resolved.addresses.is_empty() && !timed_out => continue,
                Ok(resolved) => {
                    completed_resolvers.push(key.clone());
                    resolved
                }
                Err(error) => {
                    tracing::warn!(%error, "could not read a DNS-SD resolution");
                    failed_resolvers.push(key.clone());
                    continue;
                }
            };

            if let Some(resolved) = resolved
                && services.contains_key(key)
            {
                if is_system_service(&resolved.service.service_type) {
                    let mut application = applications::resolved_application(resolved.service);
                    application.addresses = resolved
                        .addresses
                        .into_iter()
                        .map(|address| address.to_string())
                        .collect();
                    // A printer is configured through an application running here, never
                    // through someone else's on the network.
                    if !application.is_local() {
                        continue;
                    }
                    application_ids.insert(key.clone(), application.id.clone());
                    runtime.block_on(crate::printer_app::record_discovery(
                        context.clone(),
                        application,
                    ));
                } else {
                    endpoint_names.insert(key.clone(), normalize(&resolved.service.full_name));
                    endpoints::record_device_resolution(
                        &context,
                        resolved.service,
                        &resolved.addresses,
                    );
                }
            }
        }

        // The service stays advertised on failure, so it is left for a later announcement
        // to retry rather than dropped.
        for key in failed_resolvers {
            resolvers.remove(&key);
            started.remove(&key);
            if application_ids.remove(&key).is_some() {
                applications::retain_active(&context, &runtime, &application_ids);
            }
        }
        for key in completed_resolvers {
            resolvers.remove(&key);
            started.remove(&key);
        }

        while let Ok(message) = error_receiver.try_recv() {
            tracing::warn!(message, "libcups DNS-SD error");
            // Browsers do not survive the daemon going away and libcups does not
            // rebuild them, so they go quiet for good unless discovery starts over.
            if message.contains("Avahi server crashed") {
                return Err(cups_rs::Error::NetworkError(message));
            }
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
        0,
        normalize(&service.name),
        normalize(&service.service_type),
        normalize(&service.domain),
    )
}
