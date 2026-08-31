//! Print jobs, as a client sees them.

use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, zlink::introspect::Type,
)]
pub enum JobFilter {
    #[default]
    Active,
    Completed,
    All,
}

#[derive(Clone, Debug, Serialize, Deserialize, zlink::introspect::Type)]
pub struct JobInfo {
    pub id: i32,
    pub printer_id: String,
    pub title: String,
    pub state: JobState,
    pub user: String,
    pub size: i32,
    pub priority: i32,
    pub creation_time: i64,
    pub processing_time: i64,
    pub completed_time: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, zlink::introspect::Type)]
pub enum JobState {
    Pending,
    Processing,
    Completed,
    Canceled,
    Aborted,
    Held,
    Stopped,
    Failed,
    Unknown,
}
