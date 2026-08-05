use cosmic_settings_printers_core::{JobInfo, JobState, PrinterEntry};
use cups_rs::{IppAttribute, IppOperation, IppRequest, IppStatus, IppTag, IppValueTag};

use super::helpers::{
    CupsResultExt, add_requesting_user, ensure_success, printer_uri_from_parts, request_scheme,
    send_ipp_request,
};
use crate::error::{BackendError, BackendResult};

const JOB_ATTRIBUTES: &[&str] = &[
    "job-id",
    "job-uri",
    "job-printer-uri",
    "job-name",
    "job-state",
    "job-state-reasons",
    "job-originating-user-name",
    "job-k-octets",
    "job-impressions-completed",
    "job-priority",
    "time-at-creation",
    "time-at-processing",
    "time-at-completed",
];
const CUPS_JOBS_URI: &str = "ipp://localhost/jobs";

pub async fn get_jobs(printer: &PrinterEntry, filter: &str) -> BackendResult<Vec<JobInfo>> {
    let printer_id = printer.id().to_string();
    let printer_uri = resolve_job_printer_uri(printer)?;
    let filter = filter.to_string();

    tokio::task::spawn_blocking(move || {
        let request = get_jobs_request(&printer_uri, which_jobs(&filter))?;
        let response = send_ipp_request(request, &printer_uri)?;
        ensure_success(&response, "Get-Jobs")?;

        Ok(parse_jobs(response.attributes(), &printer_id))
    })
    .await
    .map_err(BackendError::Join)?
}

fn get_jobs_request(printer_uri: &str, which_jobs: &str) -> BackendResult<IppRequest> {
    let mut request = IppRequest::new(IppOperation::GetJobs).cups_err()?;

    add_operation_defaults(&mut request)?;
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Uri,
            "printer-uri",
            printer_uri,
        )
        .cups_err()?;
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Keyword,
            "which-jobs",
            which_jobs,
        )
        .cups_err()?;
    request
        .add_boolean(IppTag::Operation, "my-jobs", false)
        .cups_err()?;
    request
        .add_strings(
            IppTag::Operation,
            IppValueTag::Keyword,
            "requested-attributes",
            JOB_ATTRIBUTES,
        )
        .cups_err()?;
    add_requesting_user(&mut request)?;

    Ok(request)
}

fn which_jobs(filter: &str) -> &str {
    match filter {
        "active" => "not-completed",
        "completed" => "completed",
        _ => "all",
    }
}

pub async fn cancel_job(printer: &PrinterEntry, job_id: i32) -> BackendResult<()> {
    send_job_request(IppOperation::CancelJob, printer, job_id).await
}

pub async fn pause_job(printer: &PrinterEntry, job_id: i32) -> BackendResult<()> {
    send_job_request(IppOperation::HoldJob, printer, job_id).await
}

pub async fn resume_job(printer: &PrinterEntry, job_id: i32) -> BackendResult<()> {
    send_job_request(IppOperation::ReleaseJob, printer, job_id).await
}

pub async fn move_job(
    source: &PrinterEntry,
    job_id: i32,
    destination: &PrinterEntry,
) -> BackendResult<()> {
    if source.id() == destination.id() {
        return Err(BackendError::InvalidMoveDestination {
            why: "source and destination queues are the same".to_string(),
        });
    }

    let source_uri = resolve_job_printer_uri(source)?;
    let destination_uri = resolve_job_printer_uri(destination)?;

    tokio::task::spawn_blocking(move || {
        let mut request = IppRequest::new(IppOperation::CupsMoveJob).cups_err()?;

        add_operation_defaults(&mut request)?;
        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                "printer-uri",
                &source_uri,
            )
            .cups_err()?;
        request
            .add_integer(IppTag::Operation, IppValueTag::Integer, "job-id", job_id)
            .cups_err()?;
        add_requesting_user(&mut request)?;
        request
            .add_string(
                IppTag::Job,
                IppValueTag::Uri,
                "job-printer-uri",
                &destination_uri,
            )
            .cups_err()?;

        let response = send_ipp_request(request, CUPS_JOBS_URI)?;
        ensure_move_job_success(response.status(), job_id)
    })
    .await
    .map_err(BackendError::Join)?
}

async fn send_job_request(
    operation: IppOperation,
    printer: &PrinterEntry,
    job_id: i32,
) -> BackendResult<()> {
    let printer_uri = resolve_job_printer_uri(printer)?;

    tokio::task::spawn_blocking(move || {
        let mut request = IppRequest::new(operation).cups_err()?;

        add_operation_defaults(&mut request)?;
        request
            .add_string(
                IppTag::Operation,
                IppValueTag::Uri,
                "printer-uri",
                &printer_uri,
            )
            .cups_err()?;
        request
            .add_integer(IppTag::Operation, IppValueTag::Integer, "job-id", job_id)
            .cups_err()?;
        add_requesting_user(&mut request)?;

        let response = send_ipp_request(request, &printer_uri)?;

        ensure_success(&response, "job operation")
    })
    .await
    .map_err(BackendError::Join)?
}

