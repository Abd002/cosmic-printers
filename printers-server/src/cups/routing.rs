//! Who answers for a printer, and at which URI.

use cosmic_settings_printers_core::PrinterEntry;

use super::attributes::{
    PRINTER_ATTRIBUTES, reload_attrs_from_device_uri, reload_attrs_from_printer_uri,
};
use crate::error::BackendResult;
use crate::ipp::{is_local_scheduler_uri, loopback_uri, system_service_uri};
use crate::printer_app::{OwnedPrinter, reconcile};

/// The attribute CUPS reads to decide where to submit a job.
pub(super) const PRINTER_URI_SUPPORTED: &str = "printer-uri-supported";

/// Re-reads one destination's attributes from whoever can answer for it.
pub(super) fn read_printer_attrs(
    destination: &cups_rs::Destination,
    printer: &mut PrinterEntry,
    owned: &[OwnedPrinter],
) -> BackendResult<()> {
    let owner = reconcile::find_owner(printer, owned);
    if let Some(owner) = owner {
        printer.set_printer_application_id(&owner.application_id);
    }

    if printer.printer_uri().is_some_and(is_local_scheduler_uri) {
        return reload_attrs_from_printer_uri(printer, PRINTER_ATTRIBUTES);
    }

    if let Some(owner) = owner {
        apply_owning_application(printer, owner);

        return reload_attrs_from_printer_uri(printer, PRINTER_ATTRIBUTES);
    }

    match printer.device_uri() {
        Some(_) => reload_attrs_from_device_uri(destination, printer, PRINTER_ATTRIBUTES),
        None => reload_attrs_from_printer_uri(printer, PRINTER_ATTRIBUTES),
    }
}

/// Applies the owning application's exact printer URI and web page to a destination.
fn apply_owning_application(printer: &mut PrinterEntry, owner: &OwnedPrinter) {
    if let Some(printer_uri) = owner.printer.printer_uri.as_deref() {
        printer.set_option(PRINTER_URI_SUPPORTED, printer_uri);
    }
    if let Some(web_page) = owner.printer.web_interface_uri.as_deref() {
        printer.set_option("printer-more-info", web_page);
    }
    if let Some(uuid) = owner.printer.printer_uuid.as_deref() {
        printer.set_option("printer-uuid", uuid);
    }
}

/// Which service holds a destination, and so where administering it has to be sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    /// The local scheduler holds a queue for it, and is administered through `/admin`
    /// because it offers no system service of its own.
    Scheduler,
    /// The printer answers for itself. A Printer Application's printers arrive this way,
    /// and `system_uri` is the service that decides which printers it has.
    Service {
        printer_uri: String,
        system_uri: String,
    },
    /// Nothing to administer: the destination is known, but no service holds a queue for
    /// it and it named no endpoint to ask.
    Unowned,
}

/// Decides which service holds a destination.
pub(crate) fn owner_of(printer: &PrinterEntry) -> Owner {
    let Some(printer_uri) = printer.printer_uri() else {
        return Owner::Unowned;
    };

    if is_local_scheduler_uri(printer_uri) {
        return Owner::Scheduler;
    }

    // PAPPL accepts unauthenticated local administration only over loopback.
    let addressable = if printer.endpoint_is_local() {
        loopback_uri(printer_uri).unwrap_or_else(|| printer_uri.to_string())
    } else {
        printer_uri.to_string()
    };

    match system_service_uri(&addressable) {
        Some(system_uri) => Owner::Service {
            printer_uri: addressable,
            system_uri,
        },
        None => Owner::Unowned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn printer(printer_uri: Option<&str>) -> PrinterEntry {
        let mut options = HashMap::new();
        if let Some(uri) = printer_uri {
            options.insert("printer-uri-supported".to_string(), uri.to_string());
        }

        PrinterEntry::new("Acme_Laser", "Acme Laser", false, options)
    }

    #[test]
    fn a_queue_on_this_machine_belongs_to_the_scheduler() {
        assert_eq!(
            owner_of(&printer(Some("ipp://localhost/printers/Acme_Laser"))),
            Owner::Scheduler
        );
    }

    #[test]
    fn a_printer_that_answers_for_itself_names_its_own_service() {
        assert_eq!(
            owner_of(&printer(Some(
                "ipp://desktop.local:8001/ipp/print/Acme_Laser"
            ))),
            Owner::Service {
                printer_uri: "ipp://desktop.local:8001/ipp/print/Acme_Laser".to_string(),
                system_uri: "ipp://desktop.local:8001/ipp/system".to_string(),
            }
        );
    }

    #[test]
    fn the_service_keeps_the_port_the_printer_answered_on() {
        let Owner::Service { system_uri, .. } =
            owner_of(&printer(Some("ipps://desktop.local:8002/ipp/print/Other")))
        else {
            panic!("a printer answering elsewhere is not the scheduler's");
        };

        assert_eq!(system_uri, "ipps://desktop.local:8002/ipp/system");
    }

    #[test]
    fn a_destination_with_no_uri_is_owned_by_nothing() {
        assert_eq!(owner_of(&printer(None)), Owner::Unowned);
        assert_eq!(owner_of(&printer(Some("not a uri"))), Owner::Unowned);
    }

    #[test]
    fn an_unresolved_service_uri_is_owned_by_nothing() {
        assert_eq!(
            owner_of(&printer(Some("dnssd://Acme%20Laser._ipps._tcp.local/"))),
            Owner::Unowned
        );
    }

    #[test]
    fn a_service_on_this_machine_is_administered_over_loopback() {
        let mut local = printer(Some("ipps://desktop.local:8001/ipp/print/Acme_Laser"));
        local.set_option("endpoint-is-local", "true");

        assert_eq!(
            owner_of(&local),
            Owner::Service {
                printer_uri: "ipps://localhost:8001/ipp/print/Acme_Laser".to_string(),
                system_uri: "ipps://localhost:8001/ipp/system".to_string(),
            }
        );
    }

    #[test]
    fn a_service_elsewhere_keeps_the_host_it_answered_on() {
        let mut remote = printer(Some("ipps://desktop.local:8001/ipp/print/Acme_Laser"));
        remote.set_option("endpoint-is-local", "false");

        let Owner::Service { printer_uri, .. } = owner_of(&remote) else {
            panic!("a printer answering elsewhere is not the scheduler's");
        };

        assert_eq!(
            printer_uri,
            "ipps://desktop.local:8001/ipp/print/Acme_Laser"
        );
    }
}
