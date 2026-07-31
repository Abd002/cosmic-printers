use futures_util::{Stream, StreamExt};
use std::path::PathBuf;
use zlink::Connection;

pub use cosmic_settings_printers_core::{
    Error, GetJobsReply, GroupedDevice, JobFilter, JobInfo, JobState, ListDiscoveredPrintersReply,
    ListPrinterApplicationsReply, ListPrintersReply, PrintTestPageReply, PrinterApplication,
    PrinterApplicationState, PrinterEntry, PrinterStatus, PrintersEvent, PrintersEventKind,
    SupplyLevel, group_printers, printers_match,
};

mod protocol;

pub type ClientResult<T> = Result<T, ClientError>;

#[derive(Debug)]
#[non_exhaustive]
pub enum ClientError {
    RuntimeDirectoryUnavailable,
    Transport(zlink::Error),
    Service(cosmic_settings_printers_core::Error),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuntimeDirectoryUnavailable => {
                formatter.write_str("the runtime directory is unavailable")
            }
            Self::Transport(error) => write!(formatter, "printer service transport error: {error}"),
            Self::Service(error) => write!(formatter, "printer service error: {error}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RuntimeDirectoryUnavailable => None,
            Self::Transport(error) => Some(error),
            Self::Service(error) => Some(error),
        }
    }
}

impl From<zlink::Error> for ClientError {
    fn from(error: zlink::Error) -> Self {
        Self::Transport(error)
    }
}

impl From<cosmic_settings_printers_core::Error> for ClientError {
    fn from(error: cosmic_settings_printers_core::Error) -> Self {
        Self::Service(error)
    }
}

pub async fn connect() -> ClientResult<Client> {
    Client::connect().await
}

pub struct Client {
    #[deprecated(note = "use the high-level Client methods instead")]
    pub conn: Connection<zlink::unix::Stream>,
}

#[allow(deprecated)]
impl Client {
    pub async fn connect() -> ClientResult<Self> {
        let path = socket_path()?;
        let conn = zlink::unix::connect(path)
            .await
            .map_err(ClientError::Transport)?;

        Ok(Self { conn })
    }

    pub async fn printers(&mut self) -> ClientResult<Vec<PrinterEntry>> {
        let reply = flatten(protocol::CosmicPrintersProxy::list_printers(&mut self.conn).await)?;
        Ok(reply.printers)
    }

    /// Starts a background discovery refresh.
    ///
    /// Watch [`Self::printer_events`] and read the cache again to observe
    /// results that arrive after this call returns.
    pub async fn start_discovery(&mut self) -> ClientResult<()> {
        flatten(protocol::CosmicPrintersProxy::start_discovery(&mut self.conn).await)
    }

    /// Returns the current discovered-printer cache without starting discovery.
    pub async fn discovered_printers(&mut self) -> ClientResult<Vec<PrinterEntry>> {
        let reply =
            flatten(protocol::CosmicPrintersProxy::list_discovered_printers(&mut self.conn).await)?;
        Ok(reply.printers)
    }

    /// Returns the current Printer Application cache without starting discovery.
    pub async fn printer_applications(&mut self) -> ClientResult<Vec<PrinterApplication>> {
        let reply = flatten(
            protocol::CosmicPrintersProxy::list_printer_applications(&mut self.conn).await,
        )?;
        Ok(reply.printer_applications)
    }

