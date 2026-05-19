//! Service runtime state container.
//!
//! Holds the shared store, config, and lifecycle flags.

use crate::sync_primitives::{Arc, AtomicBool, Ordering};
use crate::tasks::heartbeat::config::HeartbeatConfig;
use crate::tasks::heartbeat::store::HeartbeatStore;

/// Runtime state for the heartbeat service.
///
/// The store is behind a `tokio::sync::Mutex` because `persist()` does file I/O.
pub struct HeartbeatServiceState {
    pub store: Arc<tokio::sync::Mutex<HeartbeatStore>>,
    pub config: HeartbeatConfig,
    shutdown: AtomicBool,
}

impl HeartbeatServiceState {
    /// Create a new service state.
    pub fn new(store: Arc<tokio::sync::Mutex<HeartbeatStore>>, config: HeartbeatConfig) -> Self {
        Self {
            store,
            config,
            shutdown: AtomicBool::new(false),
        }
    }

    /// Whether a shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// Request a graceful shutdown.
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> HeartbeatServiceState {
        let store = HeartbeatStore::open_in_memory().unwrap();
        HeartbeatServiceState::new(
            Arc::new(tokio::sync::Mutex::new(store)),
            HeartbeatConfig::default(),
        )
    }

    #[test]
    fn initial_state() {
        let state = make_state();
        assert!(!state.is_shutdown());
    }

    #[test]
    fn request_shutdown_flag() {
        let state = make_state();
        state.request_shutdown();
        assert!(state.is_shutdown());
    }
}
