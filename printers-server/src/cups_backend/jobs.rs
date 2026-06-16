use cosmic_settings_printers_core::{Error, JobInfo, JobState};
use cups_rs::{IppOperation, IppRequest, IppTag, IppValueTag};

use super::helpers::{CupsResultExt, add_requesting_user, ensure_success};

pub async fn get_jobs(printer_id: &str, filter: &str) -> Result<Vec<JobInfo>, Error> {
    let printer_id = if printer_id.is_empty() {
        None
    } else {
        Some(printer_id.to_string())
    };
    let filter = filter.to_string();

    tokio::task::spawn_blocking(move || {
        let printer_id = printer_id.as_deref();
        let jobs = match filter.as_str() {
            "active" => cups_rs::get_active_jobs(printer_id),
            "completed" => cups_rs::get_completed_jobs(printer_id),
            _ => cups_rs::get_jobs(printer_id),
        }
        .cups_err()?;

        Ok::<Vec<JobInfo>, Error>(jobs.into_iter().map(job_info).collect())
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

pub async fn cancel_job(printer_uri: &str, job_id: i32) -> Result<(), Error> {
    send_job_request(IppOperation::CancelJob, printer_uri, job_id).await
}

pub async fn pause_job(printer_uri: &str, job_id: i32) -> Result<(), Error> {
    send_job_request(IppOperation::HoldJob, printer_uri, job_id).await
}

pub async fn resume_job(printer_uri: &str, job_id: i32) -> Result<(), Error> {
    send_job_request(IppOperation::ReleaseJob, printer_uri, job_id).await
}

async fn send_job_request(
    operation: IppOperation,
    printer_uri: &str,
    job_id: i32,
) -> Result<(), Error> {
    let printer_uri = printer_uri.to_string();

    tokio::task::spawn_blocking(move || {
        let mut request = IppRequest::new(operation).cups_err()?;

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

        let response = request.send_default("/jobs/").cups_err()?;

        ensure_success(&response, "job operation")
    })
    .await
    .map_err(|error| Error::Internal {
        why: error.to_string(),
    })?
}

/// Converts cups-rs job data into the job type exposed by the printer API.
fn job_info(job: cups_rs::JobInfo) -> JobInfo {
    JobInfo {
        id: job.id,
        printer_id: job.dest,
        title: job.title,
        state: job_state(job.status),
        user: job.user,
        size: job.size,
        priority: job.priority,
        creation_time: job.creation_time,
        processing_time: job.processing_time,
        completed_time: job.completed_time,
    }
}

/// Maps a cups-rs job status to the shared API job state.
fn job_state(status: cups_rs::JobStatus) -> JobState {
    match status {
        cups_rs::JobStatus::Pending => JobState::Pending,
        cups_rs::JobStatus::Processing => JobState::Processing,
        cups_rs::JobStatus::Completed => JobState::Completed,
        cups_rs::JobStatus::Canceled => JobState::Canceled,
        cups_rs::JobStatus::Aborted => JobState::Aborted,
        cups_rs::JobStatus::Held => JobState::Held,
        cups_rs::JobStatus::Stopped => JobState::Stopped,
        cups_rs::JobStatus::Unknown => JobState::Unknown,
    }
}
