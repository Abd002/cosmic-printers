//! Backend abstraction for printer UI requests.

use cosmic_settings_printers_client::{self as client, ClientError};
use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, ConfigureDiscoveredPrinterRequest, ConfigurePrinterReply, Error,
    JobFilter, JobInfo, ManualSetupPrinterApplication, PrinterApplication, PrinterEntry,
    PrintersEvent, StartAddPrinterDiscoveryReply, SupplyLevel,
};

/// Errors returned by backend requests.
///
/// Service errors remain structured so setup-page responses can be handled separately.
#[derive(Debug)]
pub enum BackendError {
    /// The backend is unavailable.
    Unavailable(String),
    /// The backend returned a service error.
    Service(Error),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => formatter.write_str(why),
            Self::Service(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<ClientError> for BackendError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Service(error) => Self::Service(error),
            error => Self::Unavailable(error.to_string()),
        }
    }
}

impl From<Error> for BackendError {
    fn from(error: Error) -> Self {
        Self::Service(error)
    }
}

type Answer<T> = Result<T, BackendError>;

/// Backend used by printer UI requests.
#[derive(Clone)]
pub enum Backend {
    /// Connects to the daemon.
    Daemon,
    /// Runs an embedded server.
    #[cfg(feature = "embedded")]
    Embedded(std::sync::Arc<cosmic_settings_printers_server::Server>),
}

impl Default for Backend {
    fn default() -> Self {
        Self::Daemon
    }
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Daemon => formatter.write_str("Daemon"),
            #[cfg(feature = "embedded")]
            Self::Embedded(_) => formatter.write_str("Embedded"),
        }
    }
}

impl Backend {
    /// Uses the daemon when available and otherwise falls back to an embedded server.
    pub async fn detect() -> Self {
        match std::env::var("COSMIC_PRINTERS_BACKEND").as_deref() {
            Ok("daemon") => return Self::Daemon,
            #[cfg(feature = "embedded")]
            Ok("embedded") => return Self::embedded(),
            _ => {}
        }

        match client::connect().await {
            Ok(_) => Self::Daemon,
            Err(error) => {
                tracing::info!(
                    %error,
                    "no printers daemon to talk to; serving printers from this process"
                );
                Self::fallback()
            }
        }
    }

    /// Selects a backend synchronously.
    #[must_use]
    pub fn detect_blocking() -> Self {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return Self::fallback();
        };

        runtime.block_on(Self::detect())
    }

    /// Creates an embedded backend.
    #[cfg(feature = "embedded")]
    #[must_use]
    pub fn embedded() -> Self {
        Self::Embedded(std::sync::Arc::new(
            cosmic_settings_printers_server::Server::new(),
        ))
    }

    #[cfg(feature = "embedded")]
    fn fallback() -> Self {
        Self::embedded()
    }

    #[cfg(not(feature = "embedded"))]
    fn fallback() -> Self {
        Self::Daemon
    }

    async fn client(&self) -> Answer<client::Client> {
        client::connect().await.map_err(BackendError::from)
    }
}

impl Backend {
    /// Lists available printers.
    pub async fn printers(&self) -> Answer<Vec<PrinterEntry>> {
        match self {
            Self::Daemon => Ok(self.client().await?.printers().await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.list_printers().await?),
        }
    }

    /// Reads one printer from the backend cache. Missing means it was removed.
    pub async fn printer(&self, printer_id: &str) -> Answer<Option<PrinterEntry>> {
        let result = match self {
            Self::Daemon => self
                .client()
                .await?
                .printer(printer_id)
                .await
                .map_err(BackendError::from),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => server
                .get_printer(printer_id)
                .await
                .map_err(BackendError::from),
        };

        match result {
            Ok(printer) => Ok(Some(printer)),
            Err(BackendError::Service(Error::PrinterNotFound)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Refreshes available CUPS destinations.
    pub async fn refresh_available_destinations(&self) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .refresh_available_destinations()
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.refresh_available_destinations().await?),
        }
    }

