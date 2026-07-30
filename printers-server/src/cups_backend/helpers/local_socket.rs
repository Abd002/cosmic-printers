use crate::error::{BackendError, BackendResult};
use crate::ipp::CupsResultExt;
use std::sync::{Mutex, MutexGuard, OnceLock};

const LOCAL_CUPS_SOCKET: &str = "/run/cups/cups.sock";

fn cups_server_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(in crate::cups_backend) struct LocalSocketGuard {
    previous: String,
    restored: bool,
    _lock: MutexGuard<'static, ()>,
}

impl LocalSocketGuard {
    pub(in crate::cups_backend) fn engage() -> BackendResult<Self> {
        let lock = cups_server_lock()
            .lock()
            .map_err(|_| BackendError::Internal("CUPS server lock was poisoned".to_string()))?;
        let previous = cups_rs::config::get_server();
        cups_rs::config::set_server(Some(LOCAL_CUPS_SOCKET)).cups_err()?;
        Ok(Self {
            previous,
            restored: false,
            _lock: lock,
        })
    }

    pub(in crate::cups_backend) fn restore(mut self) -> BackendResult<()> {
        cups_rs::config::set_server(Some(&self.previous)).cups_err()?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for LocalSocketGuard {
    fn drop(&mut self) {
        if !self.restored {
            let _ = cups_rs::config::set_server(Some(&self.previous));
        }
    }
}
