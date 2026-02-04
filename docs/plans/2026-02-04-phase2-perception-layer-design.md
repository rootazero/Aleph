# Phase 2: Perception Layer Design

**Date:** 2026-02-04
**Status:** Approved
**Dependencies:** Phase 1 (Daemon Manager)
**Related:** [Proactive AI Architecture](./2026-02-04-proactive-ai-architecture-design.md)

## Executive Summary

This document presents the detailed design for Phase 2 of Aether's proactive AI system: the Perception Layer. This layer acts as the system's "sensory system," continuously monitoring environment changes and converting them into standardized events for the WorldModel (Phase 3).

**Core Philosophy:** Build an adaptive, configurable sensing system that operates invisibly while providing rich environmental context.

---

## 1. System Overview

### 1.1 Objective

Act as Aether's "sensory system" - continuously monitoring OS-level events (processes, files, time, system state) and transforming them into structured events for higher-level reasoning.

### 1.2 Design Principles

1. **Invisible First** - Low resource footprint (< 5% CPU, < 50MB RAM)
2. **Configurable** - User-customizable monitoring scope via `perception.toml`
3. **Adaptive** - Dynamic adjustment based on system resources (Level 0/1 tiering)
4. **Isolated** - Independent event system (DaemonEvent) separate from Agent domain events

### 1.3 Architecture Layers

```
┌─────────────────────────────────────────────┐
│  Watchers (感知器)                          │
│  ├─ Level 0: TimeWatcher                    │
│  │           SystemStateWatcher             │
│  │           ProcessWatcher                 │
│  └─ Level 1: FSEventWatcher (pausable)      │
└──────────┬──────────────────────────────────┘
           │ DaemonEvent (Raw/Derived)
┌──────────┴──────────────────────────────────┐
│  DaemonEventBus                             │
│  (tokio::broadcast channel)                 │
└──────────┬──────────────────────────────────┘
           │ Subscribe
┌──────────┴──────────────────────────────────┐
│  WorldModel (Phase 3)                       │
│  Dispatcher (Phase 4)                       │
└─────────────────────────────────────────────┘
```

### 1.4 Core Architectural Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| **EventBus Strategy** | Independent DaemonEventBus | Avoid polluting Agent domain events with OS-level noise |
| **Monitoring Granularity** | Configurable + Default Coarse | Balance flexibility with resource frugality |
| **Lifecycle Strategy** | Hybrid Tiering (Level 0 always-on, Level 1 pausable) | Maintain "sensing core" while enabling adaptive throttling |
| **Event Layering** | Raw → Derived separation | Clean boundary between OS events and inferred context |

---

## 2. Event System Design

### 2.1 Module Structure

```
core/src/daemon/
├── events.rs           # DaemonEvent enum definitions
├── event_bus.rs        # EventBus implementation
└── perception/         # Perception subsystem
    ├── mod.rs
    ├── config.rs       # PerceptionConfig
    ├── watcher.rs      # Watcher trait
    ├── registry.rs     # WatcherRegistry
    └── watchers/       # Concrete implementations
        ├── process.rs
        ├── filesystem.rs
        ├── time.rs
        └── system.rs
```

### 2.2 Event Types

```rust
// core/src/daemon/events.rs

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

/// System control events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    Shutdown,
    ConfigReloaded,
    WatcherPaused(String),
    WatcherResumed(String),
}
```

### 2.3 EventBus Implementation

```rust
// core/src/daemon/event_bus.rs
use tokio::sync::broadcast;

pub struct DaemonEventBus {
    sender: broadcast::Sender<DaemonEvent>,
    capacity: usize,
}

impl DaemonEventBus {
    /// Create new EventBus with specified capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender, capacity }
    }

    /// Send event to all subscribers
    pub fn send(&self, event: DaemonEvent) -> Result<()> {
        self.sender.send(event)
            .map(|_| ())
            .map_err(|_| DaemonError::EventBus("No subscribers".into()))
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.sender.subscribe()
    }
}
```

**Design Notes:**
- Uses `tokio::sync::broadcast` for multi-subscriber support
- Default capacity: 1000 events (prevents memory leaks under high load)
- Lagged receivers are automatically dropped if they can't keep up

---

## 3. Configuration System

### 3.1 Configuration Structure

```rust
// core/src/daemon/perception/config.rs

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
    pub watched_paths: Vec<PathBuf>,
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
```

