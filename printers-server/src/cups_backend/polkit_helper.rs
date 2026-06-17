use cosmic_settings_printers_core::Error;
use zbus::{Connection, proxy};

#[proxy(
    interface = "org.opensuse.CupsPkHelper.Mechanism",
    default_service = "org.opensuse.CupsPkHelper.Mechanism",
    default_path = "/"
)]
trait CupsPkHelper {
    async fn printer_set_default(&self, name: &str) -> zbus::Result<String>;
}

pub(super) async fn set_default(queue_name: String) -> Result<(), Error> {
    let connection = Connection::system().await.map_err(helper_err)?;
    let proxy = CupsPkHelperProxy::new(&connection)
        .await
        .map_err(helper_err)?;

    match proxy.printer_set_default(&queue_name).await {
        Ok(error) if error.is_empty() => Ok(()),
        Ok(error) => Err(Error::CupsFailed { why: error }),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with(".NotPrivileged") => {
            Err(Error::PermissionDenied {
                operation: "CUPS-Set-Default".into(),
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
