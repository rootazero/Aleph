# Phase 2: Perception Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Implement Aleph's Perception Layer with 4 Watchers (Process, File, Time, System) that monitor OS-level events and publish them to a dedicated EventBus.

**Architecture:** Build an event-driven sensing system with configurable Watchers, using tokio::broadcast for event distribution and tokio::sync::watch for lifecycle control. Level 0 Watchers (always-on) and Level 1 Watchers (pausable) enable adaptive resource management.

**Tech Stack:** Rust + Tokio (async), notify (file watching), sysinfo (system monitoring), battery (power status), user-idle (idle detection), tokio::broadcast (event bus)

---

## Task 1: Event System Foundation

**Files:**
- Create: `core/src/daemon/events.rs`
- Create: `core/src/daemon/event_bus.rs`
- Modify: `core/src/daemon/mod.rs`
- Test: `core/src/daemon/tests/event_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/tests/event_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::{DaemonEvent, DaemonEventBus, RawEvent};
    use chrono::Utc;

    #[tokio::test]
    async fn test_event_bus_send_and_receive() {
        let bus = DaemonEventBus::new(10);
        let mut receiver = bus.subscribe();

        let event = DaemonEvent::Raw(RawEvent::Heartbeat {
            timestamp: Utc::now(),
        });

        bus.send(event.clone()).unwrap();
        let received = receiver.recv().await.unwrap();

        assert!(matches!(received, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));
    }

    #[tokio::test]
    async fn test_event_bus_multiple_subscribers() {
        let bus = DaemonEventBus::new(10);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        let event = DaemonEvent::Raw(RawEvent::Heartbeat {
            timestamp: Utc::now(),
        });

        bus.send(event.clone()).unwrap();

        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();

        assert!(matches!(r1, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));
        assert!(matches!(r2, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib event_tests`
Expected: FAIL with "no module named `events`" or similar

**Step 3: Implement DaemonEvent types**

Create `core/src/daemon/events.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Top-level daemon event enum
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonEvent {
    /// Raw events from Watchers (Level 1: Direct OS observations)
    Raw(RawEvent),

    /// Derived events from WorldModel (Level 2: Inferred context)
    Derived(DerivedEvent),

    /// System control events
    System(SystemEvent),
}

/// Raw OS-level events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RawEvent {
    // ProcessWatcher
    ProcessDetected {
        name: String,
        pid: u32,
        cpu_usage: f32,
        memory: u64,
    },
    ProcessTerminated {
        name: String,
        pid: u32,
    },

    // FSEventWatcher
    FileChanged {
        path: PathBuf,
        kind: FileChangeKind,
    },

    // TimeWatcher
    Heartbeat {
        timestamp: DateTime<Utc>,
    },

    // SystemStateWatcher
    SystemState {
        battery_percent: Option<f32>,
        is_idle: bool,
        network_online: bool,
        cpu_usage: f32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FileChangeKind {
    Created,
    Modified,
    Removed,
}

/// Derived high-level events (from WorldModel - Phase 3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DerivedEvent {
    UserActivityChanged {
        previous: Activity,
        current: Activity,
    },
    ResourceConstraintDetected {
        constraint: ResourceConstraint,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Activity {
    Coding,
    Meeting,
    Browsing,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResourceConstraint {
    LowBattery,
    HighCpu,
    LowMemory,
}

/// System control events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    Shutdown,
    ConfigReloaded,
    WatcherPaused(String),
    WatcherResumed(String),
}
```

**Step 4: Implement DaemonEventBus**

Create `core/src/daemon/event_bus.rs`:

```rust
use crate::daemon::{DaemonEvent, DaemonError, Result};
use tokio::sync::broadcast;

pub struct DaemonEventBus {
    sender: broadcast::Sender<DaemonEvent>,
}

impl DaemonEventBus {
    /// Create new EventBus with specified capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Send event to all subscribers
    pub fn send(&self, event: DaemonEvent) -> Result<()> {
        self.sender
            .send(event)
            .map(|_| ())
            .map_err(|_| DaemonError::EventBus("No active subscribers".into()))
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.sender.subscribe()
    }

    /// Get current subscriber count
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}
```

**Step 5: Update DaemonError**

Modify `core/src/daemon/error.rs`:

Add new variant to `DaemonError` enum:

```rust
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    // ... existing variants ...

    #[error("EventBus error: {0}")]
    EventBus(String),
}
```

**Step 6: Update module exports**

Modify `core/src/daemon/mod.rs`:

```rust
pub mod cli;
pub mod error;
pub mod event_bus;     // NEW
pub mod events;        // NEW
pub mod ipc;
pub mod resource_governor;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

pub use cli::{DaemonCli, DaemonCommand};
pub use error::{DaemonError, Result};
pub use event_bus::DaemonEventBus;  // NEW
pub use events::{Activity, DaemonEvent, DerivedEvent, FileChangeKind, RawEvent, ResourceConstraint, SystemEvent};  // NEW
pub use ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{create_service_manager, ServiceManager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
```

**Step 7: Update test module**

Modify `core/src/daemon/tests/mod.rs` to include:

```rust
mod event_tests;
```

**Step 8: Run tests to verify they pass**

Run: `cargo test --lib daemon::tests::event_tests`
Expected: PASS (2 tests)

**Step 9: Commit**

```bash
git add core/src/daemon/events.rs core/src/daemon/event_bus.rs core/src/daemon/error.rs core/src/daemon/mod.rs core/src/daemon/tests/event_tests.rs core/src/daemon/tests/mod.rs
git commit -m "feat(daemon): add event system foundation

- Add DaemonEvent enum with Raw/Derived/System variants
- Implement DaemonEventBus using tokio::broadcast
- Add comprehensive event type definitions
- Tests: event bus send/receive and multi-subscriber"
```

---

## Task 2: Configuration System

