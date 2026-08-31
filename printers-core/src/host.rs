//! Host identity and locality checks.

use std::net::IpAddr;

use nix::ifaddrs::getifaddrs;
use nix::sys::socket::SockaddrStorage;

/// Returns whether two host strings name the same host.
pub(crate) fn hosts_match(left: &str, right: &str) -> bool {
    match (parse_ip(left), parse_ip(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left.eq_ignore_ascii_case(right),
    }
}

pub(crate) fn host_is_known_local(host: &str) -> bool {
    if let Some(ip) = parse_ip(host) {
        return is_local_address(ip);
    }

    host.eq_ignore_ascii_case("localhost")
}

/// Checks local names and interface addresses without blocking on DNS.
pub fn host_is_local(host: &str) -> bool {
    host_is_known_local(host) || host_is_this_machine(host)
}

fn host_is_this_machine(host: &str) -> bool {
    let host = bare_local_name(host);
    if host.is_empty() {
        return false;
    }

    nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .is_some_and(|name| bare_local_name(&name).eq_ignore_ascii_case(host))
}

fn bare_local_name(host: &str) -> &str {
    let host = host.trim().trim_end_matches('.');

    host.rfind('.')
        .filter(|at| host[at + 1..].eq_ignore_ascii_case("local"))
        .map_or(host, |at| &host[..at])
}

pub(crate) fn parse_ip(host: &str) -> Option<IpAddr> {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse().ok()
}

#[doc(hidden)]
pub fn is_local_address(target: IpAddr) -> bool {
    if target.is_loopback() {
        return true;
    }

    let Ok(addrs) = getifaddrs() else {
        return false;
    };

    addrs
        .filter_map(|ifaddr| ifaddr.address)
        .filter_map(|address| sockaddr_to_ip(&address))
        .any(|address| address == target)
}

fn sockaddr_to_ip(addr: &SockaddrStorage) -> Option<IpAddr> {
    if let Some(addr) = addr.as_sockaddr_in() {
        return Some(IpAddr::V4(addr.ip()));
    }

    if let Some(addr) = addr.as_sockaddr_in6() {
        return Some(IpAddr::V6(addr.ip()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_is_local_under_the_name_it_advertises() {
        let hostname = nix::unistd::gethostname()
            .expect("a host has a name")
            .into_string()
            .expect("the host name is utf-8");

        assert!(host_is_local(&hostname));
        assert!(host_is_local(&format!("{hostname}.local")));
        assert!(host_is_local(&format!("{hostname}.local.")));
        assert!(host_is_local(&format!("{}.LOCAL", hostname.to_uppercase())));
    }

    #[test]
    fn some_other_hosts_name_is_not_local() {
        assert!(!host_is_local("a-printer-on-another-desk.local"));
        assert!(!host_is_local("printer.example.com"));
        assert!(!host_is_local(""));
    }

    #[test]
    fn an_mdns_suffix_is_not_mistaken_for_part_of_the_name() {
        assert_eq!(bare_local_name("desktop.local"), "desktop");
        assert_eq!(bare_local_name("desktop.local."), "desktop");
        assert_eq!(bare_local_name("desktop.LOCAL"), "desktop");
        assert_eq!(bare_local_name("desktop"), "desktop");
        assert_eq!(
            bare_local_name("print.local.example.com"),
            "print.local.example.com"
        );
        assert_eq!(bare_local_name("local"), "local");
    }
}
