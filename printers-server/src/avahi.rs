#![allow(clippy::too_many_arguments)]

use cosmic_settings_printers_core::{PrinterApplication, PrinterApplicationState, PrinterEntry};
use futures_util::TryStreamExt;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;
use zbus::message::Type;
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, MatchRule, MessageStream, proxy};

use crate::context::Context;

const AVAHI_SERVICE: &str = "org.freedesktop.Avahi";
const AVAHI_SERVICE_BROWSER_IFACE: &str = "org.freedesktop.Avahi.ServiceBrowser";
const AVAHI_IF_UNSPEC: i32 = -1;
const AVAHI_PROTO_UNSPEC: i32 = -1;
const DISCOVERY_WINDOW: Duration = Duration::from_millis(5000);
const RESOLVE_TIMEOUT: Duration = Duration::from_millis(5000);

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
    ) -> zbus::Result<RawResolvedService>;
}

type RawResolvedService = (
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResolvedServiceKind {
    Printer,
    PrinterApplication,
}

enum ResolvedServiceEntry {
    Printer(PrinterEntry),
    PrinterApplication(PrinterApplication),
}

#[derive(Debug, Default)]
pub(crate) struct DiscoverySummary {
    pub services_seen: usize,
    pub printers_resolved: usize,
    pub applications_resolved: usize,
    pub warnings: usize,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DiscoveryError {
    #[error("failed to initialize Avahi discovery at {stage}: {source}")]
    Setup {
        stage: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Avahi message stream failed: {0}")]
    Stream(#[source] zbus::Error),
}

impl DiscoveryError {
    fn setup(stage: &'static str, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Setup {
            stage,
            source: Box::new(source),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum ResolveError {
    #[error("service resolution timed out")]
    Timeout,

    #[error("Avahi service resolution failed: {0}")]
    Zbus(#[source] zbus::Error),

    #[error("unsupported DNS-SD service type '{0}'")]
    UnsupportedServiceType(String),
}

pub(crate) async fn discover_printers_into_cache(
    context: Context,
) -> Result<DiscoverySummary, DiscoveryError> {
    let connection = Connection::system()
        .await
        .map_err(|error| DiscoveryError::setup("system bus connection", error))?;
    let server = AvahiServerProxy::new(&connection)
        .await
        .map_err(|error| DiscoveryError::setup("server proxy", error))?;

    let rule_builder = MatchRule::builder().msg_type(Type::Signal);
    let rule_builder = rule_builder
        .sender(AVAHI_SERVICE)
        .map_err(|error| DiscoveryError::setup("match-rule sender", error))?;
    let rule_builder = rule_builder
        .interface(AVAHI_SERVICE_BROWSER_IFACE)
        .map_err(|error| DiscoveryError::setup("match-rule interface", error))?;
    let rule_builder = rule_builder
        .member("ItemNew")
        .map_err(|error| DiscoveryError::setup("match-rule member", error))?;
    let rule = rule_builder.build();
    let mut stream = MessageStream::for_match_rule(rule, &connection, Some(5000))
        .await
        .map_err(|error| DiscoveryError::setup("message stream", error))?;
    let mut summary = DiscoverySummary::default();

    for service_type in SERVICE_TYPES {
        if let Err(error) = server
            .service_browser_new(AVAHI_IF_UNSPEC, AVAHI_PROTO_UNSPEC, service_type, "", 0)
            .await
        {
            summary.warnings += 1;
            tracing::warn!(
                service_type,
                error = ?error,
                "failed to create Avahi service browser"
            );
        }
    }

    let mut services = HashSet::<AvahiService>::new();
    let mut active_application_ids = HashSet::<String>::new();
    let deadline = tokio::time::sleep(DISCOVERY_WINDOW);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            message = stream.try_next() => {
                let message = match message {
                    Ok(Some(message)) => message,
                    Ok(None) => break,
                    Err(error) => return Err(DiscoveryError::Stream(error)),
                };
                let (interface, protocol, name, service_type, domain, _flags) =
                    match message.body().deserialize::<(i32, i32, String, String, String, u32)>() {
                        Ok(body) => body,
                        Err(error) => {
                            summary.warnings += 1;
                            tracing::warn!(
                                error = ?error,
                                "failed to deserialize Avahi service signal"
                            );
                            continue;
                        }
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
                    summary.services_seen += 1;
                    if service_kind(&service.service_type) == Some(ResolvedServiceKind::Printer) {
                        merge_printer_into_cache(&context, service_to_partial_entry(&service)).await;
                    }
                    let server = match AvahiServerProxy::new(&connection).await {
                        Ok(server) => server,
                        Err(error) => {
                            summary.warnings += 1;
                            tracing::warn!(
                                service_name = service.name,
                                service_type = service.service_type,
                                error = ?error,
                                "failed to create Avahi resolver proxy"
                            );
                            continue;
                        }
                    };
                    let service_name = service.name.clone();
                    let service_type = service.service_type.clone();
                    match resolve_service_entry(server, service).await {
                        Ok(ResolvedServiceEntry::Printer(printer)) => {
                            summary.printers_resolved += 1;
                            merge_printer_into_cache(&context, printer).await;
                        }
                        Ok(ResolvedServiceEntry::PrinterApplication(application)) => {
                            summary.applications_resolved += 1;
                            active_application_ids.insert(application.id.clone());
                            crate::printer_application_backend::record_discovery(
                                context.clone(),
                                application,
                            )
                            .await;
                        }
                        Err(error) => {
                            summary.warnings += 1;
                            tracing::warn!(
                                service_name,
                                service_type,
                                error = ?error,
                                "failed to resolve Avahi service"
                            );
                        }
                    }
                }
            }
        }
    }

    retain_seen_services(&context, services, active_application_ids).await;
    Ok(summary)
}

async fn merge_printer_into_cache(context: &Context, printer: PrinterEntry) {
    context
        .merge_discovered_printer_by(printer.clone(), discovered_printers_match)
        .await;
    crate::cups_backend::auto_add_discovered_printer(context.clone(), printer).await;
}

async fn retain_seen_services(
    context: &Context,
    services: HashSet<AvahiService>,
    active_application_ids: HashSet<String>,
) {
    let active_printers = services
        .into_iter()
        .filter(|service| service_kind(&service.service_type) == Some(ResolvedServiceKind::Printer))
        .map(|service| service_to_partial_entry(&service))
        .collect::<Vec<_>>();
    let active_printer_ids = active_printers
        .iter()
        .filter_map(discovered_printer_id)
        .collect::<HashSet<_>>();

    context
        .retain_discovered_printers_by(active_printers, discovered_printers_match)
        .await;
    context
        .retain_printer_applications(&active_application_ids)
        .await;
    crate::cups_backend::delete_stale_discovered_printers(active_printer_ids).await;
}

async fn resolve_service_entry(
    server: AvahiServerProxy<'_>,
    service: AvahiService,
) -> Result<ResolvedServiceEntry, ResolveError> {
    let resolved = match tokio::time::timeout(
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
    {
        Ok(Ok(resolved)) => resolved,
        Ok(Err(error)) => return Err(ResolveError::Zbus(error)),
        Err(_) => return Err(ResolveError::Timeout),
    };

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

    let service = AvahiService {
        interface,
        protocol,
        name,
        service_type,
        domain,
    };
    let txt = parse_txt_records(txt);

    match service_kind(&service.service_type)
        .ok_or_else(|| ResolveError::UnsupportedServiceType(service.service_type.clone()))?
    {
        ResolvedServiceKind::Printer => Ok(ResolvedServiceEntry::Printer(resolved_printer_entry(
            service, hostname, address, port, txt,
        ))),
        ResolvedServiceKind::PrinterApplication => Ok(ResolvedServiceEntry::PrinterApplication(
            resolved_printer_application(service, hostname, address, port, txt),
        )),
    }
}

fn resolved_printer_entry(
    service: AvahiService,
    hostname: String,
    address: String,
    port: u16,
    txt: BTreeMap<String, String>,
) -> PrinterEntry {
    let mut printer = service_to_partial_entry(&service);
    let resource_path = txt
        .get("rp")
        .cloned()
        .unwrap_or_else(|| "ipp/print".to_string());
    let device_uri = dnssd_device_uri(&service.service_type, &hostname, port, &resource_path);

    printer.set_option("device-uri", device_uri.clone());
    printer.set_option("printer-uri-supported", device_uri);
    printer.set_option("dnssd-hostname", hostname);
    printer.set_option("dnssd-address", address);
    printer.set_option("dnssd-port", port.to_string());
    printer.set_option("dnssd-resource-path", resource_path);
    printer.set_option("cosmic-discovery-detail-state", "resolved");

    if let Some(location) = txt.get("note") {
        printer.set_option("printer-location", location);
    }
    if let Some(admin_url) = txt.get("adminurl").filter(|value| !value.is_empty()) {
        printer.set_option("printer-more-info", admin_url);
    }
    for (source, destination) in [
        ("UUID", "device-uuid"),
        ("uuid", "device-uuid"),
        ("device-uuid", "device-uuid"),
        ("printer-uuid", "printer-uuid"),
    ] {
        if let Some(uuid) = txt.get(source).filter(|value| !value.is_empty()) {
            printer.set_option(destination, uuid);
        }
    }

    printer
}

fn resolved_printer_application(
    service: AvahiService,
    hostname: String,
    address: String,
    port: u16,
    txt: BTreeMap<String, String>,
) -> PrinterApplication {
    let system_uri = dnssd_device_uri(&service.service_type, &hostname, port, "ipp/system");
    let system_uuid = txt
        .get("UUID")
        .or_else(|| txt.get("uuid"))
        .filter(|value| !value.is_empty())
        .cloned();
    let make_and_model = txt.get("ty").filter(|value| !value.is_empty()).cloned();

    PrinterApplication {
        id: printer_application_id(&service.name, &service.domain, &hostname, port),
        service_name: service.name,
        service_type: service.service_type,
        domain: service.domain,
        hostname,
        port,
        addresses: vec![address],
        system_uri,
        system_uuid,
        make_and_model,
        operations_supported: Vec::new(),
        txt,
        state: PrinterApplicationState::Discovered,
    }
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

    PrinterEntry::new(String::new(), service.name.clone(), false, options)
}

fn parse_txt_records(records: Vec<Vec<u8>>) -> BTreeMap<String, String> {
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
    let service_type = printer.option("dnssd-service-type")?;
    let domain = printer.option("dnssd-domain")?;
    let name = printer.option("dnssd-service-name")?;
    Some(format!("dnssd:{service_type}:{domain}:{name}"))
}

fn service_kind(service_type: &str) -> Option<ResolvedServiceKind> {
    match service_type {
        "_ipp._tcp" | "_ipps._tcp" => Some(ResolvedServiceKind::Printer),
        "_ipp-system._tcp" | "_ipps-system._tcp" => Some(ResolvedServiceKind::PrinterApplication),
        _ => None,
    }
}

fn printer_application_id(name: &str, domain: &str, hostname: &str, port: u16) -> String {
    let normalize = |value: &str| value.trim().trim_end_matches('.').to_ascii_lowercase();
    format!(
        "dnssd-system:{}:{}:{}:{port}",
        normalize(name),
        normalize(domain),
        normalize(hostname)
    )
}

fn discovery_name(printer: &PrinterEntry) -> Option<String> {
    let name = printer
        .option("dnssd-service-name")
        .unwrap_or(printer.name())
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

    #[test]
    fn classifies_printers_and_systems_separately() {
        assert_eq!(
            service_kind("_ipp._tcp"),
            Some(ResolvedServiceKind::Printer)
        );
        assert_eq!(
            service_kind("_ipps._tcp"),
            Some(ResolvedServiceKind::Printer)
        );
        assert_eq!(
            service_kind("_ipp-system._tcp"),
            Some(ResolvedServiceKind::PrinterApplication)
        );
        assert_eq!(
            service_kind("_ipps-system._tcp"),
            Some(ResolvedServiceKind::PrinterApplication)
        );
    }

    #[test]
    fn application_identity_uses_dns_sd_name_host_and_port() {
        let first = printer_application_id("LPrint", "local.", "DESKTOP-96VEKVC-2.local.", 8000);
        let second = printer_application_id("lprint", "LOCAL", "desktop-96vekvc-2.LOCAL", 8000);
        assert_eq!(first, second);
        assert_ne!(
            first,
            printer_application_id("LPrint", "local", "desktop-96vekvc-2.local", 8001)
        );
    }
}
