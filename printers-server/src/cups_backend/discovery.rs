use cosmic_settings_printers_core::PrinterEntry;
use url::Url;

use crate::context::Context;

pub(crate) async fn start_discovery(context: Context) {
    let Some(discovery_lease) = context.try_start_discovery() else {
        return;
    };

    tokio::spawn(async move {
        let _discovery_lease = discovery_lease;
        match crate::avahi::discover_printers_into_cache(context.clone()).await {
            Ok(summary) => {
                tracing::debug!(
                    services_seen = summary.services_seen,
                    printers_resolved = summary.printers_resolved,
                    applications_resolved = summary.applications_resolved,
                    warnings = summary.warnings,
                    "printer discovery refresh completed"
                );
            }
            Err(error) => {
                tracing::warn!(error = ?error, "printer discovery refresh failed");
            }
        }
    });
}

pub(crate) fn attach_discovered_metadata(
    destinations: &mut [PrinterEntry],
    discovered: &[PrinterEntry],
) {
    for destination in destinations {
        let Some(discovery) = find_matching_discovery(destination, discovered) else {
            continue;
        };

        copy_discovered_metadata(destination, discovery);
    }
}

fn copy_discovered_metadata(destination: &mut PrinterEntry, discovery: &PrinterEntry) {
    copy_option(destination, "device-uri", discovery.device_uri());

    copy_option(destination, "device-uuid", discovery.device_uuid());

    copy_option(destination, "printer-more-info", discovery.web_page());

    copy_option(destination, "dnssd-address", discovery.dnssd_address());

    if let (Some(hostname), Some(port)) = (discovery.hostname(), discovery.port()) {
        destination.set_option("dnssd-hostname", hostname);
        destination.set_option("dnssd-port", port.to_string());
        destination.set_option("endpoint-hostname", hostname);
        destination.set_option("endpoint-port", port.to_string());
    }
}

fn copy_option(destination: &mut PrinterEntry, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        destination.set_option(name, value);
    }
}

fn find_matching_discovery<'a>(
    destination: &PrinterEntry,
    discovered: &'a [PrinterEntry],
) -> Option<&'a PrinterEntry> {
    find_by_dnssd_identity(destination, discovered)
        .or_else(|| find_by_device_uuid(destination, discovered))
}

fn find_by_dnssd_identity<'a>(
    destination: &PrinterEntry,
    discovered: &'a [PrinterEntry],
) -> Option<&'a PrinterEntry> {
    let identity = dnssd_identity(destination)?;

    discovered
        .iter()
        .find(|candidate| dnssd_identity(candidate).as_ref() == Some(&identity))
}

fn find_by_device_uuid<'a>(
    destination: &PrinterEntry,
    discovered: &'a [PrinterEntry],
) -> Option<&'a PrinterEntry> {
    let destination_uuid = normalized_uuid(destination.device_uuid())?;

    discovered.iter().find(|candidate| {
        normalized_uuid(candidate.device_uuid()).is_some_and(|uuid| uuid == destination_uuid)
    })
}

#[derive(Debug, PartialEq, Eq)]
struct DnssdIdentity {
    service_name: String,
    service_type: String,
    domain: String,
}

fn dnssd_identity(printer: &PrinterEntry) -> Option<DnssdIdentity> {
    let explicit_identity = || {
        Some(DnssdIdentity {
            service_name: normalize_dns_name(printer.option("dnssd-service-name")?),
            service_type: normalize_dns_name(printer.option("dnssd-service-type")?),
            domain: normalize_dns_name(printer.option("dnssd-domain")?),
        })
    };

    explicit_identity().or_else(|| dnssd_identity_from_uri(printer.device_uri()?))
}

fn dnssd_identity_from_uri(uri: &str) -> Option<DnssdIdentity> {
    const SERVICE_TYPES: [&str; 2] = ["_ipps._tcp", "_ipp._tcp"];

    let uri = Url::parse(uri).ok()?;
    let host = uri.host_str()?.trim_end_matches('.');
    let lowercase_host = host.to_ascii_lowercase();

    for service_type in SERVICE_TYPES {
        let separator = format!(".{service_type}.");

        let Some(separator_index) = lowercase_host.rfind(&separator) else {
            continue;
        };

        let service_name = &host[..separator_index];
        let domain = &host[separator_index + separator.len()..];

        if service_name.is_empty() || domain.is_empty() {
            return None;
        }

        return Some(DnssdIdentity {
            service_name: normalize_dns_name(service_name),
            service_type: service_type.to_owned(),
            domain: normalize_dns_name(domain),
        });
    }

    None
}