**Files:**
- Create: `core/src/daemon/perception/mod.rs`
- Create: `core/src/daemon/perception/config.rs`
- Modify: `core/src/daemon/mod.rs`
- Modify: `core/Cargo.toml`
- Test: `core/src/daemon/perception/tests/config_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/perception/tests/config_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::perception::PerceptionConfig;
    use std::path::PathBuf;

    #[test]
    fn test_default_config() {
        let config = PerceptionConfig::default();

        assert!(config.enabled);
        assert!(config.process.enabled);
        assert_eq!(config.process.poll_interval_secs, 5);
        assert!(config.process.watched_apps.contains(&"Code".to_string()));

        assert!(config.filesystem.enabled);
        assert_eq!(config.filesystem.debounce_ms, 500);

        assert!(config.time.enabled);
        assert_eq!(config.time.heartbeat_interval_secs, 30);

        assert!(config.system.enabled);
        assert_eq!(config.system.poll_interval_secs, 60);
        assert!(config.system.track_battery);
    }

    #[test]
    fn test_config_serialization() {
        let config = PerceptionConfig::default();
        let toml_str = toml::to_string(&config).unwrap();

        assert!(toml_str.contains("enabled = true"));
        assert!(toml_str.contains("[process]"));
        assert!(toml_str.contains("[filesystem]"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib perception::tests::config_tests`
Expected: FAIL with "no module named `perception`"

**Step 3: Add dependencies**

Modify `core/Cargo.toml`, add to dependencies section:

```toml
# Perception Layer dependencies
notify = "6.1"
notify-debouncer-full = "0.3"
user-idle = "0.5"
shellexpand = "3.1"
```

**Step 4: Implement PerceptionConfig**

Create `core/src/daemon/perception/config.rs`:

```rust
use crate::daemon::{DaemonError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionConfig {
    pub enabled: bool,
    pub process: ProcessWatcherConfig,
    pub filesystem: FSWatcherConfig,
    pub time: TimeWatcherConfig,
    pub system: SystemWatcherConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessWatcherConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub watched_apps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FSWatcherConfig {
    pub enabled: bool,
    pub watched_paths: Vec<String>,
    pub ignore_patterns: Vec<String>,
    pub debounce_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeWatcherConfig {
    pub enabled: bool,
    pub heartbeat_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemWatcherConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub track_battery: bool,
    pub track_network: bool,
    pub idle_threshold_secs: u64,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            process: ProcessWatcherConfig {
                enabled: true,
                poll_interval_secs: 5,
                watched_apps: vec![
                    "Code".to_string(),
                    "Google Chrome".to_string(),
                    "Zoom".to_string(),
                    "Slack".to_string(),
                    "Terminal".to_string(),
                ],
            },
            filesystem: FSWatcherConfig {
                enabled: true,
                watched_paths: vec!["~/Downloads".to_string(), "~/Desktop".to_string()],
                ignore_patterns: vec![
                    "**/.git/**".to_string(),
                    "**/node_modules/**".to_string(),
                    "**/target/**".to_string(),
                    "**/.DS_Store".to_string(),
                ],
                debounce_ms: 500,
            },
            time: TimeWatcherConfig {
                enabled: true,
                heartbeat_interval_secs: 30,
            },
            system: SystemWatcherConfig {
                enabled: true,
                poll_interval_secs: 60,
                track_battery: true,
                track_network: true,
                idle_threshold_secs: 300,
            },
        }
    }
}

impl PerceptionConfig {
    /// Load configuration from ~/.aleph/perception.toml
    pub fn load() -> Result<Self> {
        let path = dirs::home_dir()
            .ok_or_else(|| DaemonError::Config("HOME environment variable not set".into()))?
            .join(".aether/perception.toml");

        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| DaemonError::Io(e))?;
            toml::from_str(&content)
                .map_err(|e| DaemonError::Config(format!("Invalid TOML: {}", e)))
        } else {
            Ok(Self::default())
        }
    }

    /// Expand tilde in filesystem paths
    pub fn expand_paths(&mut self) -> Result<()> {
        self.filesystem.watched_paths = self
            .filesystem
            .watched_paths
            .iter()
            .map(|p| {
                shellexpand::tilde(p).to_string()
            })
            .collect();
        Ok(())
    }
}
```

**Step 5: Create perception module**

Create `core/src/daemon/perception/mod.rs`:

```rust
pub mod config;

#[cfg(test)]
mod tests;

pub use config::{
    FSWatcherConfig, PerceptionConfig, ProcessWatcherConfig, SystemWatcherConfig,
    TimeWatcherConfig,
};
```

Create `core/src/daemon/perception/tests/mod.rs`:

```rust
mod config_tests;
```

**Step 6: Update daemon module exports**

Modify `core/src/daemon/mod.rs`:

```rust
pub mod cli;
pub mod error;
pub mod event_bus;
pub mod events;
pub mod ipc;
pub mod perception;    // NEW
pub mod resource_governor;
pub mod service_manager;
pub mod types;

#[cfg(target_os = "macos")]
pub mod platforms;

pub use cli::{DaemonCli, DaemonCommand};
pub use error::{DaemonError, Result};
pub use event_bus::DaemonEventBus;
pub use events::{Activity, DaemonEvent, DerivedEvent, FileChangeKind, RawEvent, ResourceConstraint, SystemEvent};
pub use ipc::{IpcServer, JsonRpcRequest, JsonRpcResponse};
pub use perception::PerceptionConfig;  // NEW
pub use resource_governor::{GovernorDecision, ResourceGovernor, ResourceLimits};
pub use service_manager::{create_service_manager, ServiceManager};
pub use types::{DaemonConfig, DaemonStatus, ServiceStatus};
```

**Step 7: Run tests to verify they pass**

Run: `cargo test --lib perception::tests::config_tests`
Expected: PASS (2 tests)

**Step 8: Commit**

```bash
git add core/Cargo.toml core/src/daemon/perception/ core/src/daemon/mod.rs
git commit -m "feat(daemon): add perception configuration system

- Add PerceptionConfig with sub-configs for all watchers
- Support TOML serialization/deserialization
- Load from ~/.aleph/perception.toml with fallback to defaults
- Path expansion support for ~ in filesystem paths"
```

---

## Task 3: Watcher Trait and Registry

