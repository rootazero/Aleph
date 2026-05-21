//! Perception test context

use alephcore::daemon::{
    perception::{PerceptionConfig, WatcherRegistry},
    DaemonEventBus,
};
use std::sync::Arc;

/// Perception test context for BDD tests
/// Note: Cannot derive Debug because WatcherRegistry doesn't implement Debug
#[derive(Default)]
pub struct PerceptionContext {
    pub config: Option<PerceptionConfig>,
    pub config_toml: Option<String>,
    pub registry: Option<WatcherRegistry>,
    pub event_bus: Option<Arc<DaemonEventBus>>,
    pub watcher_count: Option<usize>,
    /// Watcher info for assertions: (id, is_pausable)
    pub watcher_info: Option<(String, bool)>,
}

impl std::fmt::Debug for PerceptionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerceptionContext")
            .field("config", &self.config)
            .field("config_toml", &self.config_toml.as_ref().map(|_| "<toml>"))
            .field("registry", &self.registry.as_ref().map(|r| format!("<WatcherRegistry: {} watchers>", r.watcher_count())))
            .field("event_bus", &self.event_bus.as_ref().map(|_| "<DaemonEventBus>"))
            .field("watcher_count", &self.watcher_count)
            .field("watcher_info", &self.watcher_info)
            .finish()
    }
}
