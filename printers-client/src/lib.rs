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

    async fn set_printer_default(
        &mut self,
        id: String,
        password: String,
    ) -> zlink::Result<Result<(), Error>>;
}
