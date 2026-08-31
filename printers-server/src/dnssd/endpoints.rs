//! What an `_ipp._tcp` or `_ipps._tcp` advertisement says about where a printer answers.
//! The resolved address keeps mDNS-named pages reachable without `libnss-mdns`.

use cosmic_settings_printers_core::is_local_address;
use cups_rs::DnssdResolveEvent;

use super::normalize;
use crate::state::{DnssdDeviceEndpoint, State};

pub(super) fn record_device_resolution(
    context: &State,
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
