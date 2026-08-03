//! Varlink replies require named object fields rather than bare lists.

use serde::{Deserialize, Serialize};

use crate::PrinterApplication;
use crate::jobs::JobInfo;
use crate::printer::PrinterEntry;
use crate::supplies::SupplyLevel;

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListPrintersReply {
    pub printers: Vec<PrinterEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct ListPrinterApplicationsReply {
    pub printer_applications: Vec<PrinterApplication>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct GetPrinterSuppliesReply {
    pub supplies: Vec<SupplyLevel>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct GetJobsReply {
    pub jobs: Vec<JobInfo>,
}

#[derive(Debug, Clone, Deserialize, Serialize, zlink::introspect::Type)]
pub struct PrintTestPageReply {
    pub job_id: i32,
}