fn normalize_dns_name(value: &str) -> String {
    value.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn normalized_uuid(value: Option<&str>) -> Option<String> {
    const UUID_PREFIX: &str = "urn:uuid:";

    let value = value?.trim();

    let value = value
        .strip_prefix(UUID_PREFIX)
        .or_else(|| {
            value
                .get(..UUID_PREFIX.len())
                .filter(|prefix| prefix.eq_ignore_ascii_case(UUID_PREFIX))
                .map(|_| &value[UUID_PREFIX.len()..])
        })
        .unwrap_or(value);

    let normalized = value.trim_matches(['{', '}']).to_ascii_lowercase();

    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn printer(id: &str, name: &str, options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            id,
            name,
            false,
            options
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn attaches_resolved_metadata_by_dns_sd_service_identity() {
        let mut destinations = vec![printer(
            "Abd_Office",
            "Abd-Office",
            &[
                ("device-uri", "ipps://Abd-Office._ipps._tcp.local/"),
                ("endpoint-hostname", "abd-office._ipps._tcp.local"),
                ("endpoint-port", "631"),
                ("printer-more-info", "https://abd-office._ipps._tcp.local/"),
            ],
        )];
        let discovered = vec![printer(
            "",
            "Abd-Office",
            &[
                ("dnssd-service-name", "Abd-Office"),
                ("dnssd-service-type", "_ipps._tcp"),
                ("dnssd-domain", "local"),
                ("dnssd-hostname", "DESKTOP-96VEKVC.local"),
                ("dnssd-address", "192.168.1.2"),
                ("dnssd-port", "8884"),
                ("device-uri", "ipps://DESKTOP-96VEKVC.local:8884/ipp/print"),
                ("device-uuid", "a94f7fbb-2ea6-3648-67e4-3c96da8b1aae"),
                ("printer-more-info", "https://DESKTOP-96VEKVC.local:8884/"),
            ],
        )];

        attach_discovered_metadata(&mut destinations, &discovered);

        let destination = &destinations[0];
        assert_eq!(destination.id(), "Abd_Office");
        assert_eq!(destination.name(), "Abd-Office");
        assert_eq!(
            destination.device_uri(),
            Some("ipps://DESKTOP-96VEKVC.local:8884/ipp/print")
        );
        assert_eq!(
            destination.device_uuid(),
            Some("a94f7fbb-2ea6-3648-67e4-3c96da8b1aae")
        );
        assert_eq!(destination.hostname(), Some("DESKTOP-96VEKVC.local"));
        assert_eq!(destination.dnssd_address(), Some("192.168.1.2"));
        assert_eq!(destination.port(), Some(8884));
        assert_eq!(
            destination.web_page(),
            Some("https://DESKTOP-96VEKVC.local:8884/")
        );
    }

    #[test]
    fn does_not_attach_metadata_using_only_a_display_name() {
        let mut destinations = vec![printer(
            "queue",
            "Office Printer",
            &[("device-uri", "ipp://unrelated.example/ipp/print")],
        )];
        let discovered = vec![printer(
            "",
            "Office Printer",
            &[
                ("dnssd-service-name", "Office Printer"),
                ("dnssd-service-type", "_ipp._tcp"),
                ("dnssd-domain", "local"),
                ("device-uri", "ipp://printer.local/ipp/print"),
            ],
        )];

        attach_discovered_metadata(&mut destinations, &discovered);

        assert_eq!(
            destinations[0].device_uri(),
            Some("ipp://unrelated.example/ipp/print")
        );
    }

    #[test]
    fn falls_back_to_normalized_device_uuid() {
        let mut destinations = vec![printer(
            "queue",
            "Queue",
            &[
                ("device-uri", "ipp://queue.example/ipp/print"),
                (
                    "device-uuid",
                    "urn:uuid:A94F7FBB-2EA6-3648-67E4-3C96DA8B1AAE",
                ),
            ],
        )];
        let discovered = vec![printer(
            "",
            "Discovered",
            &[
                ("device-uri", "ipps://printer.local:8884/ipp/print"),
                ("device-uuid", "a94f7fbb-2ea6-3648-67e4-3c96da8b1aae"),
            ],
        )];

        attach_discovered_metadata(&mut destinations, &discovered);

        assert_eq!(
            destinations[0].device_uri(),
            Some("ipps://printer.local:8884/ipp/print")
        );
    }
}