**Files:**
- Create: `core/src/daemon/perception/watcher.rs`
- Create: `core/src/daemon/perception/registry.rs`
- Modify: `core/src/daemon/perception/mod.rs`
- Test: `core/src/daemon/perception/tests/registry_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/perception/tests/registry_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::{DaemonEventBus, perception::{WatcherRegistry, WatcherControl, WatcherHealth}};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_registry_lifecycle() {
        let mut registry = WatcherRegistry::new();
        let bus = Arc::new(DaemonEventBus::new(10));

        // Registry starts empty
        assert_eq!(registry.watcher_count(), 0);

        // Start and shutdown without watchers should work
        registry.start_all(bus.clone()).await.unwrap();
        registry.shutdown_all().await.unwrap();
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib perception::tests::registry_tests`
Expected: FAIL with "no type named `WatcherRegistry`"

**Step 3: Implement Watcher trait**

Create `core/src/daemon/perception/watcher.rs`:

```rust
use crate::daemon::{DaemonEventBus, Result};
use async_trait::async_trait;
use std::sync::Arc;
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
```

**Step 4: Implement WatcherRegistry**

Create `core/src/daemon/perception/registry.rs`:

```rust
use crate::daemon::{perception::{Watcher, WatcherControl}, DaemonEventBus, DaemonError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info};

pub struct WatcherRegistry {
    watchers: HashMap<String, Box<dyn Watcher>>,
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
    pub fn register(&mut self, watcher: Box<dyn Watcher>) {
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
            let watcher_ref: &dyn Watcher = &**watcher;

            // Clone necessary items for the task
            let bus_clone = bus.clone();
            let watcher_ptr = watcher_ref as *const dyn Watcher;

            let handle = tokio::spawn(async move {
                // SAFETY: Watcher lives for the entire program lifetime
                let watcher = unsafe { &*watcher_ptr };

                if let Err(e) = watcher.run(bus_clone, rx).await {
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
```

**Step 5: Update perception module exports**

Modify `core/src/daemon/perception/mod.rs`:

```rust
pub mod config;
pub mod registry;
pub mod watcher;

#[cfg(test)]
mod tests;

pub use config::{
    FSWatcherConfig, PerceptionConfig, ProcessWatcherConfig, SystemWatcherConfig,
    TimeWatcherConfig,
};
pub use registry::WatcherRegistry;
pub use watcher::{Watcher, WatcherControl, WatcherHealth};
```

Update `core/src/daemon/perception/tests/mod.rs`:

```rust
mod config_tests;
mod registry_tests;
```

**Step 6: Run tests to verify they pass**

Run: `cargo test --lib perception::tests::registry_tests`
Expected: PASS (1 test)

**Step 7: Commit**

```bash
git add core/src/daemon/perception/watcher.rs core/src/daemon/perception/registry.rs core/src/daemon/perception/mod.rs core/src/daemon/perception/tests/
git commit -m "feat(daemon): add watcher trait and registry

- Define Watcher trait with lifecycle hooks
- Implement WatcherRegistry for centralized management
- Support Level 0/1 tiering (pausable vs always-on)
- Lifecycle control via tokio::sync::watch channels"
```

---

## Task 4: TimeWatcher Implementation

**Files:**
- Create: `core/src/daemon/perception/watchers/mod.rs`
- Create: `core/src/daemon/perception/watchers/time.rs`
- Modify: `core/src/daemon/perception/mod.rs`
- Test: `core/src/daemon/perception/watchers/tests/time_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/perception/watchers/tests/time_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::{
        DaemonEvent, DaemonEventBus, RawEvent,
        perception::{TimeWatcher, TimeWatcherConfig, Watcher, WatcherControl},
    };
    use std::sync::Arc;
    use tokio::sync::watch;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    async fn test_time_watcher_heartbeat() {
        let config = TimeWatcherConfig {
            enabled: true,
            heartbeat_interval_secs: 1, // Fast for testing
        };

        let watcher = TimeWatcher::new(config);
        let bus = Arc::new(DaemonEventBus::new(10));
        let mut receiver = bus.subscribe();

        let (tx, rx) = watch::channel(WatcherControl::Run);

        // Start watcher in background
        let watcher_task = tokio::spawn({
            let bus = bus.clone();
            async move {
                watcher.run(bus, rx).await
            }
        });

        // Wait for first heartbeat
        let result = timeout(Duration::from_secs(2), receiver.recv()).await;
        assert!(result.is_ok());

        let event = result.unwrap().unwrap();
        assert!(matches!(event, DaemonEvent::Raw(RawEvent::Heartbeat { .. })));

        // Shutdown
        tx.send(WatcherControl::Shutdown).unwrap();
        let _ = watcher_task.await;
    }

    #[tokio::test]
    async fn test_time_watcher_is_not_pausable() {
        let config = TimeWatcherConfig {
            enabled: true,
            heartbeat_interval_secs: 30,
        };

        let watcher = TimeWatcher::new(config);
        assert_eq!(watcher.id(), "time");
        assert!(!watcher.is_pausable()); // Level 0
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib watchers::tests::time_tests`
Expected: FAIL with "no type named `TimeWatcher`"

**Step 3: Implement TimeWatcher**

Create `core/src/daemon/perception/watchers/time.rs`:

```rust
use crate::daemon::{
    DaemonEvent, DaemonEventBus, RawEvent, Result,
    perception::{TimeWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

pub struct TimeWatcher {
    config: TimeWatcherConfig,
}

impl TimeWatcher {
    pub fn new(config: TimeWatcherConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Watcher for TimeWatcher {
    fn id(&self) -> &'static str {
        "time"
    }

    fn is_pausable(&self) -> bool {
        false // Level 0: always-on
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!("TimeWatcher started (heartbeat every {}s)", self.config.heartbeat_interval_secs);

        let mut ticker = interval(Duration::from_secs(self.config.heartbeat_interval_secs));

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let event = DaemonEvent::Raw(RawEvent::Heartbeat {
                        timestamp: Utc::now(),
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("TimeWatcher: No subscribers for heartbeat: {}", e);
                    }
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {
                            // Already running, ignore
                        }
                        WatcherControl::Pause => {
                            // Level 0 ignores pause
                            debug!("TimeWatcher: Ignoring pause signal (Level 0)");
                        }
                        WatcherControl::Shutdown => {
                            info!("TimeWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
```

**Step 4: Create watchers module structure**

Create `core/src/daemon/perception/watchers/mod.rs`:

```rust
pub mod time;

#[cfg(test)]
mod tests;

pub use time::TimeWatcher;
```

