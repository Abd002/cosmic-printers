//! What is allowed to run at once, and what has to take its turn.
//! These leases stay outside the model lock so network waits never block readers.

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use super::State;

/// How many Printer Applications to ask for devices at once.
pub(super) const DEFAULT_SCAN_CONCURRENCY: usize = 4;

/// Locks created on demand, one per key.
pub(super) type KeyedLocks = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>;

pub(crate) struct DiscoveryLease {
    running: Arc<AtomicBool>,
}

impl Drop for DiscoveryLease {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

impl State {
    pub(crate) fn try_start_printer_application_discovery(&self) -> Option<DiscoveryLease> {
        self.discovery_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DiscoveryLease {
                running: Arc::clone(&self.discovery_running),
            })
    }

    pub(crate) fn try_start_available_destinations_refresh(&self) -> Option<DiscoveryLease> {
        self.available_destinations_refresh_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DiscoveryLease {
                running: Arc::clone(&self.available_destinations_refresh_running),
            })
    }

    /// Acquires a slot to run a device scan in.
    pub(crate) async fn acquire_scan_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(&self.pa_scan_semaphore)
            .acquire_owned()
            .await
            .expect("scan semaphore is never closed")
    }

    /// Returns the lock that serializes scans of one Printer Application.
    pub(crate) fn scan_lock(&self, application_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        keyed_lock(&self.pa_scan_locks, application_id)
    }

    /// Serializes configuration for one device and Printer Application pair.
    pub(crate) fn configuration_lock(
        &self,
        application_id: &str,
        device_uri: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        keyed_lock(
            &self.pa_configuration_locks,
            &format!("{application_id}\u{1}{device_uri}"),
        )
    }
}

/// Returns a lock for a key, creating it on first use.
fn keyed_lock(locks: &KeyedLocks, key: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    Arc::clone(locks.entry(key.to_string()).or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_lease_releases_on_drop() {
        let context = State::new();
        let lease = context.try_start_printer_application_discovery().unwrap();
        assert!(context.try_start_printer_application_discovery().is_none());

        drop(lease);

        assert!(context.try_start_printer_application_discovery().is_some());
    }

    #[test]
    fn destination_refresh_lease_prevents_parallel_enumerations() {
        let context = State::new();
        let lease = context.try_start_available_destinations_refresh().unwrap();
        assert!(context.try_start_available_destinations_refresh().is_none());

        drop(lease);

        assert!(context.try_start_available_destinations_refresh().is_some());
    }
}
