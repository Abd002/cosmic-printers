use cosmic_settings_printers_core::{PrintersEvent, PrintersEventKind};
use futures_util::{Stream, StreamExt};
use tokio::sync::broadcast;

use super::Server;

impl Server {
    /// Streams transport-independent printer and discovery changes.
    pub fn watch_printers(&self) -> impl Stream<Item = PrintersEvent> + Unpin + use<> {
        let receiver = self.context.subscribe_events();

        futures_util::stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => return Some((event, receiver)),
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "printer event receiver lagged");
                        return Some((
                            PrintersEvent {
                                kind: PrintersEventKind::AvailableDestinationsChanged,
                                printer_id: None,
                            },
                            receiver,
                        ));
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .boxed()
    }
}
