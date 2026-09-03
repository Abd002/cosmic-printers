//! Administering a printer: what it is called, where it is, and whether it takes work.
//! Requests go directly to the owning IPP service; the local scheduler authenticates peer credentials.

use crate::ipp::IppTimeouts;
use cosmic_settings_printers_core::PrinterEntry;
use cups_rs::{IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

use super::routing::{Owner, owner_of};
use super::scheduler::{self, SCHEDULER_ADMIN_URI, local_printer_uri};
use crate::error::{BackendError, BackendResult};
use crate::ipp::{CupsResultExt, add_requesting_user, ensure_success};

/// Allows mDNS resolution and TLS setup when reaching a printer service.
const ADMIN_TIMEOUTS: IppTimeouts = IppTimeouts {
    connect_ms: 5000,
    response_seconds: 30.0,
};

/// Where the scheduler names the groups it lets administer printers.
const CUPS_FILES_CONF: &str = "/etc/cups/cups-files.conf";

/// Checks the local group membership used by scheduler peer-credential authorization.
pub(super) fn user_may_administer() -> bool {
    // Root is in every group that matters.
    if nix::unistd::geteuid().is_root() {
        return true;
    }

    let configured = system_groups();

    let names: Vec<&str> = match &configured {
        Some(groups) => groups.iter().map(String::as_str).collect(),
        None => Vec::new(),
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
pub(crate) async fn set_location(printer: PrinterEntry, location: String) -> BackendResult<()> {
    set_printer_text(printer, "printer-location", location).await
}

/// Sets what the printer says about itself.
pub(crate) async fn set_info(printer: PrinterEntry, info: String) -> BackendResult<()> {
    set_printer_text(printer, "printer-info", info).await
}

/// Starts or stops the printer, which is what `cupsenable` and `cupsdisable` do.
pub(crate) async fn set_enabled(printer: PrinterEntry, enabled: bool) -> BackendResult<()> {
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
pub(crate) async fn set_accept_jobs(
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

/// Removes a queue held by the local scheduler.
pub(crate) async fn delete_scheduler_printer(printer: PrinterEntry) -> BackendResult<()> {
    send(
        IppOperation::CupsDeletePrinter,
        "CUPS-Delete-Printer",
        SCHEDULER_ADMIN_URI.to_string(),
        local_printer_uri(printer.id(), false),
        |_| Ok(()),
    )
    .await
}

/// Sets one text attribute on the printer itself.
async fn set_printer_text(
    printer: PrinterEntry,
    attribute: &'static str,
    value: String,
) -> BackendResult<()> {
    let owner = owner_of(&printer);
    let target = printer_target(&printer)?;

    // PAPPL may report success while ignoring unsupported attributes, so check support first.
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

    // The scheduler alone supports the CUPS add/modify fallback.
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

/// Returns whether the destination has a persistent administrable queue or application printer.
pub(super) fn can_be_administered(printer: &PrinterEntry, user_may_administer: bool) -> bool {
    if !user_may_administer {
        return false;
    }

    match owner_of(printer) {
        Owner::Scheduler => true,
        Owner::Service { .. } => !published_settable(printer).is_empty(),
        Owner::Unowned => false,
    }
}

/// The attributes a service says it will let us change, under either spelling.
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
fn settable_on(printer: &PrinterEntry, attribute: &str) -> bool {
    // A Printer Application says `printer-settable-attributes`; the scheduler spells the same
    // thing with `-supported`. Either counts.
    let published = published_settable(printer);

    published.is_empty() || published.iter().any(|listed| listed.trim() == attribute)
}

/// Returns whether the standard operation failed in a way the scheduler's own might not.
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
async fn send(
    operation: IppOperation,
    operation_name: &'static str,
    target: String,
    printer_uri: String,
    attributes: impl FnOnce(&mut IppRequest) -> BackendResult<()> + Send + 'static,
) -> BackendResult<()> {
    tokio::task::spawn_blocking(move || {
        // Duplicate charset/language operation attributes make CUPS ignore printer attributes.
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

        let response = scheduler::send_with_timeouts(request, &target, ADMIN_TIMEOUTS)?;

        ensure_success(&response, operation_name)
    })
    .await
    .map_err(BackendError::Join)?
}
