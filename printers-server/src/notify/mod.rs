//! Hearing about job and printer changes instead of enumerating to find them.

mod apply;
mod events;
mod subscription;

use crate::state::State;

const RESUBSCRIBE_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

const ATTEMPTS: usize = 3;

pub(crate) fn start(context: State) {
    let Some(lease) = context.try_start_notifications() else {
        return;
    };

    tokio::spawn(async move {
        let _lease = lease;
        for _ in 0..ATTEMPTS {
            if let Err(error) = events::watch(&context).await {
                tracing::warn!(error = ?error, "IPP notifications stopped");
            }
            tokio::time::sleep(RESUBSCRIBE_DELAY).await;
        }
    });
}