Create `core/src/daemon/perception/watchers/tests/mod.rs`:

```rust
mod time_tests;
```

**Step 5: Update perception module exports**

Modify `core/src/daemon/perception/mod.rs`:

```rust
pub mod config;
pub mod registry;
pub mod watcher;
pub mod watchers;  // NEW

#[cfg(test)]
mod tests;

pub use config::{
    FSWatcherConfig, PerceptionConfig, ProcessWatcherConfig, SystemWatcherConfig,
    TimeWatcherConfig,
};
pub use registry::WatcherRegistry;
pub use watcher::{Watcher, WatcherControl, WatcherHealth};
pub use watchers::*;  // NEW
```

**Step 6: Run tests to verify they pass**

Run: `cargo test --lib watchers::tests::time_tests`
Expected: PASS (2 tests)

**Step 7: Commit**

```bash
git add core/src/daemon/perception/watchers/
git commit -m "feat(daemon): implement TimeWatcher

- Heartbeat every N seconds (configurable)
- Level 0 watcher (always-on, ignores pause)
- Uses tokio::time::interval for precise timing
- Tests: heartbeat emission and Level 0 behavior"
```

---

## Task 5: ProcessWatcher Implementation

**Files:**
- Create: `core/src/daemon/perception/watchers/process.rs`
- Modify: `core/src/daemon/perception/watchers/mod.rs`
- Test: `core/src/daemon/perception/watchers/tests/process_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/perception/watchers/tests/process_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::perception::{ProcessWatcher, ProcessWatcherConfig, Watcher};

    #[test]
    fn test_process_watcher_creation() {
        let config = ProcessWatcherConfig {
            enabled: true,
            poll_interval_secs: 5,
            watched_apps: vec!["Code".to_string()],
        };

        let watcher = ProcessWatcher::new(config);
        assert_eq!(watcher.id(), "process");
        assert!(!watcher.is_pausable()); // Level 0
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib watchers::tests::process_tests`
Expected: FAIL with "no type named `ProcessWatcher`"

**Step 3: Implement ProcessWatcher**

Create `core/src/daemon/perception/watchers/process.rs`:

```rust
use crate::daemon::{
    DaemonEvent, DaemonEventBus, RawEvent, Result,
    perception::{ProcessWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use sysinfo::{ProcessRefreshKind, RefreshKind, System};
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

pub struct ProcessWatcher {
    config: ProcessWatcherConfig,
}

impl ProcessWatcher {
    pub fn new(config: ProcessWatcherConfig) -> Self {
        Self { config }
    }

    fn check_processes(&self, system: &mut System, bus: &Arc<DaemonEventBus>, tracked: &mut HashMap<u32, String>) {
        // Refresh process list
        system.refresh_processes_specifics(
            ProcessRefreshKind::new()
                .with_cpu()
                .with_memory()
        );

        let mut current_pids = HashMap::new();

        // Check for new processes
        for (pid, process) in system.processes() {
            let name = process.name();

            if self.config.watched_apps.iter().any(|app| name.contains(app)) {
                let pid_value = pid.as_u32();
                current_pids.insert(pid_value, name.to_string());

                // New process detected
                if !tracked.contains_key(&pid_value) {
                    let event = DaemonEvent::Raw(RawEvent::ProcessDetected {
                        name: name.to_string(),
                        pid: pid_value,
                        cpu_usage: process.cpu_usage(),
                        memory: process.memory(),
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("ProcessWatcher: Failed to send event: {}", e);
                    }
                }
            }
        }

        // Check for terminated processes
        for (pid, name) in tracked.iter() {
            if !current_pids.contains_key(pid) {
                let event = DaemonEvent::Raw(RawEvent::ProcessTerminated {
                    name: name.clone(),
                    pid: *pid,
                });

                if let Err(e) = bus.send(event) {
                    debug!("ProcessWatcher: Failed to send event: {}", e);
                }
            }
        }

        *tracked = current_pids;
    }
}

#[async_trait]
impl Watcher for ProcessWatcher {
    fn id(&self) -> &'static str {
        "process"
    }

    fn is_pausable(&self) -> bool {
        false // Level 0: always-on
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!(
            "ProcessWatcher started (polling every {}s, tracking {} apps)",
            self.config.poll_interval_secs,
            self.config.watched_apps.len()
        );

        let mut system = System::new_with_specifics(
            RefreshKind::new().with_processes(ProcessRefreshKind::new())
        );
        let mut tracked_processes: HashMap<u32, String> = HashMap::new();
        let mut ticker = interval(Duration::from_secs(self.config.poll_interval_secs));

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.check_processes(&mut system, &bus, &mut tracked_processes);
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {}
                        WatcherControl::Pause => {
                            debug!("ProcessWatcher: Ignoring pause signal (Level 0)");
                        }
                        WatcherControl::Shutdown => {
                            info!("ProcessWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
```

**Step 4: Update watchers module**

Modify `core/src/daemon/perception/watchers/mod.rs`:

```rust
pub mod process;  // NEW
pub mod time;

#[cfg(test)]
mod tests;

pub use process::ProcessWatcher;  // NEW
pub use time::TimeWatcher;
```

Update `core/src/daemon/perception/watchers/tests/mod.rs`:

```rust
mod process_tests;  // NEW
mod time_tests;
```

**Step 5: Run tests to verify they pass**

Run: `cargo test --lib watchers::tests::process_tests`
Expected: PASS (1 test)

**Step 6: Commit**

```bash
git add core/src/daemon/perception/watchers/process.rs core/src/daemon/perception/watchers/mod.rs core/src/daemon/perception/watchers/tests/
git commit -m "feat(daemon): implement ProcessWatcher

- Monitor configured applications via sysinfo
- Detect process start/termination
- Track CPU usage and memory per process
- Level 0 watcher (always-on)
- Maintains state to detect terminations"
```

---

## Task 6: SystemStateWatcher Implementation

