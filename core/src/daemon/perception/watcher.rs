use crate::daemon::{DaemonEventBus, Result};
use async_trait::async_trait;
use crate::sync_primitives::Arc;
use tokio::sync::watch;

/// Watcher control signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherControl {
    Run,      // Normal operation
    Pause,    // Pause (Level 1 only)
    Shutdown, // Terminate
}

/// Watcher health status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatcherHealth {
    Healthy,
    Degraded(u32), // Error count
    Failed,
}

#[async_trait]
pub trait Watcher: Send + Sync {
    /// Unique identifier
    fn id(&self) -> &'static str;

    /// Whether this Watcher can be paused
    /// Level 0 (always-on) returns false
    /// Level 1 (pausable) returns true
    fn is_pausable(&self) -> bool {
        true
    }

    /// Main Watcher loop
    ///
    /// # Parameters
    /// - bus: EventBus for sending events
    /// - control: Control signal receiver for Pause/Shutdown
    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        control: watch::Receiver<WatcherControl>,
    ) -> Result<()>;

    /// Health check (optional)
    fn health(&self) -> WatcherHealth {
        WatcherHealth::Healthy
    }
}
