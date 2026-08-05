//! `Get-Printers`: what a Printer Application has already created.
//!
//! Two things need this. Before creating a printer, so the same device is not
//! configured twice. And after a request whose reply was lost, so a printer that
//! was created can be found instead of the request being retried — a blind retry
//! is how one device ends up with two queues.

use cups_rs::{IppOperation, IppTag};

use super::client::{MAX_COLLECTIONS, OperationCost, PaError, PaRequest, bounded};

/// A printer a Printer Application has already created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConfiguredPrinter {
    pub(crate) name: String,
    /// The device URI this printer drives, as the application recorded it.
    pub(crate) device_uri: Option<String>,
    pub(crate) printer_uri: Option<String>,
    pub(crate) printer_uuid: Option<String>,
    pub(crate) web_interface_uri: Option<String>,
}

/// Attributes worth asking for. Anything else would be read and discarded.
const PRINTER_ATTRIBUTES: &[&str] = &[
    "printer-name",
    "printer-uri-supported",
    "printer-uuid",
    "printer-more-info",
    "smi55357-device-uri",
];

/// Lists the printers a Printer Application has created.
pub(crate) fn get_printers(system_uri: &str) -> Result<Vec<ConfiguredPrinter>, PaError> {
    let response = PaRequest::new(IppOperation::GetPrinters, system_uri)?
        .keywords("requested-attributes", PRINTER_ATTRIBUTES)?
        .send(system_uri, OperationCost::Query)?;

    let mut printers = Vec::new();
    let mut current = PartialPrinter::default();

    // One response carries every printer, with the attributes of each in its own
    // Printer group. libcups separates the groups with a nameless zero-group
    // attribute, which is the only reliable boundary: relying on seeing
    // printer-name first would depend on the sender's attribute order, and IPP
    // does not fix one.
    for attribute in response.attributes() {
        let Some(name) = attribute.name() else {
            printers.extend(current.finish());
            current = PartialPrinter::default();
            continue;
        };

        if attribute.group_tag() != Some(IppTag::Printer) {
            continue;
        }

        let value = attribute.get_string(0).map(bounded);
        let Some(value) = value
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        match name.as_str() {
            "printer-name" => current.name = Some(value),
            "smi55357-device-uri" => current.device_uri = Some(value),
            "printer-uri-supported" => current.printer_uri = Some(value),
            "printer-uuid" => current.printer_uuid = Some(value),
            "printer-more-info" => current.web_interface_uri = Some(value),
            _ => {}
        }

        if printers.len() >= MAX_COLLECTIONS {
            break;
        }
    }
    printers.extend(current.finish());

    Ok(printers)
}

#[derive(Default)]
struct PartialPrinter {
    name: Option<String>,
    device_uri: Option<String>,
    printer_uri: Option<String>,
    printer_uuid: Option<String>,
    web_interface_uri: Option<String>,
}

impl PartialPrinter {
    /// Yields a printer only when it has a name, which is what identifies it in
    /// every later operation.
    fn finish(self) -> Option<ConfiguredPrinter> {
        Some(ConfiguredPrinter {
            name: self.name?,
            device_uri: self.device_uri,
            printer_uri: self.printer_uri,
            printer_uuid: self.printer_uuid,
            web_interface_uri: self.web_interface_uri,
        })
    }
}

/// Looks for a printer this application already has for a device.
///
/// Matching is by the exact device URI the application itself recorded. That is
/// the only comparison that means anything here: it is this application's own
/// spelling of the route to the device, so an exact match is the same device
/// configured through the same software.
pub(crate) fn find_by_device_uri<'a>(
    printers: &'a [ConfiguredPrinter],
    device_uri: &str,
) -> Option<&'a ConfiguredPrinter> {
    printers
        .iter()
        .find(|printer| printer.device_uri.as_deref() == Some(device_uri))
}

/// Looks for a printer by the name that was requested.
pub(crate) fn find_by_name<'a>(
    printers: &'a [ConfiguredPrinter],
    name: &str,
) -> Option<&'a ConfiguredPrinter> {
    printers
        .iter()
        .find(|printer| printer.name.eq_ignore_ascii_case(name))
}

// There is deliberately no lookup by UUID here. Duplicate detection happens
// before a printer exists, so there is no UUID to compare against yet; matching a
// created printer by UUID is reconciliation's job, against destinations, and lives
// in `super::reconcile`.

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(name: &str, device_uri: Option<&str>, uuid: Option<&str>) -> ConfiguredPrinter {
        ConfiguredPrinter {
            name: name.to_string(),
            device_uri: device_uri.map(ToString::to_string),
            printer_uri: None,
            printer_uuid: uuid.map(ToString::to_string),
            web_interface_uri: None,
        }
    }

    #[test]
    fn finds_an_existing_printer_by_its_exact_device_uri() {
        let printers = vec![
            printer("Other", Some("socket://192.0.2.11:9100"), None),
            printer("Acme", Some("socket://192.0.2.10:9100"), None),
        ];

        assert_eq!(
            find_by_device_uri(&printers, "socket://192.0.2.10:9100")
                .map(|printer| printer.name.as_str()),
            Some("Acme")
        );
        // A different spelling is not a match: the application recorded the URI
        // it was given, so only that exact value proves the same configuration.
        assert!(find_by_device_uri(&printers, "socket://192.0.2.10").is_none());
    }

    #[test]
    fn finds_a_printer_by_name_ignoring_case() {
        let printers = vec![printer("Acme_Test_Laser", None, None)];

        assert!(find_by_name(&printers, "acme_test_laser").is_some());
        assert!(find_by_name(&printers, "Acme_Test_Laser_2").is_none());
    }

    #[test]
    fn a_record_without_a_name_is_not_a_printer() {
        let partial = PartialPrinter {
            name: None,
            device_uri: Some("socket://192.0.2.10:9100".into()),
            ..PartialPrinter::default()
        };

        assert_eq!(partial.finish(), None);
    }
}