### 3.2 Default Configuration

**File:** `~/.aether/perception.toml`

```toml
enabled = true

[process]
enabled = true
poll_interval_secs = 5
watched_apps = ["Code", "Google Chrome", "Zoom", "Slack", "Terminal"]

[filesystem]
enabled = true
watched_paths = ["~/Downloads", "~/Desktop"]
ignore_patterns = ["**/.git/**", "**/node_modules/**", "**/target/**", "**/.DS_Store"]
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

### 3.3 Configuration Loading

```rust
impl PerceptionConfig {
    pub fn load() -> Result<Self> {
        let path = dirs::home_dir()
            .ok_or_else(|| DaemonError::Config("HOME not found".into()))?
            .join(".aether/perception.toml");

        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            toml::from_str(&content)
                .map_err(|e| DaemonError::Config(format!("Invalid TOML: {}", e)))
        } else {
            Ok(Self::default())
        }
    }
}
```

**Design Notes:**
- Fine-grained control: Each Watcher has independent enable/disable
- Performance tuning: Polling intervals and debounce times are configurable
- Safe defaults: Conservative monitoring scope for "out-of-box" experience
- Path expansion: Uses `shellexpand` for `~` support

---

## 4. Watcher Trait and Lifecycle

### 4.1 Watcher Trait

```rust
// core/src/daemon/perception/watcher.rs
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
    fn is_pausable(&self) -> bool { true }

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

### 4.2 WatcherRegistry

```rust
// core/src/daemon/perception/registry.rs

pub struct WatcherRegistry {
    watchers: HashMap<String, Box<dyn Watcher>>,
    handles: HashMap<String, JoinHandle<()>>,
    control_senders: HashMap<String, watch::Sender<WatcherControl>>,
}

impl WatcherRegistry {
    pub fn new() -> Self { /* ... */ }

    /// Register a Watcher
    pub fn register(&mut self, watcher: Box<dyn Watcher>) { /* ... */ }

    /// Start all registered Watchers
    pub async fn start_all(&mut self, bus: Arc<DaemonEventBus>) -> Result<()> { /* ... */ }

    /// Pause a Watcher (Level 1 only)
    pub async fn pause_watcher(&self, id: &str) -> Result<()> { /* ... */ }

    /// Resume a paused Watcher
    pub async fn resume_watcher(&self, id: &str) -> Result<()> { /* ... */ }

    /// Shutdown all Watchers gracefully
    pub async fn shutdown_all(&mut self) -> Result<()> { /* ... */ }
}
```

**Design Notes:**
- Control signals use `tokio::sync::watch` for broadcast to all Watchers
- Each Watcher runs in independent `tokio::spawn` task (error isolation)
- Level 0 Watchers ignore Pause signals but respect Shutdown
- Registry provides centralized lifecycle management

---

## 5. Watcher Implementations

### 5.1 ProcessWatcher (Level 0)

**Purpose:** Detect key application launches/terminations

**Polling Strategy:** Check every 5 seconds using `sysinfo` crate

**Key Features:**
- Tracks only pre-configured applications (default: IDEs, browsers, communication tools)
- Maintains state of tracked PIDs to detect terminations
- Reports CPU usage and memory for each detected process

**Resource Cost:** ~1-2% CPU, ~5MB RAM

---

### 5.2 FSEventWatcher (Level 1 - Pausable)

**Purpose:** Monitor file changes in critical directories

**Implementation:** Uses `notify` crate with `notify-debouncer-full`

**Key Features:**
- Recursive monitoring of configured paths (default: ~/Downloads, ~/Desktop)
- Ignore patterns for noise reduction (`.git`, `node_modules`, `target`)
- 500ms debounce to prevent event floods (e.g., during npm install)
- **Pausable** when battery < 20% or high CPU load

**Resource Cost:**
- Light load: ~2% CPU, ~10MB RAM
- Heavy load (e.g., Rust compile): Can spike to 20% CPU without debouncing

---

### 5.3 TimeWatcher (Level 0)

**Purpose:** Provide heartbeat signal for time-based triggers

**Implementation:** Simple `tokio::time::interval` loop

**Key Features:**
- Sends heartbeat every 30 seconds (configurable)
- Most lightweight Watcher
- Always-on (Level 0)

**Resource Cost:** < 0.1% CPU, < 1MB RAM

---

### 5.4 SystemStateWatcher (Level 0)

