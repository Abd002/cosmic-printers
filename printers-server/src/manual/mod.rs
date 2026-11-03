//! Adding a printer manually: asking the address the user typed what answers there.

mod applications;
mod connection;
mod inspect;
mod target;

use cosmic_settings_printers_core::{
    ConfigurePrinterAtAddressRequest, ConfigurePrinterReply, Error, PrinterAtAddress,
    PrintersAtAddressReply,
};
use std::net::Ipv6Addr;

use crate::printer_app::{self, ManualPrinter};
use crate::state::State;
use inspect::{EndpointKind, EndpointReport, FoundPrinter};

/// Formats a printer takes when it needs no driver: PWG Raster, Apple Raster and PDF.
const DRIVERLESS_FORMATS: &[&str] = &["image/pwg-raster", "image/urf", "application/pdf"];

/// Asks an address what answers there.
/// Lists the driverless printers that answered, and the Printer Applications that can reach it.
pub(crate) async fn printers_at_address(
    context: &State,
    host: String,
    port: Option<u16>,
) -> Result<PrintersAtAddressReply, Error> {
    let report = probe(address(&host, port)).await?;
    let printer_applications = applications::reaching(context, &report).await;

    Ok(PrintersAtAddressReply {
        printers: report
            .printers
            .iter()
            .filter(|printer| is_driverless(printer))
            .map(printer_at_address)
            .collect(),
        printer_applications,
    })
}

/// Creates a printer at a typed address through the Printer Application chosen.
/// The address is asked again, so the device URI comes from this probe and never from the client.
pub(crate) async fn configure_printer_at_address(
    context: &State,
    request: ConfigurePrinterAtAddressRequest,
) -> Result<ConfigurePrinterReply, Error> {
    let input = address(&request.host, request.port);
    let report = probe(input.clone()).await?;

    let application = context
        .printer_applications_cached()
        .await
        .into_iter()
        .find(|application| application.id == request.printer_application_id)
        .ok_or_else(|| Error::PrinterApplicationNotFound {
            application_id: request.printer_application_id.clone(),
        })?;

    let Some(connection) = applications::connection(&application, &report) else {
        return Err(Error::PrinterConfigurationManualActionRequired {
            application_id: application.id.clone(),
            web_interface_uri: printer_app::manual_setup_application(&application)
                .map(|application| application.web_interface_uri),
            why: "this printer application cannot create a printer at this address".to_string(),
        });
    };

    printer_app::configure_printer_manually(
        context,
        &application,
        ManualPrinter {
            address: input,
            device_uri: connection.device_uri,
            device_id: connection.device_id,
            name: connection.name,
        },
    )
    .await
}

/// Asks the address what answers there.
async fn probe(input: String) -> Result<EndpointReport, Error> {
    let probed = input.clone();
    let report = tokio::task::spawn_blocking(move || inspect::inspect(&probed))
        .await
        .map_err(|error| Error::Internal {
            why: error.to_string(),
        })??;

    if report.kind == EndpointKind::Silent {
        return Err(Error::DeviceUnreachable {
            why: format!("nothing answers at {input}"),
        });
    }

    Ok(report)
}

/// Joins the typed host and port; an IPv6 literal needs brackets to carry a port.
fn address(host: &str, port: Option<u16>) -> String {
    let host = host.trim();
    let host = if host.parse::<Ipv6Addr>().is_ok() {
        format!("[{host}]")
    } else {
        host.to_string()
    };

    match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    }
}

/// A printer that says it is IPP Everywhere, or takes a format any client can send it.
fn is_driverless(printer: &FoundPrinter) -> bool {
    printer
        .ipp_features
        .iter()
        .any(|feature| feature == "ipp-everywhere")
        || printer
            .formats
            .iter()
            .any(|format| DRIVERLESS_FORMATS.contains(&format.as_str()))
}

fn printer_at_address(printer: &FoundPrinter) -> PrinterAtAddress {
    let display_name = [
        printer.info.as_deref(),
        printer.make_and_model.as_deref(),
        Some(printer.name.as_str()),
    ]
    .into_iter()
    .flatten()
    .find(|name| !name.trim().is_empty())
    .unwrap_or(printer.uri.as_str())
    .to_string();

    PrinterAtAddress {
        id: printer.uri.clone(),
        display_name,
        make_and_model: printer.make_and_model.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(formats: &[&str]) -> FoundPrinter {
        FoundPrinter {
            name: "office".into(),
            uri: "ipp://192.0.2.10:631/ipp/print".into(),
            uris: Vec::new(),
            make_and_model: Some("HP LaserJet Pro M404n".into()),
            info: None,
            device_id: None,
            formats: formats.iter().map(ToString::to_string).collect(),
            ipp_features: Vec::new(),
        }
    }

    #[test]
    fn a_printer_taking_pwg_raster_is_driverless() {
        assert!(is_driverless(&found(&[
            "application/octet-stream",
            "image/pwg-raster"
        ])));
        assert!(is_driverless(&found(&["image/urf"])));
    }

    #[test]
    fn a_printer_saying_it_is_ipp_everywhere_is_driverless() {
        let mut printer = found(&["application/vnd.hp-PCL"]);
        printer.ipp_features = vec!["airprint-1.8".into(), "ipp-everywhere".into()];

        assert!(is_driverless(&printer));
    }

    #[test]
    fn a_printer_taking_only_its_own_language_needs_a_driver() {
        assert!(!is_driverless(&found(&["application/vnd.hp-PCL"])));
        assert!(!is_driverless(&found(&[])));
    }

    #[test]
    fn a_printer_is_named_by_its_model_when_it_has_no_description() {
        let printer = printer_at_address(&found(&["image/urf"]));

        assert_eq!(printer.display_name, "HP LaserJet Pro M404n");
        assert_eq!(printer.id, "ipp://192.0.2.10:631/ipp/print");
    }

    #[test]
    fn an_ipv6_literal_is_bracketed_before_its_port() {
        assert_eq!(address("fe80::1", Some(631)), "[fe80::1]:631");
        assert_eq!(address(" printer.local ", None), "printer.local");
        assert_eq!(address("192.0.2.10", Some(9100)), "192.0.2.10:9100");
    }
}
