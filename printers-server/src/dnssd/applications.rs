//! What an `_ipp-system._tcp` or `_ipps-system._tcp` advertisement says about a Printer
//! Application, and which of them are still being advertised.

use cosmic_settings_printers_core::{
    PrinterApplication, PrinterApplicationCapabilities, PrinterApplicationId,
    PrinterApplicationState,
};
use cups_rs::DnssdResolveEvent;
use std::collections::{BTreeMap, HashMap, HashSet};

use super::browse::ServiceKey;
use crate::state::State;

/// Builds an application identified by DNS-SD name, type, and domain rather than its mutable endpoint.
pub(super) fn resolved_application(service: DnssdResolveEvent) -> PrinterApplication {
    let txt = service.txt.into_iter().collect::<BTreeMap<_, _>>();
    let make_and_model = txt.get("ty").filter(|value| !value.is_empty()).cloned();
    let scheme = if service.service_type.starts_with("_ipps") {
        "ipps"
    } else {
        "ipp"
    };
    let system_uri = format!(
        "{scheme}://{}:{}/ipp/system",
        service.hostname, service.port
    );
    let id = PrinterApplicationId::new(&service.name, &service.service_type, &service.domain);
    // The browser page is the endpoint root, not the IPP `/ipp/system` resource.
    let web_interface_uri = (!service.hostname.trim().is_empty()).then(|| {
        let web_scheme = if scheme == "ipps" { "https" } else { "http" };
        format!(
            "{web_scheme}://{}:{}/",
            service.hostname.trim().trim_end_matches('.'),
            service.port
        )
    });

    PrinterApplication {
        id: id.as_key(),
        service_name: service.name,
        service_type: service.service_type,
        domain: service.domain,
        hostname: service.hostname,
        port: service.port,
        addresses: Vec::new(),
        system_uri,
        make_and_model,
        web_interface_uri,
        endpoints: Vec::new(),
        capabilities: PrinterApplicationCapabilities::default(),
        txt,
        state: PrinterApplicationState::Discovered,
    }
}

pub(super) fn retain_active(
    context: &State,
    runtime: &tokio::runtime::Handle,
    application_ids: &HashMap<ServiceKey, String>,
) {
    let active_ids = application_ids.values().cloned().collect::<HashSet<_>>();
    runtime.block_on(context.retain_printer_applications(&active_ids));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolution(
        name: &str,
        interface_index: u32,
        hostname: &str,
        port: u16,
        txt: Vec<(String, String)>,
    ) -> DnssdResolveEvent {
        DnssdResolveEvent {
            name: name.into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local.".into(),
            interface_index,
            full_name: format!("{name}._ipps-system._tcp.local."),
            hostname: hostname.into(),
            port,
            txt,
        }
    }

    #[test]
    fn resolved_system_builds_secure_application_uri() {
        let application = resolved_application(resolution(
            "LPrint",
            2,
            "printer.local",
            8000,
            vec![("ty".into(), "LPrint".into())],
        ));

        assert_eq!(
            application.system_uri,
            "ipps://printer.local:8000/ipp/system"
        );
        assert_eq!(application.make_and_model.as_deref(), Some("LPrint"));
    }

    #[test]
    fn two_applications_sharing_a_system_uuid_stay_separate() {
        let uuid = ("UUID".to_string(), "urn:uuid:shared".to_string());
        let first = resolved_application(resolution(
            "LPrint",
            2,
            "localhost",
            8000,
            vec![uuid.clone()],
        ));
        let second = resolved_application(resolution(
            "PostScript Printer Application",
            2,
            "localhost",
            8001,
            vec![uuid],
        ));

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn the_same_service_on_two_interfaces_is_one_application() {
        let first =
            resolved_application(resolution("LPrint", 2, "printer.local", 8000, Vec::new()));
        let second =
            resolved_application(resolution("LPrint", 3, "printer.local", 8000, Vec::new()));

        assert_eq!(first.id, second.id);
    }

    #[test]
    fn a_restart_on_a_new_port_updates_the_same_application() {
        let before =
            resolved_application(resolution("LPrint", 2, "printer.local", 8000, Vec::new()));
        let after =
            resolved_application(resolution("LPrint", 2, "desktop.local", 8631, Vec::new()));

        assert_eq!(before.id, after.id);
        assert_ne!(before.system_uri, after.system_uri);
    }

    #[test]
    fn dropping_one_interface_keeps_an_application_advertised_elsewhere() {
        let mut application_ids = HashMap::<ServiceKey, String>::new();
        let first =
            resolved_application(resolution("LPrint", 2, "printer.local", 8000, Vec::new()));
        let second =
            resolved_application(resolution("LPrint", 3, "printer.local", 8000, Vec::new()));
        let first_key = (
            2,
            "lprint".into(),
            "_ipps-system._tcp".into(),
            "local".into(),
        );
        let second_key = (
            3,
            "lprint".into(),
            "_ipps-system._tcp".into(),
            "local".into(),
        );
        application_ids.insert(first_key.clone(), first.id.clone());
        application_ids.insert(second_key, second.id);

        application_ids.remove(&first_key);

        let active = application_ids.values().cloned().collect::<HashSet<_>>();
        assert_eq!(active, HashSet::from([first.id]));
    }
}