**Purpose:** Monitor system resources and user activity

**Implementation:** Integrates multiple system APIs

**Key Features:**
- Battery level tracking (`battery` crate)
- User idle detection (`user-idle` crate)
- Network connectivity check (route command on macOS)
- Global CPU usage (`sysinfo` crate)
- Always-on (Level 0) - provides data for ResourceGovernor decisions

**Resource Cost:** ~1-2% CPU, ~5MB RAM

---

## 6. Integration with Daemon

### 6.1 Module Export

```rust
// core/src/daemon/mod.rs
pub mod cli;
pub mod error;
pub mod events;        // NEW
pub mod event_bus;     // NEW
pub mod ipc;
pub mod perception;    // NEW
pub mod resource_governor;
pub mod service_manager;
pub mod types;

pub use events::{DaemonEvent, RawEvent, DerivedEvent};
pub use event_bus::DaemonEventBus;
pub use perception::{
    PerceptionConfig,
    WatcherRegistry,
    watchers::*,
};
```

### 6.2 Daemon Startup Integration

```rust
// core/src/daemon/cli.rs - run() method extension
async fn run(&self) -> Result<()> {
    info!("Starting Aether daemon with Perception Layer...");

    // 1. Load configuration
    let config = DaemonConfig::default();
    let perception_config = PerceptionConfig::load()?;

    // 2. Create EventBus
    let event_bus = Arc::new(DaemonEventBus::new(1000));

    // 3. Create and register Watchers
    let mut registry = WatcherRegistry::new();

    if perception_config.process.enabled {
        registry.register(Box::new(
            ProcessWatcher::new(perception_config.process.clone())
        ));
    }

    if perception_config.filesystem.enabled {
        registry.register(Box::new(
            FSEventWatcher::new(perception_config.filesystem.clone())
        ));
    }

    if perception_config.time.enabled {
        registry.register(Box::new(
            TimeWatcher::new(perception_config.time.clone())
        ));
    }

    if perception_config.system.enabled {
        registry.register(Box::new(
            SystemStateWatcher::new(perception_config.system.clone())
        ));
    }

    // 4. Start all Watchers
    registry.start_all(event_bus.clone()).await?;
    info!("All Watchers started");

    // 5. Start IPC Server
    let ipc_server = IpcServer::new(config.socket_path);
    tokio::spawn(async move {
        if let Err(e) = ipc_server.start().await {
            error!("IPC server error: {}", e);
        }
    });

    // 6. Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    // 7. Graceful shutdown
    info!("Shutting down daemon...");
    registry.shutdown_all().await?;
    info!("All Watchers stopped");

    Ok(())
}
```

---

## 7. Testing Strategy

### 7.1 Unit Tests

```rust
// core/src/daemon/perception/watchers/tests.rs
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_process_watcher_lifecycle() {
        // Test: Watcher starts, runs, and shuts down cleanly
    }

    #[tokio::test]
    async fn test_time_watcher_heartbeat() {
        // Test: Heartbeat events are sent at correct intervals
    }

    #[tokio::test]
    async fn test_fs_watcher_ignores_patterns() {
        // Test: Ignore patterns correctly filter events
    }

    #[tokio::test]
    async fn test_level1_watcher_pause_resume() {
        // Test: Level 1 Watchers respect Pause/Resume signals
    }
}
```

### 7.2 Integration Tests

```rust
// core/src/daemon/tests/perception_integration.rs
#[tokio::test]
#[ignore]
async fn test_full_perception_lifecycle() {
    // Test: All Watchers start, send events, and shut down
}

#[tokio::test]
#[ignore]
async fn test_event_bus_multi_subscriber() {
    // Test: Multiple subscribers receive events correctly
}
```

### 7.3 Acceptance Criteria

- ✅ All 4 Watchers start and shutdown cleanly
- ✅ Level 1 Watcher (FSEventWatcher) can be paused and resumed
- ✅ Events successfully sent to EventBus and received by subscribers
- ✅ Configuration file correctly loaded and applied
- ✅ Resource usage: CPU < 5%, Memory < 50MB under normal load
- ✅ No memory leaks during 24-hour stress test

---

## 8. Dependencies

### 8.1 New Crate Dependencies

Add to `core/Cargo.toml`:

```toml
# Perception Layer dependencies
notify = "6.1"                        # File system monitoring
notify-debouncer-full = "0.3"        # Event debouncing
user-idle = "0.5"                     # User idle detection
shellexpand = "3.1"                   # Path expansion (~)
chrono = { version = "0.4", features = ["serde"] } # Already exists
```

### 8.2 Existing Dependencies (Already in Cargo.toml)

- `sysinfo = "0.32"` - Process and system monitoring
- `battery = "0.7"` - Battery status
- `tokio` - Async runtime
- `serde`, `serde_json`, `toml` - Serialization

---

## 9. Performance Characteristics

### 9.1 Resource Budget

| Component | CPU (idle) | CPU (active) | Memory |
|-----------|-----------|--------------|--------|
| ProcessWatcher | 1-2% | 2-3% | 5MB |
| FSEventWatcher | 2% | 5-20%* | 10MB |
| TimeWatcher | < 0.1% | < 0.1% | 1MB |
| SystemStateWatcher | 1-2% | 2-3% | 5MB |
| EventBus | < 0.5% | 1% | 10MB |
| **Total** | **< 5%** | **10-30%** | **< 50MB** |

*FSEventWatcher can spike during heavy file operations (npm install, Rust compile)

### 9.2 Event Throughput

- **Normal load:** 1-10 events/second
- **Heavy load:** 100-1000 events/second (with debouncing)
- **EventBus capacity:** 1000 events (prevents memory exhaustion)

---

## 10. Future Extensions

### 10.1 Phase 3 Integration Points

- **WorldModel subscription** to DaemonEventBus
- **Context inference** from Raw events → Derived events
- **State persistence** of inferred context

### 10.2 Phase 4 Integration Points

- **Dispatcher subscription** to Derived events
- **PolicyEngine** uses Derived events for action decisions
- **Agent triggering** based on context changes

### 10.3 Potential New Watchers

- **ScreenWatcher** - OCR/screenshot analysis (privacy-sensitive)
- **NetworkWatcher** - Active connections monitoring
- **CalendarWatcher** - Integration with macOS Calendar
- **EmailWatcher** - Inbox monitoring (requires explicit permission)

---

## 11. Security and Privacy

### 11.1 Privacy Principles

1. **Minimal by Default** - Only monitor essential applications and directories
2. **User Control** - Full configuration control via `perception.toml`
3. **Transparent** - Clear logging of what is being monitored
4. **Local-Only** - All events stay on device, never sent to external servers

### 11.2 Sensitive Data Handling

- File paths are logged but file contents are **never** read
- Process names are tracked but command-line arguments are **excluded**
- Network check is binary (online/offline), no packet inspection

---

## 12. Success Metrics

### 12.1 Technical Metrics

- **Latency:** Event detection to EventBus < 100ms
- **Reliability:** Watcher uptime > 99.9% (with auto-recovery)
- **Efficiency:** CPU < 5%, RAM < 50MB

### 12.2 User Experience Metrics

- **Invisibility:** User perceives zero performance impact
- **Accuracy:** Event detection accuracy > 95%
- **Configurability:** Advanced users can customize without code changes

---

## 13. Implementation Roadmap

### Task Breakdown

**Task 1:** Event System Foundation (events.rs, event_bus.rs)
**Task 2:** Configuration System (config.rs, perception.toml)
**Task 3:** Watcher Trait and Registry (watcher.rs, registry.rs)
**Task 4:** ProcessWatcher Implementation
**Task 5:** TimeWatcher Implementation
**Task 6:** SystemStateWatcher Implementation
**Task 7:** FSEventWatcher Implementation
**Task 8:** Integration with Daemon CLI
**Task 9:** Testing Suite
**Task 10:** Documentation

**Estimated Total:** 10 tasks, ~15-20 hours of implementation

---

## Conclusion

Phase 2 (Perception Layer) transforms Aether from a static daemon into an **adaptive sensing system**. By implementing a configurable, tiered Watcher architecture with an isolated event system, we create the foundation for context-aware proactive AI while maintaining the "Invisible First" principle.

The design's key innovation is the **Level 0/1 tiering strategy**, which ensures Aether always maintains a "sensing core" (Level 0) while intelligently throttling resource-intensive operations (Level 1) based on system constraints. This "organic" behavior mimics a living system that adapts its awareness to available energy.

**Design Status:** ✅ Approved for Implementation
**Next Step:** Create detailed implementation plan using `superpowers:writing-plans`
