use cosmic_settings_printers_core::{PrinterEntry, PrinterStatus};
use futures_util::TryStreamExt;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use zbus::message::Type;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, MatchRule, MessageStream, proxy};

use crate::context::Context;

const AVAHI_SERVICE: &str = "org.freedesktop.Avahi";
const AVAHI_SERVICE_BROWSER_IFACE: &str = "org.freedesktop.Avahi.ServiceBrowser";
const AVAHI_IF_UNSPEC: i32 = -1;
const AVAHI_PROTO_UNSPEC: i32 = -1;
const DISCOVERY_WINDOW: Duration = Duration::from_millis(900);
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(1_200);

const SERVICE_TYPES: &[&str] = &[
    "_ipp._tcp",
    "_ipps._tcp",
    "_ipp-system._tcp",
    "_ipps-system._tcp",
];

#[proxy(
    interface = "org.freedesktop.Avahi.Server",
    default_service = "org.freedesktop.Avahi",
    default_path = "/"
)]
trait AvahiServer {
    async fn service_browser_new(
        &self,
        interface: i32,
        protocol: i32,
        service_type: &str,
        domain: &str,
        flags: u32,
    ) -> zbus::Result<OwnedObjectPath>;

    async fn resolve_service(
        &self,
        interface: i32,
        protocol: i32,
        name: &str,
        service_type: &str,
        domain: &str,
        aprotocol: i32,
        flags: u32,
    ) -> zbus::Result<ResolvedService>;
}

type ResolvedService = (
    i32,
    i32,
    String,
    String,
    String,
    String,
    i32,
    String,
    u16,
    Vec<Vec<u8>>,
    u32,
);

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AvahiService {
    interface: i32,
    protocol: i32,
    name: String,
    service_type: String,
    domain: String,
}

pub async fn discover_printers_into_cache(context: Context) {
    let Ok(connection) = Connection::system().await else {
        return;
    };
    let Ok(server) = AvahiServerProxy::new(&connection).await else {
        return;
    };

    let rule_builder = MatchRule::builder().msg_type(Type::Signal);
    let Ok(rule_builder) = rule_builder.sender(AVAHI_SERVICE) else {
        return;
    };
    let Ok(rule_builder) = rule_builder.interface(AVAHI_SERVICE_BROWSER_IFACE) else {
        return;
    };
    let Ok(rule_builder) = rule_builder.member("ItemNew") else {
        return;
    };
    let rule = rule_builder.build();
    let Ok(mut stream) = MessageStream::for_match_rule(rule, &connection, Some(100)).await else {
        return;
    };

    for service_type in SERVICE_TYPES {
        let _ = server
            .service_browser_new(AVAHI_IF_UNSPEC, AVAHI_PROTO_UNSPEC, service_type, "", 0)
            .await;
    }

    let mut services = HashSet::<AvahiService>::new();
    let deadline = tokio::time::sleep(DISCOVERY_WINDOW);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            message = stream.try_next() => {
                let Ok(Some(message)) = message else {
                    break;
                };
                let Ok((interface, protocol, name, service_type, domain, _flags)) =
                    message.body().deserialize::<(i32, i32, String, String, String, u32)>()
                else {
                    continue;
                };
                if !SERVICE_TYPES.contains(&service_type.as_str()) {
                    continue;
                }

                let service = AvahiService {
                    interface,
                    protocol,
                    name,
                    service_type,
                    domain,
                };

                if services.insert(service.clone()) {
                    merge_printer_into_cache(&context, service_to_partial_entry(&service)).await;
                    let Ok(server) = AvahiServerProxy::new(&connection).await else {
                        continue;
                    };
                    if let Some(printer) = resolve_service_entry(server, service).await {
                        merge_printer_into_cache(&context, printer).await;
                    }
                }
            }
        }
    }

    retain_seen_printers(&context, services).await;
}

async fn merge_printer_into_cache(context: &Context, printer: PrinterEntry) {
    context
        .merge_discovered_printer_by(printer, discovered_printers_match)
        .await;
}

async fn retain_seen_printers(context: &Context, services: HashSet<AvahiService>) {
    context
        .retain_discovered_printers_by(
            services
                .into_iter()
                .map(|service| service_to_partial_entry(&service)),
            discovered_printers_match,
        )
        .await;
}

