use cosmic_settings_printers_core::{JobInfo, JobState, PrinterEntry};
use cups_rs::{IppAttribute, IppOperation, IppRequest, IppTag, IppValueTag};

use super::helpers::{
    CupsResultExt, add_requesting_user, ensure_success, local_printer_uri, send_ipp_request,
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

pub async fn get_jobs(printer: &PrinterEntry, filter: &str) -> BackendResult<Vec<JobInfo>> {
    let printer_id = printer.id().to_string();
    let printer_uri = resolve_job_printer_uri(printer);
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

async fn send_job_request(
    operation: IppOperation,
    printer: &PrinterEntry,
    job_id: i32,
) -> BackendResult<()> {
    let printer_uri = resolve_job_printer_uri(printer);

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

fn resolve_job_printer_uri(printer: &PrinterEntry) -> String {
    // match printer.device_uri().filter(|uri| is_ipp_uri(uri)) {
    //     Some(uri) => uri.to_string(),
    //     None => local_printer_uri(printer.id(), false),
    // }
    local_printer_uri(printer.id(), false)
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
}
