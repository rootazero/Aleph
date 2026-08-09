//! Service runtime state container.
//!
//! Holds the shared store, clock, config, and lifecycle flags.

use crate::sync_primitives::{Arc, AtomicBool, AtomicI64, Ordering};
use crate::tasks::cron::config::CronConfig;
use crate::tasks::cron::store::CronStore;
use crate::tasks::shared::clock::Clock;

/// Runtime state for the cron service.
///
/// Generic over `C: Clock` to allow deterministic testing with `FakeClock`.
/// The store is behind a `tokio::sync::Mutex` because `persist()` does file I/O.
pub struct ServiceState<C: Clock> {
    pub store: Arc<tokio::sync::Mutex<CronStore>>,
    pub clock: Arc<C>,
    pub config: CronConfig,
    last_tick_at_ms: AtomicI64,
    shutdown: AtomicBool,
}

impl<C: Clock> ServiceState<C> {
    /// Create a new service state.
    pub const fn new(
        store: Arc<tokio::sync::Mutex<CronStore>>,
        clock: Arc<C>,
        config: CronConfig,
    ) -> Self {
        Self {
            store,
            clock,
            config,
            last_tick_at_ms: AtomicI64::new(0),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Wall-clock ms of the scheduler's last completed due-scan, or `0` when it
    /// has never scanned.
    ///
    /// This is the **only** evidence that the timer loop is alive. It exists
    /// because `cron.status` used to answer `running: true` unconditionally
    /// while the loop's startup is conditional (no execution adapter ⇒ every
    /// `cron.*` handler is still registered but nothing is ever scheduled). A
    /// constant `true` decides nothing, and the lie is only visible to the
    /// operator on the other side.
    ///
    /// Written by `run_timer_loop` after each scan; read by the `cron.status`
    /// handler. One writer, one reader — not an abstraction held open for a
    /// future consumer.
    pub fn last_tick_at_ms(&self) -> i64 {
        self.last_tick_at_ms.load(Ordering::Relaxed)
    }

    /// Record that a due-scan just completed. See [`Self::last_tick_at_ms`].
    pub fn mark_tick(&self, now_ms: i64) {
        self.last_tick_at_ms.store(now_ms, Ordering::Relaxed);
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
    use crate::sync_primitives::Arc;
    use crate::tasks::cron::clock::testing::FakeClock;
    use tempfile::TempDir;

    fn make_state() -> ServiceState<FakeClock> {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cron.db");
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
        assert_eq!(
            state.last_tick_at_ms(),
            0,
            "a service that has never scanned must not look alive"
        );
        assert!(!state.is_shutdown());
    }

    #[test]
    fn mark_tick_records_the_scan_time() {
        let state = make_state();
        state.mark_tick(1_700_000_000_000);
        assert_eq!(state.last_tick_at_ms(), 1_700_000_000_000);
    }

    #[test]
    fn request_shutdown_flag() {
        let state = make_state();
        state.request_shutdown();
        assert!(state.is_shutdown());
    }
}
