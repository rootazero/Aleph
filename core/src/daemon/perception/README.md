# Perception Layer

The Perception Layer is Aether's "sensory system" - a collection of Watchers that monitor OS-level events and convert them into structured events for higher-level reasoning.

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
use aethecore::daemon::{
    DaemonEventBus, PerceptionConfig, WatcherRegistry,
    perception::watchers::*,
};

// Load config
let config = PerceptionConfig::load()?;

// Create EventBus
let bus = Arc::new(DaemonEventBus::new(1000));

// Register watchers
let mut registry = WatcherRegistry::new();
registry.register(Arc::new(TimeWatcher::new(config.time)));
registry.register(Arc::new(ProcessWatcher::new(config.process)));

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
cargo run -p aethecore -- daemon run
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