**Files:**
- Create: `core/src/daemon/perception/watchers/system.rs`
- Modify: `core/src/daemon/perception/watchers/mod.rs`
- Test: `core/src/daemon/perception/watchers/tests/system_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/perception/watchers/tests/system_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::perception::{SystemStateWatcher, SystemWatcherConfig, Watcher};

    #[test]
    fn test_system_watcher_creation() {
        let config = SystemWatcherConfig {
            enabled: true,
            poll_interval_secs: 60,
            track_battery: true,
            track_network: true,
            idle_threshold_secs: 300,
        };

        let watcher = SystemStateWatcher::new(config);
        assert_eq!(watcher.id(), "system");
        assert!(!watcher.is_pausable()); // Level 0
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib watchers::tests::system_tests`
Expected: FAIL with "no type named `SystemStateWatcher`"

**Step 3: Implement SystemStateWatcher**

Create `core/src/daemon/perception/watchers/system.rs`:

```rust
use crate::daemon::{
    DaemonEvent, DaemonEventBus, RawEvent, Result,
    perception::{SystemWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use battery::Manager as BatteryManager;
use std::sync::Arc;
use sysinfo::System;
use tokio::sync::watch;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};
use user_idle::UserIdle;

pub struct SystemStateWatcher {
    config: SystemWatcherConfig,
}

impl SystemStateWatcher {
    pub fn new(config: SystemWatcherConfig) -> Self {
        Self { config }
    }

    async fn collect_state(&self) -> (Option<f32>, bool, bool, f32) {
        // Battery status
        let battery_percent = if self.config.track_battery {
            self.get_battery_level().await
        } else {
            None
        };

        // User idle status
        let is_idle = UserIdle::get_time()
            .map(|idle_time| idle_time.as_seconds() > self.config.idle_threshold_secs)
            .unwrap_or(false);

        // Network status
        let network_online = if self.config.track_network {
            self.check_network().await
        } else {
            false
        };

        // CPU usage
        let mut sys = System::new();
        sys.refresh_cpu();
        tokio::time::sleep(Duration::from_millis(200)).await; // Wait for CPU calculation
        sys.refresh_cpu();
        let cpu_usage = sys.global_cpu_info().cpu_usage();

        (battery_percent, is_idle, network_online, cpu_usage)
    }

    async fn get_battery_level(&self) -> Option<f32> {
        match BatteryManager::new() {
            Ok(manager) => {
                if let Ok(mut batteries) = manager.batteries() {
                    if let Some(Ok(battery)) = batteries.next() {
                        return Some(battery.state_of_charge().value * 100.0);
                    }
                }
                None
            }
            Err(e) => {
                debug!("Failed to initialize battery manager: {}", e);
                None
            }
        }
    }

    async fn check_network(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            tokio::process::Command::new("route")
                .arg("-n")
                .arg("get")
                .arg("default")
                .output()
                .await
                .map(|output| output.status.success())
                .unwrap_or(false)
        }

        #[cfg(not(target_os = "macos"))]
        {
            warn!("Network checking not implemented for this platform");
            false
        }
    }
}

#[async_trait]
impl Watcher for SystemStateWatcher {
    fn id(&self) -> &'static str {
        "system"
    }

    fn is_pausable(&self) -> bool {
        false // Level 0: always-on
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!(
            "SystemStateWatcher started (polling every {}s)",
            self.config.poll_interval_secs
        );

        let mut ticker = interval(Duration::from_secs(self.config.poll_interval_secs));

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let (battery_percent, is_idle, network_online, cpu_usage) = self.collect_state().await;

                    let event = DaemonEvent::Raw(RawEvent::SystemState {
                        battery_percent,
                        is_idle,
                        network_online,
                        cpu_usage,
                    });

                    if let Err(e) = bus.send(event) {
                        debug!("SystemStateWatcher: Failed to send event: {}", e);
                    }
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {}
                        WatcherControl::Pause => {
                            debug!("SystemStateWatcher: Ignoring pause signal (Level 0)");
                        }
                        WatcherControl::Shutdown => {
                            info!("SystemStateWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
```

**Step 4: Update watchers module**

Modify `core/src/daemon/perception/watchers/mod.rs`:

```rust
pub mod process;
pub mod system;  // NEW
pub mod time;

#[cfg(test)]
mod tests;

pub use process::ProcessWatcher;
pub use system::SystemStateWatcher;  // NEW
pub use time::TimeWatcher;
```

Update `core/src/daemon/perception/watchers/tests/mod.rs`:

```rust
mod process_tests;
mod system_tests;  // NEW
mod time_tests;
```

**Step 5: Run tests to verify they pass**

Run: `cargo test --lib watchers::tests::system_tests`
Expected: PASS (1 test)

**Step 6: Commit**

```bash
git add core/src/daemon/perception/watchers/system.rs core/src/daemon/perception/watchers/mod.rs core/src/daemon/perception/watchers/tests/
git commit -m "feat(daemon): implement SystemStateWatcher

- Monitor battery level, user idle, network, CPU
- Uses battery, user-idle, sysinfo crates
- Level 0 watcher (always-on)
- Configurable tracking and idle threshold"
```

---

## Task 7: FSEventWatcher Implementation

**Files:**
- Create: `core/src/daemon/perception/watchers/filesystem.rs`
- Modify: `core/src/daemon/perception/watchers/mod.rs`
- Test: `core/src/daemon/perception/watchers/tests/filesystem_tests.rs`

**Step 1: Write the failing test**

Create `core/src/daemon/perception/watchers/tests/filesystem_tests.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::perception::{FSEventWatcher, FSWatcherConfig, Watcher};

    #[test]
    fn test_fs_watcher_creation() {
        let config = FSWatcherConfig {
            enabled: true,
            watched_paths: vec!["/tmp".to_string()],
            ignore_patterns: vec!["**/.git/**".to_string()],
            debounce_ms: 500,
        };

        let watcher = FSEventWatcher::new(config);
        assert_eq!(watcher.id(), "filesystem");
        assert!(watcher.is_pausable()); // Level 1
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib watchers::tests::filesystem_tests`
Expected: FAIL with "no type named `FSEventWatcher`"

**Step 3: Implement FSEventWatcher**

Create `core/src/daemon/perception/watchers/filesystem.rs`:

```rust
use crate::daemon::{
    DaemonEvent, DaemonEventBus, FileChangeKind, RawEvent, Result,
    perception::{FSWatcherConfig, Watcher, WatcherControl},
};
use async_trait::async_trait;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, error, info};

pub struct FSEventWatcher {
    config: FSWatcherConfig,
}

impl FSEventWatcher {
    pub fn new(config: FSWatcherConfig) -> Self {
        Self { config }
    }

    fn should_ignore(&self, path: &PathBuf) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.config.ignore_patterns {
            if glob_match::glob_match(pattern, &path_str) {
                return true;
            }
        }

        false
    }

    fn event_kind_to_change_kind(kind: &EventKind) -> Option<FileChangeKind> {
        match kind {
            EventKind::Create(_) => Some(FileChangeKind::Created),
            EventKind::Modify(_) => Some(FileChangeKind::Modified),
            EventKind::Remove(_) => Some(FileChangeKind::Removed),
            _ => None,
        }
    }
}

#[async_trait]
impl Watcher for FSEventWatcher {
    fn id(&self) -> &'static str {
        "filesystem"
    }

    fn is_pausable(&self) -> bool {
        true // Level 1: pausable
    }

    async fn run(
        &self,
        bus: Arc<DaemonEventBus>,
        mut control: watch::Receiver<WatcherControl>,
    ) -> Result<()> {
        info!(
            "FSEventWatcher started (watching {} paths, debounce {}ms)",
            self.config.watched_paths.len(),
            self.config.debounce_ms
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel::<DebounceEventResult>(100);

        // Create debounced watcher
        let mut debouncer: Debouncer<RecommendedWatcher, FileIdMap> = new_debouncer(
            Duration::from_millis(self.config.debounce_ms),
            None,
            move |result| {
                let _ = tx.blocking_send(result);
            },
        ).map_err(|e| crate::daemon::DaemonError::Config(format!("Failed to create watcher: {}", e)))?;

        // Watch all configured paths
        for path_str in &self.config.watched_paths {
            let expanded = shellexpand::tilde(path_str);
            let path = PathBuf::from(expanded.as_ref());

            if path.exists() {
                debouncer
                    .watcher()
                    .watch(&path, RecursiveMode::Recursive)
                    .map_err(|e| crate::daemon::DaemonError::Config(format!("Failed to watch {}: {}", path.display(), e)))?;
                info!("Watching: {}", path.display());
            } else {
                debug!("Path does not exist, skipping: {}", path.display());
            }
        }

        let mut paused = false;

        loop {
            tokio::select! {
                Some(result) = rx.recv() => {
                    if paused {
                        continue;
                    }

                    match result {
                        Ok(events) => {
                            for event in events {
                                for path in &event.paths {
                                    if self.should_ignore(path) {
                                        continue;
                                    }

                                    if let Some(kind) = Self::event_kind_to_change_kind(&event.kind) {
                                        let daemon_event = DaemonEvent::Raw(RawEvent::FileChanged {
                                            path: path.clone(),
                                            kind,
                                        });

                                        if let Err(e) = bus.send(daemon_event) {
                                            debug!("FSEventWatcher: Failed to send event: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(errors) => {
                            for error in errors {
                                error!("FSEventWatcher error: {:?}", error);
                            }
                        }
                    }
                }

                _ = control.changed() => {
                    let signal = *control.borrow();
                    match signal {
                        WatcherControl::Run => {
                            if paused {
                                info!("FSEventWatcher resuming");
                                paused = false;
                            }
                        }
                        WatcherControl::Pause => {
                            if !paused {
                                info!("FSEventWatcher pausing");
                                paused = true;
                            }
                        }
                        WatcherControl::Shutdown => {
                            info!("FSEventWatcher shutting down");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
```

**Step 4: Add glob_match dependency**

Modify `core/Cargo.toml`:

```toml
# Perception Layer dependencies
glob-match = "0.2"
notify = "6.1"
notify-debouncer-full = "0.3"
shellexpand = "3.1"
user-idle = "0.5"
```

**Step 5: Update watchers module**

Modify `core/src/daemon/perception/watchers/mod.rs`:

```rust
pub mod filesystem;  // NEW
pub mod process;
pub mod system;
pub mod time;

#[cfg(test)]
mod tests;

pub use filesystem::FSEventWatcher;  // NEW
pub use process::ProcessWatcher;
pub use system::SystemStateWatcher;
pub use time::TimeWatcher;
```

Update `core/src/daemon/perception/watchers/tests/mod.rs`:

```rust
mod filesystem_tests;  // NEW
mod process_tests;
mod system_tests;
mod time_tests;
```

**Step 6: Run tests to verify they pass**

Run: `cargo test --lib watchers::tests::filesystem_tests`
Expected: PASS (1 test)

**Step 7: Commit**

```bash
git add core/Cargo.toml core/src/daemon/perception/watchers/filesystem.rs core/src/daemon/perception/watchers/mod.rs core/src/daemon/perception/watchers/tests/
git commit -m "feat(daemon): implement FSEventWatcher

- Monitor file system changes with notify crate
- Debouncing to prevent event floods
- Ignore patterns support (git, node_modules, etc)
- Level 1 watcher (pausable)
- Recursive directory watching"
```

---

## Task 8: Integration with Daemon CLI

**Files:**
- Modify: `core/src/daemon/cli.rs`
- Test: Integration test (manual)

**Step 1: Update daemon run() method**

Modify `core/src/daemon/cli.rs`:

Find the `run()` method in `DaemonCli` implementation and replace it with:

```rust
async fn run(&self) -> Result<()> {
    info!("Starting Aleph daemon with Perception Layer...");

    // 1. Load configurations
    let config = DaemonConfig::default();
    let mut perception_config = PerceptionConfig::load()?;
    perception_config.expand_paths()?;

    // 2. Create EventBus
    let event_bus = Arc::new(DaemonEventBus::new(1000));

    // 3. Create and register Watchers
    let mut registry = WatcherRegistry::new();

    if perception_config.enabled {
        if perception_config.process.enabled {
            registry.register(Box::new(ProcessWatcher::new(
                perception_config.process.clone(),
            )));
        }

        if perception_config.filesystem.enabled {
            registry.register(Box::new(FSEventWatcher::new(
                perception_config.filesystem.clone(),
            )));
        }

        if perception_config.time.enabled {
            registry.register(Box::new(TimeWatcher::new(
                perception_config.time.clone(),
            )));
        }

        if perception_config.system.enabled {
            registry.register(Box::new(SystemStateWatcher::new(
                perception_config.system.clone(),
            )));
        }

        info!("Registered {} watchers", registry.watcher_count());

        // 4. Start all Watchers
        registry.start_all(event_bus.clone()).await?;
        info!("All watchers started");
    } else {
        info!("Perception layer disabled in configuration");
    }

    // 5. Start IPC Server
    let server = IpcServer::new(config.socket_path.clone());
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server.start().await {
            error!("IPC server error: {}", e);
        }
    });

    // 6. Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    // 7. Graceful shutdown
    info!("Shutting down daemon...");
    registry.shutdown_all().await?;
    server_handle.abort();
    info!("Daemon stopped");

    Ok(())
}
```