fn add_operation_defaults(request: &mut IppRequest) -> BackendResult<()> {
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Charset,
            "attributes-charset",
            "utf-8",
        )
        .cups_err()?;
    request
        .add_string(
            IppTag::Operation,
            IppValueTag::Language,
            "attributes-natural-language",
            "en",
        )
        .cups_err()
}

/// Returns the URI to address this printer's jobs at.
///
/// A queue keeps its jobs on the scheduler, so a queue's own URI is used as it
/// stands. A discovered printer is no queue at all: the scheduler has no
/// `/printers/<name>` for it and answers `client-error-not-found` if asked for one,
/// so the printer itself is asked, at the endpoint connecting to the device
/// resolved. Its `printer-uri-supported` cannot serve here, that being the DNS-SD
/// service name, which names a service rather than a printer; the resolved resource
/// path is what says which printer on that endpoint this is.
fn resolve_job_printer_uri(printer: &PrinterEntry) -> BackendResult<String> {
    if let Some(uri) = printer
        .printer_uri()
        .filter(|uri| crate::ipp::is_local_scheduler_uri(uri))
    {
        return Ok(uri.to_owned());
    }

    endpoint_job_uri(printer).ok_or_else(|| BackendError::UnknownEndpoint {
        printer: printer.id().to_string(),
    })
}

fn endpoint_job_uri(printer: &PrinterEntry) -> Option<String> {
    let (host, port) = printer.endpoint()?;
    let scheme = if printer.endpoint_is_local() {
        // A Printer Application advertises `_ipps` but reaches TLS by upgrading a
        // plain connection, so locally the plain scheme is the one that answers.
        "ipp"
    } else {
        request_scheme(printer.printer_uri().or_else(|| printer.device_uri())?)
    };

    printer_uri_from_parts(
        scheme,
        Some(&host),
        Some(port),
        printer.endpoint_resource_path()?,
    )
}

fn ensure_move_job_success(status: IppStatus, job_id: i32) -> BackendResult<()> {
    if status.is_successful() {
        return Ok(());
    }

    match status {
        IppStatus::ErrorNotAuthorized
        | IppStatus::ErrorForbidden
        | IppStatus::ErrorNotAuthenticated => Err(BackendError::PermissionDenied {
            operation: "CUPS-Move-Job".to_string(),
        }),
        IppStatus::ErrorNotFound | IppStatus::ErrorGone => {
            Err(BackendError::JobNotFound { job_id })
        }
        IppStatus::ErrorNotPossible => Err(BackendError::JobNotMovable { job_id }),
        IppStatus::ErrorOperationNotSupported => Err(BackendError::OperationNotSupported {
            operation: "CUPS-Move-Job".to_string(),
        }),
        _ => Err(BackendError::IppStatus {
            operation: "CUPS-Move-Job".to_string(),
            status: format!("{status:?}"),
        }),
    }
}

struct JobBuilder<'a> {
    printer_id: &'a str,
    id: Option<i32>,
    title: String,
    state: JobState,
    user: String,
    size: i32,
    priority: i32,
    creation_time: i64,
    processing_time: i64,
    completed_time: i64,
}

impl<'a> JobBuilder<'a> {
    fn new(printer_id: &'a str) -> Self {
        Self {
            printer_id,
            id: None,
            title: String::new(),
            state: JobState::Unknown,
            user: String::new(),
            size: 0,
            priority: 0,
            creation_time: 0,
            processing_time: 0,
            completed_time: 0,
        }
    }

    fn apply(&mut self, name: &str, attr: &IppAttribute) {
        match name {
            "job-id" => {
                let id = attr.get_integer(0);
                self.id = (id != 0).then_some(id);
            }
            "job-name" => self.title = attr.get_string(0).unwrap_or_default(),
            "job-state" => self.state = job_state(attr.get_integer(0)),
            "job-originating-user-name" => {
                self.user = attr.get_string(0).unwrap_or_default();
            }
            "job-k-octets" => self.size = attr.get_integer(0),
            "job-priority" => self.priority = attr.get_integer(0),
            "time-at-creation" => self.creation_time = i64::from(attr.get_integer(0)),
            "time-at-processing" => self.processing_time = i64::from(attr.get_integer(0)),
            "time-at-completed" => self.completed_time = i64::from(attr.get_integer(0)),
            _ => {}
        }
    }

    fn finish(self) -> Option<JobInfo> {
        Some(JobInfo {
            id: self.id?,
            printer_id: self.printer_id.to_string(),
            title: self.title,
            state: self.state,
            user: self.user,
            size: self.size,
            priority: self.priority,
            creation_time: self.creation_time,
            processing_time: self.processing_time,
            completed_time: self.completed_time,
        })
    }
}

