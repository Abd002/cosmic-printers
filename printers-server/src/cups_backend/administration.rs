//! Administering a printer: what it is called, where it is, and whether it takes work.
//!
//! Sent as IPP to whichever service holds the printer, chosen by [`owner_of`]. There is no
//! privileged helper involved and no password to collect: over the local domain socket the
//! scheduler authenticates the caller from the peer credentials, so a user who may
//! administer printers needs no credential, and one who may not cannot be helped by one —
//! the refusal is a group decision about an already-known identity, and arrives as
//! `forbidden` rather than as a request to authenticate.
//!
//! Most of these are the standard operations, which both the local scheduler and a Printer
//! Application answer. Two are not, and are marked where they are used: the scheduler
//! implements neither `Delete-Printer` nor a system service, and no standard attribute
//! carries sharing at all.

use crate::ipp::IppTimeouts;
use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::{IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

use super::helpers::{
    CupsResultExt, Owner, add_requesting_user, ensure_success, local_printer_uri, owner_of,
    send_ipp_request_with_timeouts,
};
use crate::error::{BackendError, BackendResult};

/// Where the scheduler takes the operations that predate the system service.
const SCHEDULER_ADMIN_URI: &str = "ipp://localhost/admin/";

/// How long to allow for reaching the service that holds a printer.
///
/// The quarter of a second a lookup is given is far too little here. An administrative request
/// goes to the printer's own endpoint, whose host is usually a `.local` name that has to be
/// resolved over mDNS, and then to a TLS handshake — a printer that answers perfectly well is
/// otherwise reported as never having replied. Matches what asking a device for its attributes
/// already allows itself.
const ADMIN_TIMEOUTS: IppTimeouts = IppTimeouts {
    connect_ms: 5000,
    response_seconds: 30.0,
};

/// Where the scheduler names the groups it lets administer printers.
const CUPS_FILES_CONF: &str = "/etc/cups/cups-files.conf";

/// The groups to assume when the scheduler's configuration cannot be read.
///
/// Which of these a build actually uses is fixed when CUPS is compiled and is not
/// published anywhere; these are the names in circulation, and being generous here only
/// means offering an action the scheduler may still refuse.
const LIKELY_ADMIN_GROUPS: &[&str] = &["lpadmin", "root", "sys", "system", "wheel"];

/// Returns whether this user is in a group the scheduler administers by.
///
/// Read rather than asked. The scheduler authenticates a request over the local socket
/// from the caller's own credentials and only then tests group membership, so a user
/// outside the group is answered `forbidden` — not challenged. There is no credential that
/// would change it and so nothing worth prompting for, which is why this is settled before
/// anything is offered rather than after something fails.
///
/// Where the configuration is unreadable — it is not world-readable on every
/// distribution — this answers generously, so the action stays offered and the scheduler's
/// own refusal decides. Hiding something that would have worked is the worse mistake.
pub(super) fn user_may_administer() -> bool {
    // Root is in every group that matters.
    if nix::unistd::geteuid().is_root() {
        return true;
    }

    let configured = system_groups();
    let names = match &configured {
        Some(groups) => groups.iter().map(String::as_str).collect::<Vec<_>>(),
        None => LIKELY_ADMIN_GROUPS.to_vec(),
    };

    let Ok(mut held) = nix::unistd::getgroups() else {
        // Without the group list there is nothing to decide on, so do not stand in the way.
        return true;
    };
    held.push(nix::unistd::getegid());

    names.iter().any(|name| {
        matches!(
            nix::unistd::Group::from_name(name),
            Ok(Some(group)) if held.contains(&group.gid)
        )
    })
}

/// Returns the groups `SystemGroup` names, if the file can be read and says.
fn system_groups() -> Option<Vec<String>> {
    let contents = std::fs::read_to_string(CUPS_FILES_CONF).ok()?;

    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let (directive, groups) = line.split_once(char::is_whitespace)?;
            directive.eq_ignore_ascii_case("SystemGroup").then(|| {
                groups
                    .split_whitespace()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
        })
        .filter(|groups| !groups.is_empty())
}

/// Sets where the printer says it is.
pub(super) async fn set_location(printer: PrinterEntry, location: String) -> BackendResult<()> {
    set_printer_text(printer, "printer-location", location).await
}

/// Sets what the printer says about itself.
pub(super) async fn set_info(printer: PrinterEntry, info: String) -> BackendResult<()> {
    set_printer_text(printer, "printer-info", info).await
}

/// Starts or stops the printer, which is what `cupsenable` and `cupsdisable` do.
///
/// Not the same as whether it accepts jobs: a stopped printer still takes them and holds
/// them until it starts again.
pub(super) async fn set_enabled(printer: PrinterEntry, enabled: bool) -> BackendResult<()> {
    let operation = if enabled {
        IppOperation::ResumePrinter
    } else {
        IppOperation::PausePrinter
    };
    let name = if enabled {
        "Resume-Printer"
    } else {
        "Pause-Printer"
    };

    let target = printer_target(&printer)?;

    send(operation, name, target.clone(), target, |_| Ok(())).await
}

/// Makes the printer accept or refuse new jobs, which is what `accept` and `reject` do.
///
/// IPP names these the other way round from CUPS: refusing work is `Disable-Printer`.
pub(super) async fn set_accept_jobs(
    printer: PrinterEntry,
    enabled: bool,
    reason: String,
) -> BackendResult<()> {
    let operation = if enabled {
        IppOperation::EnablePrinter
    } else {
        IppOperation::DisablePrinter
    };
    let name = if enabled {
        "Enable-Printer"
    } else {
        "Disable-Printer"
    };

    let target = printer_target(&printer)?;

    send(operation, name, target.clone(), target, move |request| {
        // Only the scheduler keeps a reason, and only for a refusal. Anywhere else it is
        // ignored, which is why it is offered rather than required.
        if reason.is_empty() || enabled {
            return Ok(());
        }

        request
            .add_string(
                IppTag::Printer,
                IppValueTag::Text,
                "printer-state-message",
                &reason,
            )
            .cups_err()
    })
    .await
}

/// Removes the printer.
pub(super) async fn delete_printer(printer: PrinterEntry) -> BackendResult<()> {
    match owner_of(&printer) {
        // The scheduler implements no `Delete-Printer`, so removing one of its queues is
        // still its own operation.
        Owner::Scheduler => {
            send(
                IppOperation::CupsDeletePrinter,
                "CUPS-Delete-Printer",
                SCHEDULER_ADMIN_URI.to_string(),
                local_printer_uri(printer.id(), false),
                |_| Ok(()),
            )
            .await
        }
        // Removing a printer an application holds means asking that application's system
        // service, which identifies its printers by `printer-id` rather than by URI. That
        // lookup is not written yet, and guessing at a destructive operation is worse than
        // saying so.
        Owner::Service { .. } => Err(BackendError::OperationNotSupported {
            operation: "remove a printer held by a printer application".to_string(),
        }),
        Owner::Unowned => Err(BackendError::NoQueueToAdminister {
            printer: printer.id().to_string(),
        }),
    }
}

/// Sets one text attribute on the printer itself.
async fn set_printer_text(
    printer: PrinterEntry,
    attribute: &'static str,
    value: String,
) -> BackendResult<()> {
    let owner = owner_of(&printer);
    let target = printer_target(&printer)?;

    // Ask first, because the answer cannot be trusted afterwards. A Printer Application replies
    // `successful-ok` — "Printer attributes set." — to a request naming an attribute it will not
    // change, and leaves the value exactly as it was. Reporting that as success would be a lie
    // the caller has no way to catch, so a service that has published what it will accept is
    // taken at its word.
    //
    // The scheduler is exempt: the five attributes it lists are only the ones it takes *this*
    // way, and the fallback below reaches the others.
    if !matches!(owner, Owner::Scheduler) && !settable_on(&printer, attribute) {
        return Err(BackendError::OperationNotSupported {
            operation: format!("change '{attribute}', which this printer does not allow"),
        });
    }

    let attributes = {
        let value = value.clone();
        move |request: &mut IppRequest| {
            request
                .add_string(IppTag::Printer, IppValueTag::Text, attribute, &value)
                .cups_err()
        }
    };

    let result = send(
        IppOperation::SetPrinterAttributes,
        "Set-Printer-Attributes",
        target,
        printer_target(&printer)?,
        attributes,
    )
    .await;

    let Err(error) = result else {
        return Ok(());
    };

    // A scheduler that will not take this attribute the standard way may still take it
    // through the operation that adds a printer as well as changes one. Anywhere else
    // there is nothing further to try, so the refusal stands.
    if !matches!(owner, Owner::Scheduler) || !worth_retrying_as_add_modify(&error) {
        return Err(error);
    }

    send(
        IppOperation::CupsAddModifyPrinter,
        "CUPS-Add-Modify-Printer",
        SCHEDULER_ADMIN_URI.to_string(),
        local_printer_uri(printer.id(), false),
        move |request| {
            request
                .add_string(IppTag::Printer, IppValueTag::Text, attribute, &value)
                .cups_err()
        },
    )
    .await
}

/// Returns whether any administrative operation could reach this destination.
///
/// Narrower than "something answers for it", because in the current model most destinations
/// have nothing to administer. A printer reached over DNS-SD has no permanent queue: CUPS makes
/// one on demand and reaps it when it goes idle, so there is nothing to stop or start, and a
/// location or description written to it would be discarded with the queue. Administration in
/// the sense CUPS 2.x meant it belongs to a server that keeps its queues — the local scheduler
/// here, or a sharing server — and asking anything else only earns a refusal, or worse a
/// success that changed nothing.
///
/// So: a queue the scheduler holds, or a service that has published what it will let us change.
/// Anything else is left alone.
pub(super) fn can_be_administered(printer: &PrinterEntry) -> bool {
    if !user_may_administer() {
        return false;
    }

    match owner_of(printer) {
        Owner::Scheduler => true,
        Owner::Service { .. } => !published_settable(printer).is_empty(),
        Owner::Unowned => false,
    }
}

/// The attributes a service says it will let us change, under either spelling.
///
/// A Printer Application says `printer-settable-attributes`; the scheduler spells the same thing
/// with `-supported`.
fn published_settable(printer: &PrinterEntry) -> Vec<String> {
    [
        "printer-settable-attributes",
        "printer-settable-attributes-supported",
    ]
    .into_iter()
    .flat_map(|name| printer.option_values(name))
    .collect()
}

/// Returns whether this service said it will let the attribute be changed.
///
/// True when it has published no such list at all: silence is not a refusal, and withholding
/// an operation a printer might well accept is the worse mistake.
fn settable_on(printer: &PrinterEntry, attribute: &str) -> bool {
    // A Printer Application says `printer-settable-attributes`; the scheduler spells the same
    // thing with `-supported`. Either counts.
    let published = published_settable(printer);

    published.is_empty() || published.iter().any(|listed| listed.trim() == attribute)
}

/// Returns whether the standard operation failed in a way the scheduler's own might not.
///
/// Being told the attribute is not settable, or that the operation is unknown, says the
/// request was allowed and only this route is closed. A refusal says the opposite, and
/// retrying it would only ask to be refused again.
fn worth_retrying_as_add_modify(error: &BackendError) -> bool {
    matches!(
        error,
        BackendError::IppStatus { status, .. }
            if status == &format!("{:?}", IppStatus::ErrorAttributesNotSettable)
                || status == &format!("{:?}", IppStatus::ErrorOperationNotSupported)
    )
}

/// Returns the URI naming the printer an operation is about.
fn printer_target(printer: &PrinterEntry) -> BackendResult<String> {
    match owner_of(printer) {
        Owner::Scheduler => Ok(local_printer_uri(printer.id(), false)),
        Owner::Service { printer_uri, .. } => Ok(printer_uri),
        Owner::Unowned => Err(BackendError::NoQueueToAdminister {
            printer: printer.id().to_string(),
        }),
    }
}

/// Sends one administration request.
///
/// `target` is where the request goes and `printer_uri` is what it is about; the two
/// differ for the scheduler's own operations, which are addressed to `/admin` while naming
/// a queue.
async fn send(
    operation: IppOperation,
    operation_name: &'static str,
    target: String,
    printer_uri: String,
    attributes: impl FnOnce(&mut IppRequest) -> BackendResult<()> + Send + 'static,
) -> BackendResult<()> {
    tokio::task::spawn_blocking(move || {
        // `IppRequest::new` already opens the request with `attributes-charset` and
        // `attributes-natural-language`, so nothing may add them again. A second pair is a
        // protocol violation that the scheduler answers `successful-ok` to and then ignores
        // every printer-group attribute of — a change that silently does nothing. Verified
        // against cupsd 2.4.7: the same request applies with one pair and is dropped with two.
        let mut request = IppRequest::new(operation).cups_err()?;

        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                "printer-uri",
                &printer_uri,
            )
            .cups_err()?;
        add_requesting_user(&mut request)?;
        attributes(&mut request)?;

        let response = send_ipp_request_with_timeouts(request, &target, ADMIN_TIMEOUTS)?;

        ensure_success(&response, operation_name)
    })
    .await
    .map_err(BackendError::Join)?
}