**Step 2: Add required imports**

At the top of `core/src/daemon/cli.rs`, ensure these imports exist:

```rust
use crate::daemon::{
    create_service_manager, DaemonConfig, DaemonEventBus, PerceptionConfig, ServiceManager,
    WatcherRegistry,
    perception::watchers::{FSEventWatcher, ProcessWatcher, SystemStateWatcher, TimeWatcher},
};
use std::sync::Arc;
use tracing::{error, info};
```

**Step 3: Manual integration test**

Run: `cargo run -p alephcore -- daemon run`

Expected behavior:
- Daemon starts with message "Starting Aleph daemon with Perception Layer..."
- Shows "Registered 4 watchers" (or fewer if some disabled)
- Each watcher logs its startup
- Heartbeat events appear every 30 seconds
- Ctrl+C triggers graceful shutdown

**Step 4: Commit**

```bash
git add core/src/daemon/cli.rs
git commit -m "feat(daemon): integrate perception layer with daemon CLI

- Load PerceptionConfig on daemon start
- Register and start all enabled watchers
- Graceful shutdown of watchers on Ctrl+C
- 1000-event capacity EventBus
- Full lifecycle management"
```

---

## Task 9: Testing Suite

**Files:**
- Create: `core/src/daemon/tests/perception_integration.rs`
- Modify: `core/src/daemon/tests/mod.rs`

**Step 1: Create integration test**

Create `core/src/daemon/tests/perception_integration.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::daemon::{
        DaemonEvent, DaemonEventBus, PerceptionConfig, RawEvent, WatcherRegistry,
        perception::watchers::{ProcessWatcher, TimeWatcher},
    };
    use std::sync::Arc;
    use tokio::time::{timeout, Duration};

    #[tokio::test]
    #[ignore] // Manual test - requires time to collect events
    async fn test_perception_full_lifecycle() {
        // Create minimal config
        let mut config = PerceptionConfig::default();
        config.time.heartbeat_interval_secs = 1; // Fast heartbeat
        config.process.poll_interval_secs = 2;
        config.filesystem.enabled = false; // Disable to avoid noise
        config.system.enabled = false;

        // Create EventBus and registry
        let bus = Arc::new(DaemonEventBus::new(100));
        let mut registry = WatcherRegistry::new();

        // Register watchers
        registry.register(Box::new(TimeWatcher::new(config.time.clone())));
        registry.register(Box::new(ProcessWatcher::new(config.process.clone())));

        // Start watchers
        registry.start_all(bus.clone()).await.unwrap();

        // Subscribe to events
        let mut receiver = bus.subscribe();

        // Collect events for 3 seconds
        let result = timeout(Duration::from_secs(3), async {
            let mut event_count = 0;
            let mut heartbeat_count = 0;

            while event_count < 5 {
                if let Ok(event) = receiver.recv().await {
                    event_count += 1;
                    if matches!(event, DaemonEvent::Raw(RawEvent::Heartbeat { .. })) {
                        heartbeat_count += 1;
                    }
                }
            }

            (event_count, heartbeat_count)
        })
        .await;

        assert!(result.is_ok());
        let (total, heartbeats) = result.unwrap();
        assert!(total >= 5);
        assert!(heartbeats >= 2); // At least 2 heartbeats in 3 seconds

        // Shutdown
        registry.shutdown_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_bus_capacity_limit() {
        let bus = DaemonEventBus::new(10);
        let mut receiver = bus.subscribe();

        // Send more events than capacity
        for i in 0..15 {
            let event = DaemonEvent::Raw(RawEvent::Heartbeat {
                timestamp: chrono::Utc::now(),
            });
            let _ = bus.send(event);
        }

        // Receiver should have lagged and missed some events
        let mut received = 0;
        while let Ok(_) = receiver.try_recv() {
            received += 1;
        }

        // Should receive at most 10 (capacity)
        assert!(received <= 10);
    }
}
```

**Step 2: Update test module**

Modify `core/src/daemon/tests/mod.rs`:

```rust
mod cli_tests;
mod integration_tests;
mod ipc_tests;
mod launchd_tests;
mod perception_integration;  // NEW
mod resource_governor_tests;
mod service_manager_tests;
```

**Step 3: Run all perception tests**

Run: `cargo test --lib daemon::perception`
Expected: All unit tests PASS

Run: `cargo test --lib daemon::tests::perception_integration`
Expected: Non-ignored tests PASS

**Step 4: Commit**

```bash
git add core/src/daemon/tests/perception_integration.rs core/src/daemon/tests/mod.rs
git commit -m "test(daemon): add perception layer integration tests

- Full lifecycle test with multiple watchers
- EventBus capacity limit test
- Validates event emission and shutdown
- Ignored tests for manual verification"
```

---

## Task 10: Documentation

**Files:**
- Create: `core/src/daemon/perception/README.md`
- Modify: `core/src/daemon/README.md`

**Step 1: Create perception documentation**

Create `core/src/daemon/perception/README.md`:

```markdown
# Perception Layer

The Perception Layer is Aleph's "sensory system" - a collection of Watchers that monitor OS-level events and convert them into structured events for higher-level reasoning.

## Architecture

```
┌─────────────────────────────────────────────┐
│  Watchers (Level 0 and Level 1)            │
└──────────┬──────────────────────────────────┘
           │ DaemonEvent
