use cosmic_settings_printers_core::PrinterEntry;

use crate::ipp::{is_local_scheduler_uri, loopback_uri, system_service_uri};

/// Which service holds a destination, and so where administering it has to be sent.
///
/// The two eras put the same operations in different places: `Delete-Printer` exists only
/// on a system service, while `Set-Printer-Attributes` exists only on a printer. So the
/// choice is never the operation alone, it is the operation together with the resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::cups_backend) enum Owner {
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
///
/// A queue on this machine's scheduler says so in its `printer-uri-supported`, which is
/// the same test the job paths already use. Anything else answers for itself, at the URI
/// it reported.
pub(in crate::cups_backend) fn owner_of(printer: &PrinterEntry) -> Owner {
    let Some(printer_uri) = printer.printer_uri() else {
        return Owner::Unowned;
    };

    if is_local_scheduler_uri(printer_uri) {
        return Owner::Scheduler;
    }

    // A service on this machine has to be addressed over loopback to be administered at all:
    // it judges whether a request is local from the address it arrived on, and answers one
    // sent to its own advertised name `forbidden` even when it came from the same machine.
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

/// Splits a CUPS destination id into its queue name and optional instance.
pub(in crate::cups_backend) fn split_queue_instance(printer_id: &str) -> (&str, Option<&str>) {
    printer_id
        .split_once('/')
        .map_or((printer_id, None), |(name, instance)| {
            (name, Some(instance))
        })
}

/// Constructs the local scheduler URI for a queue or printer class.
pub(in crate::cups_backend) fn local_printer_uri(printer_id: &str, is_class: bool) -> String {
    let queue_name = split_queue_instance(printer_id).0;
    let path = if is_class { "classes" } else { "printers" };

    if queue_name.is_empty() {
        "ipp://localhost/".to_string()
    } else {
        format!("ipp://localhost/{path}/{queue_name}")
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

    /// A printer answering for itself is administered where it answers, and the service
    /// that decides which printers exist is at the same endpoint.
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

    /// The port matters: several applications share one host, and each has its own service.
    #[test]
    fn the_service_keeps_the_port_the_printer_answered_on() {
        let Owner::Service { system_uri, .. } =
            owner_of(&printer(Some("ipps://desktop.local:8002/ipp/print/Other")))
        else {
            panic!("a printer answering elsewhere is not the scheduler's");
        };

        assert_eq!(system_uri, "ipps://desktop.local:8002/ipp/system");
    }

    /// Nothing said where it is, so there is nothing to administer — which has to be told
    /// apart from a queue, so the caller can say so rather than send a doomed request.
    #[test]
    fn a_destination_with_no_uri_is_owned_by_nothing() {
        assert_eq!(owner_of(&printer(None)), Owner::Unowned);
        assert_eq!(owner_of(&printer(Some("not a uri"))), Owner::Unowned);
    }

    /// A DNS-SD service URI names no endpoint that can be administered.
    #[test]
    fn an_unresolved_service_uri_is_owned_by_nothing() {
        assert_eq!(
            owner_of(&printer(Some("dnssd://Acme%20Laser._ipps._tcp.local/"))),
            Owner::Unowned
        );
    }

    /// A service on this machine is addressed over loopback, because it refuses to be
    /// administered by anything that did not arrive that way — even from this machine.
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

    /// A service on another machine keeps the name it answered on: rewriting that to loopback
    /// would address this machine instead, which is a different printer or none.
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
