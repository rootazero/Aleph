use crate::daemon::{perception::{Watcher, WatcherControl}, DaemonEventBus, DaemonError, Result};
use std::collections::HashMap;
use crate::sync_primitives::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info};

pub struct WatcherRegistry {
    watchers: HashMap<String, Arc<dyn Watcher>>,
    handles: HashMap<String, JoinHandle<()>>,
    control_senders: HashMap<String, watch::Sender<WatcherControl>>,
}

impl WatcherRegistry {
    pub fn new() -> Self {
        Self {
            watchers: HashMap::new(),
            handles: HashMap::new(),
            control_senders: HashMap::new(),
        }
    }

    /// Register a Watcher
    pub fn register(&mut self, watcher: Arc<dyn Watcher>) {
        let id = watcher.id().to_string();
        info!("Registering watcher: {}", id);
        self.watchers.insert(id, watcher);
    }

    /// Get watcher count
    pub fn watcher_count(&self) -> usize {
        self.watchers.len()
    }

    /// Start all registered Watchers
    pub async fn start_all(&mut self, bus: Arc<DaemonEventBus>) -> Result<()> {
        for (id, watcher) in self.watchers.iter() {
            let (tx, rx) = watch::channel(WatcherControl::Run);

            let watcher_id = id.clone();
            let watcher_clone = Arc::clone(watcher);
            let bus_clone = bus.clone();

            let handle = tokio::spawn(async move {
                if let Err(e) = watcher_clone.run(bus_clone, rx).await {
                    error!("Watcher {} error: {}", watcher_id, e);
                }
            });

            self.control_senders.insert(id.clone(), tx);
            self.handles.insert(id.clone(), handle);
            info!("Started watcher: {}", id);
        }

        Ok(())
    }

    /// Pause a Watcher (Level 1 only)
    pub async fn pause_watcher(&self, id: &str) -> Result<()> {
        let watcher = self.watchers.get(id)
            .ok_or_else(|| DaemonError::Config(format!("Watcher not found: {}", id)))?;

        if !watcher.is_pausable() {
            return Err(DaemonError::Config(format!(
                "Watcher {} is Level 0 (always-on) and cannot be paused",
                id
            )));
        }

        if let Some(tx) = self.control_senders.get(id) {
            tx.send(WatcherControl::Pause)
                .map_err(|_| DaemonError::EventBus(format!("Failed to pause watcher {}", id)))?;
            info!("Paused watcher: {}", id);
        }

        Ok(())
    }

    /// Resume a paused Watcher
    pub async fn resume_watcher(&self, id: &str) -> Result<()> {
        if let Some(tx) = self.control_senders.get(id) {
            tx.send(WatcherControl::Run)
                .map_err(|_| DaemonError::EventBus(format!("Failed to resume watcher {}", id)))?;
            info!("Resumed watcher: {}", id);
        }

        Ok(())
    }

    /// Shutdown all Watchers gracefully
    pub async fn shutdown_all(&mut self) -> Result<()> {
        info!("Shutting down all watchers...");

        // Send shutdown signal to all watchers
        for (id, tx) in self.control_senders.iter() {
            if let Err(e) = tx.send(WatcherControl::Shutdown) {
                error!("Failed to send shutdown signal to {}: {}", id, e);
            }
        }

        // Wait for all tasks to complete
        for (id, handle) in self.handles.drain() {
            if let Err(e) = handle.await {
                error!("Watcher {} failed to shutdown cleanly: {}", id, e);
            }
        }

        self.control_senders.clear();
        info!("All watchers shut down");

        Ok(())
    }
}

impl Default for WatcherRegistry {
    fn default() -> Self {
        Self::new()
    }
}