fn parse_jobs(attributes: Vec<IppAttribute>, fallback_printer_id: &str) -> Vec<JobInfo> {
    // cups-rs does not expose IPP group tags/group boundaries yet. CUPS and IPP
    // Printer Applications return each job group starting with job-id, so use
    // that as the boundary while keeping the local destination id as ownership.
    let mut jobs = Vec::new();
    let mut job = JobBuilder::new(fallback_printer_id);

    for attr in attributes {
        let Some(name) = attr.name() else {
            continue;
        };

        if name == "job-id"
            && let Some(previous) =
                std::mem::replace(&mut job, JobBuilder::new(fallback_printer_id)).finish()
        {
            jobs.push(previous);
        }

        job.apply(&name, &attr);
    }

    if let Some(job) = job.finish() {
        jobs.push(job);
    }

    jobs
}

/// Maps IPP job-state enum values to the shared API job state.
fn job_state(state: i32) -> JobState {
    match state {
        3 => JobState::Pending,
        4 => JobState::Held,
        5 => JobState::Processing,
        6 => JobState::Stopped,
        7 => JobState::Canceled,
        8 => JobState::Aborted,
        9 => JobState::Completed,
        _ => JobState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(options: &[(&str, &str)]) -> PrinterEntry {
        PrinterEntry::new(
            "Acme_Laser",
            "Acme Laser",
            false,
            options
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        )
    }

    /// A queue's jobs are the scheduler's to report, so its own URI is kept.
    #[test]
    fn a_queue_is_asked_through_the_scheduler() {
        let printer = printer(&[(
            "printer-uri-supported",
            "ipp://localhost:631/printers/Acme_Laser",
        )]);

        assert_eq!(
            resolve_job_printer_uri(&printer).unwrap(),
            "ipp://localhost:631/printers/Acme_Laser"
        );
    }

    /// The case that was failing: the scheduler has no queue for a discovered
    /// printer, so it is asked at the application that resolved it — over `ipp`,
    /// although it advertised `ipps`.
    #[test]
    fn a_printer_on_a_local_application_is_asked_directly() {
        let printer = printer(&[
            (
                "printer-uri-supported",
                "ipps://Acme_Laser._ipps._tcp.local/",
            ),
            ("endpoint-hostname", "desktop.local"),
            ("endpoint-port", "8000"),
            ("endpoint-is-local", "true"),
            ("endpoint-resource-path", "/ipp/print/Acme_Laser"),
        ]);

        assert_eq!(
            resolve_job_printer_uri(&printer).unwrap(),
            "ipp://localhost:8000/ipp/print/Acme_Laser"
        );
    }

    /// A remote printer does serve the scheme it advertised, unlike a local
    /// application, and is named by the host it was reached at.
    #[test]
    fn a_remote_printer_keeps_the_scheme_it_advertised() {
        let printer = printer(&[
            (
                "printer-uri-supported",
                "ipps://Acme_Laser._ipps._tcp.local/",
            ),
            ("endpoint-hostname", "printer.example"),
            ("endpoint-port", "631"),
            ("endpoint-is-local", "false"),
            ("endpoint-resource-path", "/ipp/print"),
        ]);

        assert_eq!(
            resolve_job_printer_uri(&printer).unwrap(),
            "ipps://printer.example:631/ipp/print"
        );
    }

    /// Nothing resolved, so there is nowhere to ask. Guessing a queue name is what
    /// made the scheduler answer `client-error-not-found` for every discovered
    /// printer, once per refresh.
    #[test]
    fn a_printer_that_resolved_to_nothing_is_reported_unreachable() {
        let printer = printer(&[(
            "printer-uri-supported",
            "ipps://Acme_Laser._ipps._tcp.local/",
        )]);

        assert!(matches!(
            resolve_job_printer_uri(&printer),
            Err(BackendError::UnknownEndpoint { .. })
        ));
    }

    #[test]
    fn job_builder_discards_a_job_without_an_id() {
        assert!(JobBuilder::new("printer").finish().is_none());
    }

    #[test]
    fn job_builder_finishes_a_job_with_its_fallback_printer() {
        let mut builder = JobBuilder::new("printer");
        builder.id = Some(42);

        let job = builder.finish().unwrap();
        assert_eq!(job.id, 42);
        assert_eq!(job.printer_id, "printer");
    }

    #[test]
    fn move_job_rejects_finished_jobs_as_stale_state() {
        assert!(matches!(
            ensure_move_job_success(IppStatus::ErrorNotPossible, 42),
            Err(BackendError::JobNotMovable { job_id: 42 })
        ));
    }

    #[test]
    fn move_job_reports_unsupported_schedulers() {
        assert!(matches!(
            ensure_move_job_success(IppStatus::ErrorOperationNotSupported, 42),
            Err(BackendError::OperationNotSupported { .. })
        ));
    }
}
