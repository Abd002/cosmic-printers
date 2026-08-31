//! Where a printer that advertises itself actually answers.

use cosmic_settings_printers_core::{EndpointSource, PrinterEntry};
use std::collections::HashMap;

use super::State;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DnssdDeviceEndpoint {
    pub(crate) hostname: String,
    pub(crate) port: u16,
    pub(crate) address: Option<String>,
    pub(crate) is_local: bool,
}

impl DnssdDeviceEndpoint {
    fn apply_to(&self, printer: &mut PrinterEntry) {
        printer.set_option("dnssd-hostname", &self.hostname);
        printer.set_option("dnssd-port", self.port.to_string());
        printer.set_option("endpoint-is-local", self.is_local.to_string());
        if let Some(address) = &self.address {
            printer.set_option("endpoint-address", address);
        }
    }
}

impl State {
    pub(crate) fn record_dnssd_device_endpoint(
        &self,
        service_name: String,
        endpoint: DnssdDeviceEndpoint,
    ) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        model
            .dnssd_device_endpoints
            .insert(service_name.clone(), endpoint.clone());

        let mut changed = Vec::new();
        for printer in model.available_destinations.values_mut() {
            if printer.endpoint_source() == Some(EndpointSource::Connected)
                || device_service_name(printer).as_deref() != Some(service_name.as_str())
            {
                continue;
            }
            let before = printer.clone();
            endpoint.apply_to(printer);
            if *printer != before {
                changed.push(printer.id().to_string());
            }
        }
        drop(model);

        for printer_id in changed {
            self.emit_available_destinations_changed(&printer_id);
        }
    }
}

pub(super) fn apply_resolved_device_endpoint(
    endpoints: &HashMap<String, DnssdDeviceEndpoint>,
    printer: &mut PrinterEntry,
) {
    if printer.endpoint_source() == Some(EndpointSource::Connected) {
        return;
    }
    if let Some(endpoint) = device_service_name(printer).and_then(|name| endpoints.get(&name)) {
        endpoint.apply_to(printer);
    }
}

fn device_service_name(printer: &PrinterEntry) -> Option<String> {
    let uri = url::Url::parse(printer.device_uri()?).ok()?;
    Some(
        uri.host_str()?
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(id: &str, location: &str) -> PrinterEntry {
        PrinterEntry::new(
            id,
            id,
            false,
            HashMap::from([("printer-location".to_string(), location.to_string())]),
        )
    }

    fn dnssd_destination(id: &str) -> PrinterEntry {
        let mut printer = destination(id, "");
        printer.set_option("device-uri", format!("ipps://{id}._ipps._tcp.local/"));
        printer
    }

    fn resolved_endpoint() -> DnssdDeviceEndpoint {
        DnssdDeviceEndpoint {
            hostname: "desktop.local".into(),
            port: 8000,
            address: Some("192.0.2.1".into()),
            is_local: true,
        }
    }

    #[tokio::test]
    async fn dnssd_endpoint_is_applied_when_resolution_arrives_first() {
        let context = State::new();
        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );

        context.merge_available_destination(dnssd_destination("SocketLabel"));

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), Some("192.0.2.1"));
        assert_eq!(cached[0].option("endpoint-is-local"), Some("true"));
    }

    #[tokio::test]
    async fn dnssd_endpoint_is_applied_when_destination_arrives_first() {
        let context = State::new();
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), Some("192.0.2.1"));
        assert_eq!(cached[0].option("endpoint-is-local"), Some("true"));
    }

    #[tokio::test]
    async fn later_destination_update_keeps_resolved_dnssd_endpoint() {
        let context = State::new();
        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.update_available_destination(dnssd_destination("SocketLabel"));

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
    }
}
