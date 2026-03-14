//! Service runtime state container.
//!
//! Holds the shared store, clock, config, and lifecycle flags.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::cron::clock::Clock;
use crate::cron::config::CronConfig;
use crate::cron::store::CronStore;

/// Runtime state for the cron service.
///
/// Generic over `C: Clock` to allow deterministic testing with `FakeClock`.
/// The store is behind a `tokio::sync::Mutex` because `persist()` does file I/O.
pub struct ServiceState<C: Clock> {
    pub store: Arc<tokio::sync::Mutex<CronStore>>,
    pub clock: Arc<C>,
    pub config: CronConfig,
    is_running: AtomicBool,
    shutdown: AtomicBool,
}

impl<C: Clock> ServiceState<C> {
    /// Create a new service state.
    pub fn new(
        store: Arc<tokio::sync::Mutex<CronStore>>,
        clock: Arc<C>,
        config: CronConfig,
    ) -> Self {
        Self {
            store,
            clock,
            config,
            is_running: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Whether the service is currently running.
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }

    /// Set the running state.
    pub fn set_running(&self, running: bool) {
        self.is_running.store(running, Ordering::SeqCst);
    }

    /// Whether a shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Request a graceful shutdown.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::clock::testing::FakeClock;
    use tempfile::TempDir;

    fn make_state() -> ServiceState<FakeClock> {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.json");
        let store = CronStore::load(path).unwrap();
        ServiceState::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            Arc::new(FakeClock::new(1_000_000)),
            CronConfig::default(),
        )
    }

    #[test]
    fn initial_state() {
        let state = make_state();
        assert!(!state.is_running());
        assert!(!state.is_shutdown());
    }

    #[test]
    fn set_running_flag() {
        let state = make_state();
        state.set_running(true);
        assert!(state.is_running());
        state.set_running(false);
        assert!(!state.is_running());
    }

    #[test]
    fn request_shutdown_flag() {
        let state = make_state();
        state.request_shutdown();
        assert!(state.is_shutdown());
    }
}