    /// Starts Printer Application discovery.
    pub async fn start_printer_application_discovery(&self) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .start_printer_application_discovery()
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.start_printer_application_discovery().await?),
        }
    }

    /// Lists discovered Printer Applications.
    pub async fn printer_applications(&self) -> Answer<Vec<PrinterApplication>> {
        match self {
            Self::Daemon => Ok(self.client().await?.printer_applications().await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.list_printer_applications().await?),
        }
    }

    /// Starts a round of Add Printer discovery.
    pub async fn start_add_printer_discovery(&self) -> Answer<StartAddPrinterDiscoveryReply> {
        match self {
            Self::Daemon => Ok(self.client().await?.start_add_printer_discovery().await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.start_add_printer_discovery().await?),
        }
    }

    /// Returns the active Add Printer discovery round.
    pub async fn add_printer_discovery(&self) -> Answer<AddPrinterDiscoveryReply> {
        match self {
            Self::Daemon => Ok(self.client().await?.add_printer_discovery().await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.get_add_printer_discovery().await?),
        }
    }

    /// Lists Printer Applications that require manual setup.
    pub async fn manual_setup_printer_applications(
        &self,
    ) -> Answer<Vec<ManualSetupPrinterApplication>> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .manual_setup_printer_applications()
                .await?),
            // Match the client path by unwrapping the server's wire envelope.
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server
                .list_manual_setup_printer_applications()
                .await?
                .printer_applications),
        }
    }

    /// Configures a discovered printer.
    pub async fn configure_discovered_printer(
        &self,
        request: ConfigureDiscoveredPrinterRequest,
    ) -> Answer<ConfigurePrinterReply> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .configure_discovered_printer(request)
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.configure_discovered_printer(request).await?),
        }
    }

    /// Returns the result of a printer configuration attempt.
    pub async fn printer_configuration(&self, operation_id: &str) -> Answer<ConfigurePrinterReply> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .printer_configuration(operation_id)
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.get_printer_configuration(operation_id).await?),
        }
    }

    /// Sets the user's default printer.
    pub async fn set_printer_default(&self, printer_id: &str) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self.client().await?.set_printer_default(printer_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.set_printer_default(printer_id).await?),
        }
    }

    /// Clears the user's default printer.
    pub async fn clear_printer_default(&self) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self.client().await?.clear_printer_default().await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.clear_printer_default().await?),
        }
    }

    /// Removes a printer.
    pub async fn delete_printer(&self, printer_id: &str) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self.client().await?.delete_printer(printer_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.delete_printer(printer_id).await?),
        }
    }

    /// Sets a printer's location.
    pub async fn set_printer_location(&self, printer_id: &str, location: &str) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .set_printer_location(printer_id, location)
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.set_printer_location(printer_id, location).await?),
        }
    }

    /// Sets a user default for a printer option.
    pub async fn set_printer_option_default(
        &self,
        printer_id: &str,
        option: &str,
        values: &[String],
    ) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .set_printer_option_default(printer_id, option, values)
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server
                .set_printer_option_default(printer_id, option, values)
                .await?),
        }
    }

    /// Returns a printer's supply levels.
    pub async fn printer_supplies(&self, printer_id: &str) -> Answer<Vec<SupplyLevel>> {
        match self {
            Self::Daemon => Ok(self.client().await?.printer_supplies(printer_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.printer_supplies(printer_id).await?),
        }
    }

    /// Lists a printer's jobs.
    pub async fn jobs(&self, printer_id: &str, filter: JobFilter) -> Answer<Vec<JobInfo>> {
        match self {
            Self::Daemon => Ok(self.client().await?.jobs(printer_id, filter).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.get_jobs(printer_id, job_filter(filter)).await?),
        }
    }

    /// Prints a test page and returns its job id.
    pub async fn print_test_page(&self, printer_id: &str) -> Answer<i32> {
        match self {
            Self::Daemon => Ok(self.client().await?.print_test_page(printer_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.print_test_page(printer_id).await?),
        }
    }

    /// Holds a job.
    pub async fn pause_job(&self, printer_id: &str, job_id: i32) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self.client().await?.pause_job(printer_id, job_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.pause_job(printer_id, job_id).await?),
        }
    }

    /// Releases a held job.
    pub async fn resume_job(&self, printer_id: &str, job_id: i32) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self.client().await?.resume_job(printer_id, job_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.resume_job(printer_id, job_id).await?),
        }
    }

    /// Cancels a job.
    pub async fn cancel_job(&self, printer_id: &str, job_id: i32) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self.client().await?.cancel_job(printer_id, job_id).await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server.cancel_job(printer_id, job_id).await?),
        }
    }

    /// Moves a job to another printer.
    pub async fn move_job(
        &self,
        source_printer_id: &str,
        job_id: i32,
        destination_printer_id: &str,
    ) -> Answer<()> {
        match self {
            Self::Daemon => Ok(self
                .client()
                .await?
                .move_job(source_printer_id, job_id, destination_printer_id)
                .await?),
            #[cfg(feature = "embedded")]
            Self::Embedded(server) => Ok(server
                .move_job(source_printer_id, job_id, destination_printer_id)
                .await?),
        }
    }
}