    pub async fn printer_events(
        &mut self,
    ) -> ClientResult<impl Stream<Item = ClientResult<PrintersEvent>> + '_> {
        let events = protocol::CosmicPrintersProxy::watch_printers(&mut self.conn).await?;
        Ok(events.map(flatten))
    }

    pub async fn add_discovered_printer(&mut self, printer_id: &str) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::add_discovered_printer(
                &mut self.conn,
                printer_id.to_owned(),
            )
            .await,
        )
    }

    pub async fn delete_printer(&mut self, printer_id: &str) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::delete_printer(&mut self.conn, printer_id.to_owned())
                .await,
        )
    }

    pub async fn set_printer_accept_jobs(
        &mut self,
        printer_id: &str,
        enabled: bool,
        reason: &str,
    ) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_accept_jobs(
                &mut self.conn,
                printer_id.to_owned(),
                enabled,
                reason.to_owned(),
            )
            .await,
        )
    }

    pub async fn set_printer_default(&mut self, printer_id: &str) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_default(
                &mut self.conn,
                printer_id.to_owned(),
            )
            .await,
        )
    }

    pub async fn set_printer_option_default(
        &mut self,
        printer_id: &str,
        option: &str,
        values: &[String],
    ) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_option_default(
                &mut self.conn,
                printer_id.to_owned(),
                option.to_owned(),
                values.to_vec(),
            )
            .await,
        )
    }

    pub async fn set_printer_enabled(
        &mut self,
        printer_id: &str,
        enabled: bool,
    ) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_enabled(
                &mut self.conn,
                printer_id.to_owned(),
                enabled,
            )
            .await,
        )
    }

    pub async fn set_printer_info(&mut self, printer_id: &str, info: &str) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_info(
                &mut self.conn,
                printer_id.to_owned(),
                info.to_owned(),
            )
            .await,
        )
    }

    pub async fn set_printer_location(
        &mut self,
        printer_id: &str,
        location: &str,
    ) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_location(
                &mut self.conn,
                printer_id.to_owned(),
                location.to_owned(),
            )
            .await,
        )
    }

    pub async fn set_printer_shared(&mut self, printer_id: &str, shared: bool) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::set_printer_shared(
                &mut self.conn,
                printer_id.to_owned(),
                shared,
            )
            .await,
        )
    }

    pub async fn print_test_page(&mut self, printer_id: &str) -> ClientResult<i32> {
        let reply = flatten(
            protocol::CosmicPrintersProxy::print_test_page(&mut self.conn, printer_id.to_owned())
                .await,
        )?;
        Ok(reply.job_id)
    }

    pub async fn jobs(
        &mut self,
        printer_id: &str,
        filter: JobFilter,
    ) -> ClientResult<Vec<JobInfo>> {
        let reply = flatten(
            protocol::CosmicPrintersProxy::get_jobs(
                &mut self.conn,
                printer_id.to_owned(),
                job_filter_protocol_value(filter).to_owned(),
            )
            .await,
        )?;
        Ok(reply.jobs)
    }

    pub async fn pause_job(&mut self, printer_id: &str, job_id: i32) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::pause_job(&mut self.conn, printer_id.to_owned(), job_id)
                .await,
        )
    }

    pub async fn resume_job(&mut self, printer_id: &str, job_id: i32) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::resume_job(
                &mut self.conn,
                printer_id.to_owned(),
                job_id,
            )
            .await,
        )
    }

    pub async fn cancel_job(&mut self, printer_id: &str, job_id: i32) -> ClientResult<()> {
        flatten(
            protocol::CosmicPrintersProxy::cancel_job(
                &mut self.conn,
                printer_id.to_owned(),
                job_id,
            )
            .await,
        )
    }
}

pub fn socket_path() -> ClientResult<PathBuf> {
    let runtime_dir = dirs::runtime_dir().ok_or(ClientError::RuntimeDirectoryUnavailable)?;

    Ok(runtime_dir.join("com.system76.CosmicSettings"))
}

fn flatten<T>(
    result: zlink::Result<Result<T, cosmic_settings_printers_core::Error>>,
) -> ClientResult<T> {
    result
        .map_err(ClientError::Transport)?
        .map_err(ClientError::Service)
}

fn job_filter_protocol_value(filter: JobFilter) -> &'static str {
    match filter {
        JobFilter::Active => "active",
        JobFilter::Completed => "completed",
        JobFilter::All => "all",
    }
}

#[doc(hidden)]
#[deprecated(note = "use the high-level Client methods instead")]
pub use protocol::CosmicPrintersProxy;

#[cfg(test)]
mod tests {
    use super::{JobFilter, job_filter_protocol_value};

    #[test]
    fn job_filters_match_service_values() {
        assert_eq!(job_filter_protocol_value(JobFilter::Active), "active");
        assert_eq!(job_filter_protocol_value(JobFilter::Completed), "completed");
        assert_eq!(job_filter_protocol_value(JobFilter::All), "all");
    }
}
