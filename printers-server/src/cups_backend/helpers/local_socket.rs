use crate::ipp::CupsResultExt;
use cosmic_settings_printers_core::Error;

const LOCAL_CUPS_SOCKET: &str = "/run/cups/cups.sock";

pub(in crate::cups_backend) struct LocalSocketGuard {
    previous: String,
}

impl LocalSocketGuard {
    pub(in crate::cups_backend) fn engage() -> Result<Self, Error> {
        let previous = cups_rs::config::get_server();
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).cups_err()?;
        Ok(Self { previous })
    }
}

impl Drop for LocalSocketGuard {
    fn drop(&mut self) {
        let _ = cups_rs::config::set_server(Some(&self.previous));
    }
}
