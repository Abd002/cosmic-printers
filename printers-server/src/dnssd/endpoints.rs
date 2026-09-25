//! What an `_ipp._tcp` or `_ipps._tcp` advertisement says about where a printer answers.
//! The resolved address keeps mDNS-named pages reachable without `libnss-mdns`.

use cosmic_settings_printers_core::is_local_address;
use cups_rs::DnssdResolveEvent;

use crate::state::{DnssdDeviceEndpoint, State};

/// Returns whether a cached printer answers for the service.
pub(super) fn record_device_resolution(
    context: &State,
    service_name: String,
    service: DnssdResolveEvent,
    addresses: &[std::net::IpAddr],
) -> bool {
    let is_local = addresses.iter().copied().any(is_local_address);
    context.record_dnssd_device_endpoint(
        service_name,
        DnssdDeviceEndpoint {
            hostname: service.hostname,
            port: service.port,
            address: addresses.first().map(ToString::to_string),
            is_local,
        },
    )
}

pub(super) fn forget_device_resolution(context: &State, service_name: &str) {
    context.remove_dnssd_device_endpoint(service_name);
}
