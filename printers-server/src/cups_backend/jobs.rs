use cosmic_settings_printers_core::{Error, JobInfo, JobState, PrinterEntry};
use cups_rs::{IppAttribute, IppOperation, IppRequest, IppTag, IppValueTag};

use super::helpers::{
    CupsResultExt, add_requesting_user, ensure_success, is_ipp_uri, local_printer_uri,
    send_ipp_request_to_printer_uri,
};

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

pub async fn get_jobs(printer: &PrinterEntry, filter: &str) -> Result<Vec<JobInfo>, Error> {
    let printer_id = printer.id.clone();
    let printer_uri = resolve_job_printer_uri(printer);
    let filter = filter.to_string();

    tokio::task::spawn_blocking(move || {
        let request = get_jobs_request(&printer_uri, which_jobs(&filter))?;
        let response = send_ipp_request_to_printer_uri(request, &printer_uri)?;
        ensure_success(&response, "Get-Jobs")?;

        Ok::<Vec<JobInfo>, Error>(parse_jobs(response.attributes(), &printer_id))
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

fn get_jobs_request(printer_uri: &str, which_jobs: &str) -> Result<IppRequest, Error> {
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

pub async fn cancel_job(printer: &PrinterEntry, job_id: i32) -> Result<(), Error> {
    send_job_request(IppOperation::CancelJob, printer, job_id).await
}

pub async fn pause_job(printer: &PrinterEntry, job_id: i32) -> Result<(), Error> {
    send_job_request(IppOperation::HoldJob, printer, job_id).await
}

pub async fn resume_job(printer: &PrinterEntry, job_id: i32) -> Result<(), Error> {
    send_job_request(IppOperation::ReleaseJob, printer, job_id).await
}

async fn send_job_request(
    operation: IppOperation,
    printer: &PrinterEntry,
    job_id: i32,
) -> Result<(), Error> {
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

        let response = send_ipp_request_to_printer_uri(request, &printer_uri)?;

        ensure_success(&response, "job operation")
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

fn add_operation_defaults(request: &mut IppRequest) -> Result<(), Error> {
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
    // match Some(printer.device_uri.as_str()).filter(|uri| is_ipp_uri(uri)) {
    //     Some(uri) => uri.to_string(),
    //     None => local_printer_uri(&printer.id, false),
    // }
    local_printer_uri(&printer.id, false)
}

fn parse_jobs(attributes: Vec<IppAttribute>, fallback_printer_id: &str) -> Vec<JobInfo> {
    // cups-rs does not expose IPP group tags/group boundaries yet. CUPS and IPP
    // Printer Applications return each job group starting with job-id, so use
    // that as the boundary while keeping the local destination id as ownership.
    let mut jobs = Vec::new();
    let mut job = JobInfo {
        id: 0,
        printer_id: fallback_printer_id.to_string(),
        title: String::new(),
        state: JobState::Unknown,
        user: String::new(),
        size: 0,
        priority: 0,
        creation_time: 0,
        processing_time: 0,
        completed_time: 0,
    };

    for attr in attributes {
        let Some(name) = attr.name() else {
            continue;
        };

        if name == "job-id" {
            if job.id != 0 {
                jobs.push(job);
                job = JobInfo {
                    id: 0,
                    printer_id: fallback_printer_id.to_string(),
                    title: String::new(),
                    state: JobState::Unknown,
                    user: String::new(),
                    size: 0,
                    priority: 0,
                    creation_time: 0,
                    processing_time: 0,
                    completed_time: 0,
                };
            }
        }

        match name.as_str() {
            "job-id" => job.id = attr.get_integer(0),
            "job-name" => job.title = attr.get_string(0).unwrap_or_default(),
            "job-state" => job.state = job_state(attr.get_integer(0)),
            "job-originating-user-name" => job.user = attr.get_string(0).unwrap_or_default(),
            "job-k-octets" => job.size = attr.get_integer(0),
            "job-priority" => job.priority = attr.get_integer(0),
            "time-at-creation" => job.creation_time = i64::from(attr.get_integer(0)),
            "time-at-processing" => job.processing_time = i64::from(attr.get_integer(0)),
            "time-at-completed" => job.completed_time = i64::from(attr.get_integer(0)),
            _ => {}
        }
    }

    if job.id != 0 {
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
