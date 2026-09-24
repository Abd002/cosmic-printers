//! Hearing about job and printer changes instead of enumerating to find them.

mod apply;
mod events;
mod subscription;
pub(crate) mod toast;

use std::time::{Duration, Instant};

use crate::state::State;

const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

const MAX_RESUBSCRIBE_DELAY: Duration = Duration::from_secs(300);

const HEALTHY: Duration = Duration::from_secs(60);

pub(crate) fn start(context: State) {
    let Some(lease) = context.try_start_notifications() else {
        return;
    };

    tokio::spawn(async move {
        let _lease = lease;
        let mut delay = RESUBSCRIBE_DELAY;

        loop {
            let started = Instant::now();
            if let Err(error) = events::watch(&context).await {
                tracing::warn!(error = ?error, "IPP notifications stopped");
            }

            if started.elapsed() >= HEALTHY {
                delay = RESUBSCRIBE_DELAY;
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RESUBSCRIBE_DELAY);
        }
    });
}