┌──────────┴──────────────────────────────────┐
│  DaemonEventBus (tokio::broadcast)          │
└──────────┬──────────────────────────────────┘
           │ Subscribe
┌──────────┴──────────────────────────────────┐
│  WorldModel (Phase 3)                       │
│  Dispatcher (Phase 4)                       │
└─────────────────────────────────────────────┘
```

## Watchers

### Level 0 (Always-On)

**ProcessWatcher** - Monitor application launches and terminations
- Tracks configured applications (Code, Chrome, Slack, etc.)
- Reports CPU usage and memory
- Poll interval: 5 seconds (configurable)

**TimeWatcher** - Provide heartbeat signal
- Simple periodic tick for time-based triggers
- Heartbeat interval: 30 seconds (configurable)
- Most lightweight watcher

**SystemStateWatcher** - Monitor system resources
- Battery level tracking
- User idle detection (300s threshold)
- Network connectivity check
- Global CPU usage
- Poll interval: 60 seconds (configurable)

### Level 1 (Pausable)

**FSEventWatcher** - Monitor file system changes
- Watches configured directories (~/Downloads, ~/Desktop)
- Ignore patterns for .git, node_modules, etc.
- 500ms debouncing to prevent floods
- Can be paused when battery < 20%

## Configuration

**File:** `~/.aleph/perception.toml`

```toml
enabled = true

[process]
enabled = true
poll_interval_secs = 5
watched_apps = ["Code", "Google Chrome", "Zoom", "Slack", "Terminal"]

[filesystem]
enabled = true
watched_paths = ["~/Downloads", "~/Desktop"]
ignore_patterns = ["**/.git/**", "**/node_modules/**"]
debounce_ms = 500

[time]
enabled = true
heartbeat_interval_secs = 30

[system]
enabled = true
poll_interval_secs = 60
track_battery = true
track_network = true
idle_threshold_secs = 300
```

## Event Types

### RawEvent

Direct OS observations:
- `ProcessDetected` / `ProcessTerminated`
- `FileChanged`
- `Heartbeat`
- `SystemState`

### DerivedEvent (Phase 3)

High-level inferred context:
- `UserActivityChanged`
- `ResourceConstraintDetected`

### SystemEvent

Control events:
- `Shutdown`
- `ConfigReloaded`
- `WatcherPaused` / `WatcherResumed`

## Usage

```rust
use alephcore::daemon::{
    DaemonEventBus, PerceptionConfig, WatcherRegistry,
    perception::watchers::*,
};

// Load config
let config = PerceptionConfig::load()?;

// Create EventBus
let bus = Arc::new(DaemonEventBus::new(1000));

// Register watchers
let mut registry = WatcherRegistry::new();
registry.register(Box::new(TimeWatcher::new(config.time)));
registry.register(Box::new(ProcessWatcher::new(config.process)));

// Start all
registry.start_all(bus.clone()).await?;

// Subscribe to events
let mut receiver = bus.subscribe();
while let Ok(event) = receiver.recv().await {
    println!("Event: {:?}", event);
}

// Graceful shutdown
registry.shutdown_all().await?;
```

## Testing

Run all perception tests:
```bash
cargo test --lib daemon::perception
cargo test --lib daemon::tests::perception_integration
```

Manual daemon test:
```bash
cargo run -p alephcore -- daemon run
```

## Performance

| Component | CPU (idle) | Memory |
|-----------|-----------|--------|
| ProcessWatcher | 1-2% | 5MB |
| FSEventWatcher | 2% | 10MB |
| TimeWatcher | <0.1% | 1MB |
| SystemStateWatcher | 1-2% | 5MB |
| **Total** | <5% | <50MB |

## Dependencies

- `notify` - File system monitoring
- `notify-debouncer-full` - Event debouncing
- `sysinfo` - Process and CPU monitoring
- `battery` - Battery status
- `user-idle` - User idle detection
- `shellexpand` - Path expansion
- `tokio::broadcast` - Event bus
- `tokio::sync::watch` - Control signals
```

**Step 2: Update main daemon README**

Modify `core/src/daemon/README.md`, add section after "Architecture":

```markdown
## Perception Layer (Phase 2)

The Perception Layer continuously monitors OS-level events and publishes them to the `DaemonEventBus`. See [`perception/README.md`](perception/README.md) for details.

**Key Features:**
- 4 configurable Watchers (Process, File, Time, System)
- Level 0/1 tiering for adaptive resource management
- <5% CPU, <50MB RAM resource budget
- Independent event system (DaemonEvent vs Agent events)

**Quick Start:**
```bash
# Run daemon with perception layer
cargo run -p alephcore -- daemon run

# Check configuration
cat ~/.aleph/perception.toml
```
```

**Step 3: Commit**

```bash
git add core/src/daemon/perception/README.md core/src/daemon/README.md
git commit -m "docs(daemon): add perception layer documentation

- Complete README with architecture diagram
- Configuration examples and usage guide
- Performance characteristics
- Testing instructions"
```

---

## Execution Checklist

After implementing all tasks:

- [ ] All unit tests pass (`cargo test --lib daemon`)
- [ ] Integration tests pass (non-ignored)
- [ ] Manual daemon test successful (`cargo run -p alephcore -- daemon run`)
- [ ] All 4 watchers emit events correctly
- [ ] Resource usage within budget (<5% CPU, <50MB RAM)
- [ ] Graceful shutdown works (Ctrl+C)
- [ ] Configuration loading from `~/.aleph/perception.toml` works
- [ ] Level 1 watcher (FSEventWatcher) can be paused/resumed
- [ ] Documentation complete and accurate

---

## Success Metrics

**Technical:**
- Event latency: Detection to EventBus < 100ms
- Watcher uptime: > 99.9%
- CPU usage: < 5%
- Memory usage: < 50MB

**Functional:**
- TimeWatcher: Heartbeat every 30s
- ProcessWatcher: Detects app launch/termination within 5s
- FSEventWatcher: File changes detected and debounced
- SystemStateWatcher: Battery, idle, network, CPU reported every 60s

---

## Next Steps (Phase 3)

After Phase 2 completion:
1. Implement WorldModel to subscribe to DaemonEventBus
2. Add context inference (Raw → Derived events)
3. Build state persistence for inferred context
4. Design and implement Dispatcher integration