async fn resolve_service_entry(
    server: AvahiServerProxy<'_>,
    service: AvahiService,
) -> Option<PrinterEntry> {
    let resolved = tokio::time::timeout(
        RESOLVE_TIMEOUT,
        server.resolve_service(
            service.interface,
            service.protocol,
            &service.name,
            &service.service_type,
            &service.domain,
            AVAHI_PROTO_UNSPEC,
            0,
        ),
    )
    .await
    .ok()?
    .ok()?;

    let (
        interface,
        protocol,
        name,
        service_type,
        domain,
        hostname,
        _aprotocol,
        address,
        port,
        txt,
        _flags,
    ) = resolved;

    let mut printer = service_to_partial_entry(&AvahiService {
        interface,
        protocol,
        name,
        service_type,
        domain,
    });
    let txt = parse_txt_records(txt);
    let resource_path = txt
        .get("rp")
        .cloned()
        .unwrap_or_else(|| "ipp/print".to_string());
    let device_uri = dnssd_device_uri(&service.service_type, &hostname, port, &resource_path);

    printer.device_uri = device_uri.clone();
    printer.printer_local_uri = device_uri.clone();
    printer.hostname = Some(hostname.clone());
    printer.port = Some(port);
    printer
        .options
        .insert("device-uri".into(), device_uri.clone());
    printer
        .options
        .insert("printer-uri-supported".into(), device_uri);
    printer.options.insert("dnssd-hostname".into(), hostname);
    printer.options.insert("dnssd-address".into(), address);
    printer
        .options
        .insert("dnssd-resource-path".into(), resource_path);
    printer
        .options
        .insert("cosmic-discovery-detail-state".into(), "resolved".into());

    if let Some(location) = txt.get("note") {
        printer.location = location.clone();
        printer
            .options
            .insert("printer-location".into(), location.clone());
    }
    if let Some(admin_url) = txt.get("adminurl").filter(|value| !value.is_empty()) {
        printer.web_page = Some(admin_url.clone());
        printer
            .options
            .insert("printer-more-info".into(), admin_url.clone());
    }
    for (source, destination) in [
        ("UUID", "device-uuid"),
        ("uuid", "device-uuid"),
        ("device-uuid", "device-uuid"),
        ("printer-uuid", "printer-uuid"),
    ] {
        if let Some(uuid) = txt.get(source).filter(|value| !value.is_empty()) {
            printer.options.insert(destination.into(), uuid.clone());
        }
    }

    Some(printer)
}

fn service_to_partial_entry(service: &AvahiService) -> PrinterEntry {
    let mut options = HashMap::new();
    options.insert("cosmic-discovery-source".into(), "avahi".into());
    options.insert("cosmic-discovery-detail-state".into(), "partial".into());
    options.insert("dnssd-service-name".into(), service.name.clone());
    options.insert("dnssd-service-type".into(), service.service_type.clone());
    options.insert("dnssd-domain".into(), service.domain.clone());
    options.insert("dnssd-interface".into(), service.interface.to_string());
    options.insert("dnssd-protocol".into(), service.protocol.to_string());

    PrinterEntry {
        id: String::new(),
        name: service.name.clone(),
        is_default: false,
        printer_local_uri: String::new(),
        status: PrinterStatus::Ready,
        queue_status: String::new(),
        location: String::new(),
        model: String::new(),
        device_uri: String::new(),
        hostname: None,
        port: None,
        web_page: None,
        driver_version: String::new(),
        paper_size_idx: 0,
        print_sides_idx: 0,
        options,
        supplies: Vec::new(),
        paper_sizes: Vec::new(),
        print_sides: Vec::new(),
    }
}

fn parse_txt_records(records: Vec<Vec<u8>>) -> HashMap<String, String> {
    records
        .into_iter()
        .filter_map(|record| String::from_utf8(record).ok())
        .filter_map(|record| {
            let (key, value) = record.split_once('=')?;
            (!key.is_empty()).then(|| (key.to_string(), value.to_string()))
        })
        .collect()
}

pub(crate) fn discovered_printers_match(left: &PrinterEntry, right: &PrinterEntry) -> bool {
    match (discovery_name(left), discovery_name(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

pub(crate) fn discovered_printer_id(printer: &PrinterEntry) -> Option<String> {
    if !printer.id.is_empty() {
        return Some(printer.id.clone());
    }

    let service_type = printer.options.get("dnssd-service-type")?;
    let domain = printer.options.get("dnssd-domain")?;
    let name = printer.options.get("dnssd-service-name")?;
    Some(format!("dnssd:{service_type}:{domain}:{name}"))
}

fn discovery_name(printer: &PrinterEntry) -> Option<String> {
    let name = printer
        .options
        .get("dnssd-service-name")
        .map(String::as_str)
        .unwrap_or(&printer.name)
        .trim()
        .to_ascii_lowercase();

    if name.is_empty() { None } else { Some(name) }
}

fn dnssd_device_uri(service_type: &str, hostname: &str, port: u16, resource_path: &str) -> String {
    let scheme = if service_type.starts_with("_ipps") {
        "ipps"
    } else {
        "ipp"
    };
    let resource_path = resource_path.trim_start_matches('/');
    format!("{scheme}://{hostname}:{port}/{resource_path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_txt_key_value_records() {
        let records = vec![b"rp=ipp/print".to_vec(), b"note=Office".to_vec()];
        let parsed = parse_txt_records(records);
        assert_eq!(parsed.get("rp").map(String::as_str), Some("ipp/print"));
        assert_eq!(parsed.get("note").map(String::as_str), Some("Office"));
    }
}
