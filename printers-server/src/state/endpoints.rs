//! Where a printer that advertises itself actually answers.

use cosmic_settings_printers_core::PrinterEntry;
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
    /// Returns whether a cached printer answers for the service.
    pub(crate) fn record_dnssd_device_endpoint(
        &self,
        service_name: String,
        endpoint: DnssdDeviceEndpoint,
    ) -> bool {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        model
            .dnssd_device_endpoints
            .insert(service_name.clone(), endpoint.clone());

        let mut changed = Vec::new();
        let mut found_compatible = false;

        for printer in model.available_destinations.values_mut() {
            if device_service_name(printer).as_deref() != Some(service_name.as_str()) {
                continue;
            }
            let before = printer.clone();
            endpoint.apply_to(printer);
            if *printer != before {
                changed.push(printer.id().to_string());
            }
            found_compatible = true;
        }
        drop(model);

        for printer_id in changed {
            self.emit_available_destinations_changed(&printer_id);
        }

        found_compatible
    }

    /// Forgets where a service answered once it stops advertising itself.
    /// Printers keep what was already applied: a queue outlives the advertisement, and the
    /// next resolution replaces the entry anyway.
    pub(crate) fn remove_dnssd_device_endpoint(&self, service_name: &str) {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        model.dnssd_device_endpoints.remove(service_name);

        // The advertisement going away is one miss, so the next enumeration that
        // misses the printer too drops it. A queue is still enumerated and keeps it.
        let gone = model
            .available_destinations
            .values()
            .filter(|printer| device_service_name(printer).as_deref() == Some(service_name))
            .map(|printer| printer.id().to_string())
            .collect::<Vec<_>>();
        for id in gone {
            let misses = model.enumeration_misses.entry(id).or_default();
            *misses = misses.saturating_add(1);
        }
    }
}

pub(super) fn apply_resolved_device_endpoint(
    endpoints: &HashMap<String, DnssdDeviceEndpoint>,
    printer: &mut PrinterEntry,
) {
    if let Some(endpoint) = device_service_name(printer).and_then(|name| endpoints.get(&name)) {
        endpoint.apply_to(printer);
    }
}

fn device_service_name(printer: &PrinterEntry) -> Option<String> {
    let uri = url::Url::parse(printer.device_uri()?).ok()?;
    // A space in the instance name arrives as %20.
    let host = percent_encoding::percent_decode_str(uri.host_str()?).decode_utf8_lossy();
    Some(host.trim().trim_end_matches('.').to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic_settings_printers_core::EndpointSource;
    use std::collections::HashSet;

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

    fn through_a_local_queue(id: &str) -> PrinterEntry {
        let mut printer = dnssd_destination(id);
        printer.set_option("endpoint-hostname", "localhost");
        printer.set_option("endpoint-port", "631");
        printer.set_option("endpoint-address", "127.0.0.1");
        printer.set_option("endpoint-is-local", "true");
        printer.set_endpoint_source(EndpointSource::Connected);
        printer
    }

    #[tokio::test]
    async fn the_advertisement_replaces_what_a_temporary_queue_reported_first() {
        let context = State::new();
        context.update_available_destination(through_a_local_queue("SocketLabel"));

        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            DnssdDeviceEndpoint {
                is_local: false,
                ..resolved_endpoint()
            },
        );

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), Some("192.0.2.1"));
        assert_eq!(cached[0].option("endpoint-is-local"), Some("false"));
    }

    #[tokio::test]
    async fn a_temporary_queue_read_later_does_not_take_the_endpoint_back() {
        let context = State::new();
        context.record_dnssd_device_endpoint(
            "socketlabel._ipps._tcp.local".into(),
            resolved_endpoint(),
        );
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.update_available_destination(through_a_local_queue("SocketLabel"));

        let cached = context.available_destinations_cached().await;
        assert_eq!(cached[0].hostname(), Some("desktop.local"));
        assert_eq!(cached[0].port(), Some(8000));
        assert_eq!(cached[0].endpoint_address(), Some("192.0.2.1"));
    }

    #[tokio::test]
    async fn a_name_with_spaces_matches_its_percent_encoded_device_uri() {
        let context = State::new();
        let mut printer = destination("Office_Laser", "");
        printer.set_option("device-uri", "dnssd://Office%20Laser._ipp._tcp.local/");
        context.merge_available_destination(printer);

        let known = context.record_dnssd_device_endpoint(
            "office laser._ipp._tcp.local".into(),
            resolved_endpoint(),
        );

        assert!(known);
        assert_eq!(
            context.available_destinations_cached().await[0].port(),
            Some(8000)
        );
    }

    #[tokio::test]
    async fn a_printer_whose_advertisement_went_away_is_dropped_on_the_next_miss() {
        let context = State::new();
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.remove_dnssd_device_endpoint("socketlabel._ipps._tcp.local");
        context.retain_available_destinations(&HashSet::new());

        assert!(context.available_destinations_cached().await.is_empty());
    }

    #[tokio::test]
    async fn a_printer_still_enumerated_outlives_its_advertisement() {
        let context = State::new();
        context.merge_available_destination(dnssd_destination("SocketLabel"));

        context.remove_dnssd_device_endpoint("socketlabel._ipps._tcp.local");
        context.retain_available_destinations(&HashSet::from(["SocketLabel".to_string()]));
        context.retain_available_destinations(&HashSet::new());

        assert_eq!(context.available_destinations_cached().await.len(), 1);
    }
}