#[cfg(feature = "embedded")]
fn job_filter(filter: JobFilter) -> &'static str {
    match filter {
        JobFilter::Active => "active",
        JobFilter::Completed => "completed",
        JobFilter::All => "all",
    }
}

/// Opens a printer's web interface with the desktop URL handler.
pub async fn open_printer_web_page(web_page: String) -> Result<(), String> {
    let status = tokio::process::Command::new("xdg-open")
        .arg(&web_page)
        .status()
        .await
        .map_err(|why| format!("failed to run xdg-open for {web_page}: {why}"))?;

    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("xdg-open exited with {status} for {web_page}"))
}

/// An update from the backend event feed.
#[derive(Clone, Debug)]
pub enum EventFeed {
    /// The feed reconnected after possibly missing events.
    Reconnected,
    /// A backend cache changed.
    Changed(PrintersEvent),
}

const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
const RETRY_DELAY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// Feeds events until the receiver closes, reconnecting to a daemon with bounded backoff.
pub async fn feed(backend: Backend, mut output: futures::channel::mpsc::Sender<EventFeed>) {
    match backend {
        #[cfg(feature = "embedded")]
        Backend::Embedded(server) => {
            use futures::{SinkExt, StreamExt};

            let mut events = server.watch_printers();
            if output.send(EventFeed::Reconnected).await.is_err() {
                return;
            }

            while let Some(event) = events.next().await {
                if output.send(EventFeed::Changed(event)).await.is_err() {
                    return;
                }
            }
        }
        Backend::Daemon => {
            let mut delay = RETRY_DELAY;

            loop {
                match stream_from_daemon(&mut output).await {
                    Listener::Gone => return,
                    // Reset backoff only after an event; accepting and immediately dropping a
                    // connection is still a continuing failure.
                    Listener::Waiting {
                        heard_something: true,
                    } => delay = RETRY_DELAY,
                    Listener::Waiting {
                        heard_something: false,
                    } => {}
                }

                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(RETRY_DELAY_MAX);
            }
        }
    }
}

enum Listener {
    Gone,
    Waiting { heard_something: bool },
}

async fn stream_from_daemon(output: &mut futures::channel::mpsc::Sender<EventFeed>) -> Listener {
    use futures::{SinkExt, StreamExt};

    let Ok(mut client) = cosmic_settings_printers_client::connect().await else {
        return Listener::Waiting {
            heard_something: false,
        };
    };
    let Ok(mut events) = client.printer_events().await else {
        return Listener::Waiting {
            heard_something: false,
        };
    };

    if output.send(EventFeed::Reconnected).await.is_err() {
        return Listener::Gone;
    }

    let mut heard_something = false;
    while let Some(event) = events.next().await {
        let Ok(event) = event else {
            tracing::warn!("printer event stream failed");
            break;
        };

        heard_something = true;
        if output.send(EventFeed::Changed(event)).await.is_err() {
            return Listener::Gone;
        }
    }

    Listener::Waiting { heard_something }
}
