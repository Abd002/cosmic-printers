mod system;

use cosmic_settings_printers_core::{PrinterApplication, PrinterApplicationState};

use crate::context::Context;

pub(crate) fn spawn_system_probe(context: Context, application: PrinterApplication) {
    tokio::spawn(async move {
        let application_id = application.id.clone();
        let result = system::get_system_attributes(application.system_uri).await;
        apply_probe_result(&context, &application_id, result).await;
    });
}

async fn apply_probe_result(
    context: &Context,
    application_id: &str,
    result: Result<system::SystemProbe, system::ProbeError>,
) {
    let state;
    let mut probe = None;

    match result {
        Ok(result) => {
            state = if result.operations_supported.contains(&0x402b) {
                PrinterApplicationState::Ready
            } else {
                PrinterApplicationState::Unsupported
            };
            probe = Some(result);
        }
        Err(system::ProbeError::AuthenticationRequired) => {
            state = PrinterApplicationState::AuthenticationRequired;
        }
        Err(system::ProbeError::Unreachable) => {
            state = PrinterApplicationState::Unreachable;
        }
        Err(system::ProbeError::Failed) => {
            state = PrinterApplicationState::Failed;
        }
    }

    context
        .update_printer_application(application_id, move |application| {
            if let Some(probe) = probe {
                application.system_uuid = probe.system_uuid;
                application.make_and_model = probe.make_and_model;
                application.operations_supported = probe.operations_supported;
            }
            application.state = state;
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn application() -> PrinterApplication {
        PrinterApplication {
            id: "app".into(),
            service_name: "LPrint".into(),
            service_type: "_ipps-system._tcp".into(),
            domain: "local".into(),
            hostname: "printer.local".into(),
            port: 8000,
            addresses: vec!["192.0.2.1".into()],
            system_uri: "ipps://printer.local:8000/ipp/system".into(),
            system_uuid: None,
            make_and_model: None,
            operations_supported: Vec::new(),
            txt: BTreeMap::new(),
            state: PrinterApplicationState::Discovered,
        }
    }

    #[tokio::test]
    async fn find_devices_support_marks_application_ready() {
        let context = Context::new().await;
        context.upsert_printer_application(application()).await;
        apply_probe_result(
            &context,
            "app",
            Ok(system::SystemProbe {
                system_uuid: Some("urn:uuid:test".into()),
                make_and_model: Some("Example Application".into()),
                operations_supported: vec![0x000b, 0x402b],
            }),
        )
        .await;

        let applications = context.list_printer_applications().await;
        assert_eq!(applications[0].state, PrinterApplicationState::Ready);
        assert_eq!(
            applications[0].system_uuid.as_deref(),
            Some("urn:uuid:test")
        );
    }

    #[tokio::test]
    async fn missing_find_devices_support_marks_application_unsupported() {
        let context = Context::new().await;
        context.upsert_printer_application(application()).await;
        apply_probe_result(
            &context,
            "app",
            Ok(system::SystemProbe {
                system_uuid: None,
                make_and_model: None,
                operations_supported: vec![0x000b, 0x003a],
            }),
        )
        .await;

        let applications = context.list_printer_applications().await;
        assert_eq!(applications[0].state, PrinterApplicationState::Unsupported);
    }

    #[tokio::test]
    async fn maps_probe_failures_without_removing_application() {
        for (error, expected) in [
            (
                system::ProbeError::AuthenticationRequired,
                PrinterApplicationState::AuthenticationRequired,
            ),
            (
                system::ProbeError::Unreachable,
                PrinterApplicationState::Unreachable,
            ),
            (system::ProbeError::Failed, PrinterApplicationState::Failed),
        ] {
            let context = Context::new().await;
            context.upsert_printer_application(application()).await;
            apply_probe_result(&context, "app", Err(error)).await;

            let applications = context.list_printer_applications().await;
            assert_eq!(applications.len(), 1);
            assert_eq!(applications[0].state, expected);
        }
    }
}
