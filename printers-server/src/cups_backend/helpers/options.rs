use std::collections::HashMap;
use std::net::IpAddr;

/// Checks the CUPS printer-type bitmask for the class flag.
pub(super) fn is_printer_class(options: &HashMap<String, String>) -> bool {
    options
        .get("printer-type")
        .and_then(|printer_type| printer_type.parse::<u32>().ok())
        .is_some_and(|printer_type| printer_type & cups_rs::PRINTER_CLASS != 0)
}

/// Splits a comma-separated CUPS option into trimmed values.
pub(super) fn option_values(options: &HashMap<String, String>, name: &str) -> Vec<String> {
    options
        .get(name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn parse_uri_endpoint(uri: &str) -> Option<(String, u16)> {
    let (scheme, rest) = uri.split_once("://")?;
    let authority = rest.split('/').next()?.rsplit('@').next()?.trim();
    if authority.is_empty() {
        return None;
    }

    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "ipp" | "ipps" => 631,
        "http" => 80,
        "https" => 443,
        _ => return None,
    };

    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let host = &authority[..=end];
        let port = authority
            .get(end + 1..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(default_port);
        return Some((host.to_ascii_lowercase(), port));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if port.parse::<u16>().is_ok() => (host, port.parse::<u16>().ok()),
        _ => (authority, Some(default_port)),
    };

    Some((host.to_ascii_lowercase(), port?))
}

pub(in crate::cups_backend) fn queue_name_from_printer_uri(uri: &str) -> Option<String> {
    let path = uri.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?.trim();

    (!name.is_empty()).then(|| name.to_string())
}

pub(super) fn is_loopback_host(host: &str) -> bool {
    let bare = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);

    bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
