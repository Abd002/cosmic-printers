use cosmic_settings_printers_core::Error;
use std::future::Future;
use zbus::{Connection, proxy};

#[proxy(
    interface = "org.opensuse.CupsPkHelper.Mechanism",
    default_service = "org.opensuse.CupsPkHelper.Mechanism",
    default_path = "/"
)]
trait CupsPkHelper {
    async fn printer_delete(&self, name: &str) -> zbus::Result<String>;
    async fn printer_set_accept_jobs(
        &self,
        name: &str,
        enabled: bool,
        reason: &str,
    ) -> zbus::Result<String>;
    async fn printer_set_default(&self, name: &str) -> zbus::Result<String>;
    async fn printer_set_enabled(&self, name: &str, enabled: bool) -> zbus::Result<String>;
    async fn printer_set_info(&self, name: &str, info: &str) -> zbus::Result<String>;
    async fn printer_set_location(&self, name: &str, location: &str) -> zbus::Result<String>;
    async fn printer_set_shared(&self, name: &str, shared: bool) -> zbus::Result<String>;
}

pub(super) async fn delete_printer(name: &str) -> Result<(), Error> {
    with_proxy("PrinterDelete", |proxy| async move {
        proxy.printer_delete(name).await
    })
    .await
}

pub(super) async fn set_printer_accept_jobs(
    name: &str,
    enabled: bool,
    reason: &str,
) -> Result<(), Error> {
    with_proxy("PrinterSetAcceptJobs", |proxy| async move {
        proxy.printer_set_accept_jobs(name, enabled, reason).await
    })
    .await
}

pub(super) async fn set_printer_default(name: &str) -> Result<(), Error> {
    with_proxy("PrinterSetDefault", |proxy| async move {
        proxy.printer_set_default(name).await
    })
    .await
}

pub(super) async fn set_printer_enabled(name: &str, enabled: bool) -> Result<(), Error> {
    with_proxy("PrinterSetEnabled", |proxy| async move {
        proxy.printer_set_enabled(name, enabled).await
    })
    .await
}

pub(super) async fn set_printer_info(name: &str, info: &str) -> Result<(), Error> {
    with_proxy("PrinterSetInfo", |proxy| async move {
        proxy.printer_set_info(name, info).await
    })
    .await
}

pub(super) async fn set_printer_location(name: &str, location: &str) -> Result<(), Error> {
    with_proxy("PrinterSetLocation", |proxy| async move {
        proxy.printer_set_location(name, location).await
    })
    .await
}

pub(super) async fn set_printer_shared(name: &str, shared: bool) -> Result<(), Error> {
    with_proxy("PrinterSetShared", |proxy| async move {
        proxy.printer_set_shared(name, shared).await
    })
    .await
}

async fn with_proxy<'a, F, Fut>(operation: &'static str, call: F) -> Result<(), Error>
where
    F: FnOnce(CupsPkHelperProxy<'a>) -> Fut,
    Fut: Future<Output = zbus::Result<String>>,
{
    let connection = Connection::system().await.map_err(helper_err)?;
    let proxy = CupsPkHelperProxy::new(&connection)
        .await
        .map_err(helper_err)?;

    match call(proxy).await {
        Ok(error) if error.is_empty() => Ok(()),
        Ok(error) => Err(Error::CupsFailed { why: error }),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with(".NotPrivileged") => {
            Err(Error::PermissionDenied {
                operation: operation.into(),
            })
        }
        Err(error) => Err(helper_err(error)),
    }
}

fn helper_err(error: impl std::fmt::Display) -> Error {
    Error::CupsFailed {
        why: format!("cups-pk-helper: {error}"),
    }
}
