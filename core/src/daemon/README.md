# Daemon Module

The Daemon module provides system service management for Aether, enabling it to run persistently in the background.

## Architecture

```
┌─────────────────────────────────────────────┐
│           Daemon Module                      │
├─────────────────────────────────────────────┤
│                                             │
│  ┌──────────────┐  ┌──────────────────┐   │
│  │ ServiceManager│  │ ResourceGovernor │   │
│  │  (launchd)   │  │  (CPU/Mem/Bat)   │   │
│  └──────────────┘  └──────────────────┘   │
│                                             │
│  ┌──────────────────────────────────────┐  │
│  │        IPC Server                     │  │
│  │  (Unix Socket + JSON-RPC 2.0)        │  │
│  └──────────────────────────────────────┘  │
│                                             │
└─────────────────────────────────────────────┘
```

## Components

### ServiceManager

Cross-platform trait for system service management:

- **LaunchdService** (macOS): Manages launchd plist and service lifecycle
- **SystemdService** (Linux): TODO
- **WindowsService** (Windows): TODO

### ResourceGovernor

Monitors system resources and throttles operations:

- CPU usage monitoring
- Memory usage tracking
- Battery level detection
- Automatic throttling under high load

### IPC Server

JSON-RPC 2.0 server over Unix Domain Socket:

- Socket path: `~/.aether/daemon.sock`
- Methods:
  - `daemon.status` - Get daemon status
  - `daemon.ping` - Health check
  - `daemon.shutdown` - Graceful shutdown

## Usage

### CLI Commands

```bash
# Install daemon as system service
aether daemon install

# Start daemon
aether daemon start

# Check status
aether daemon status

# Stop daemon
aether daemon stop

# Uninstall daemon
aether daemon uninstall

# Run in foreground (development)
aether daemon run
```

### Programmatic Usage

```rust
use aethecore::daemon::*;

// Create service manager
let service = create_service_manager()?;

// Install and start
let config = DaemonConfig::default();
service.install(&config).await?;
service.start().await?;

// Resource governor
let governor = ResourceGovernor::new(ResourceLimits::default());
if governor.is_safe_to_run().await {
    // Proceed with proactive tasks
}
```

## Configuration

Default configuration:

```rust
DaemonConfig {
    socket_path: "~/.aether/daemon.sock",
    binary_path: "~/.aether/bin/aether-daemon",
    log_dir: "~/.aether/logs",
    nice_value: 10,
    soft_mem_limit: 512 * 1024 * 1024,  // 512MB
    hard_mem_limit: 1024 * 1024 * 1024, // 1GB
}
```

## Testing

```bash
# Unit tests
cargo test --lib daemon::

# Integration test (requires admin privileges)
cargo test --lib test_daemon_full_lifecycle -- --ignored --nocapture
```

## Platform Support

- ✅ macOS (launchd)
- ⏳ Linux (systemd) - Planned
- ⏳ Windows (Service) - Planned
