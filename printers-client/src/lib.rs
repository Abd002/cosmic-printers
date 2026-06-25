use zlink::Connection;

pub use cosmic_settings_printers_core::*;
use std::path::PathBuf;

pub async fn connect() -> zlink::Result<Client> {
    zlink::unix::connect(socket_path())
        .await
        .map(|conn| Client { conn })
}

pub struct Client {
    pub conn: Connection<zlink::unix::Stream>,
}

pub fn socket_path() -> PathBuf {
    dirs::runtime_dir()
        .expect("runtime dir required by varlink service")
        .join("com.system76.CosmicSettings")
}

#[zlink::proxy("com.system76.CosmicSettings.Printers")]
pub trait CosmicPrintersProxy {
    async fn list_printers(&mut self) -> zlink::Result<Result<ListPrintersReply, Error>>;

    async fn list_discovered_printers(
        &mut self,
    ) -> zlink::Result<Result<ListDiscoveredPrintersReply, Error>>;

    async fn add_discovered_printer(
        &mut self,
        printer_id: String,
    ) -> zlink::Result<Result<(), Error>>;

    async fn delete_printer(&mut self, printer_id: String) -> zlink::Result<Result<(), Error>>;

    async fn set_printer_accept_jobs(
        &mut self,
        printer_id: String,
        enabled: bool,
        reason: String,
    ) -> zlink::Result<Result<(), Error>>;

    async fn set_printer_default(&mut self, printer_id: String)
    -> zlink::Result<Result<(), Error>>;

    async fn set_printer_enabled(
        &mut self,
        printer_id: String,
        enabled: bool,
    ) -> zlink::Result<Result<(), Error>>;

    async fn set_printer_info(
        &mut self,
        printer_id: String,
        info: String,
    ) -> zlink::Result<Result<(), Error>>;

    async fn set_printer_location(
        &mut self,
        printer_id: String,
        location: String,
    ) -> zlink::Result<Result<(), Error>>;

    async fn set_printer_shared(
        &mut self,
        printer_id: String,
        shared: bool,
    ) -> zlink::Result<Result<(), Error>>;

    async fn print_test_page(
        &mut self,
        printer_id: String,
    ) -> zlink::Result<Result<PrintTestPageReply, Error>>;

    async fn get_jobs(
        &mut self,
        printer_id: String,
        filter: String,
    ) -> zlink::Result<Result<GetJobsReply, Error>>;

    async fn pause_job(
        &mut self,
        printer_id: String,
        job_id: i32,
    ) -> zlink::Result<Result<(), Error>>;

    async fn resume_job(
        &mut self,
        printer_id: String,
        job_id: i32,
    ) -> zlink::Result<Result<(), Error>>;

    async fn cancel_job(
        &mut self,
        printer_id: String,
        job_id: i32,
    ) -> zlink::Result<Result<(), Error>>;
}
