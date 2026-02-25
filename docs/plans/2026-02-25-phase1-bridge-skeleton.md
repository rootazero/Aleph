# Phase 1: Bridge Skeleton — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Connect the existing Tauri bridge and Core BridgeSupervisor so that `aleph-server` automatically spawns and manages `aleph-bridge` as a subprocess, with capability negotiation and WebView control.

**Architecture:** The Tauri bridge already has a UDS server, screen capture, tray, and shortcuts. The Core already has `BridgeSupervisor`, `UnixSocketTransport`, and `Transport` trait. Phase 1 wires them together: server spawns bridge → bridge creates UDS socket → server connects via `BridgeSupervisor` → handshake with capability registration → server controls bridge's WebView.

**Tech Stack:** Rust (tokio, serde_json), Tauri v2, `aleph-protocol` shared types, existing `BridgeSupervisor` + `UnixSocketTransport`

**Key Insight:** ~70% of infrastructure already exists. The existing bridge listens on UDS and server connects to it — this matches `BridgeSupervisor`'s model (spawn process → wait for socket → connect). We keep this direction.

---

### Task 1: Unify Socket Path

**Files:**
- Modify: `shared/protocol/src/desktop_bridge.rs:101-105`
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs:21`

**Step 1: Update socket path in shared protocol**

In `shared/protocol/src/desktop_bridge.rs`, change the socket path from `desktop.sock` to `bridge.sock`:

```rust
/// Get the default Desktop Bridge socket path (~/.aleph/bridge.sock)
pub fn default_socket_path() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".aleph").join("bridge.sock")
}
```

**Step 2: Verify bridge uses shared path**

Confirm `apps/desktop/src-tauri/src/bridge/mod.rs` line 21 already uses `desktop_bridge::default_socket_path()`. No change needed if so.

**Step 3: Commit**

```bash
git add shared/protocol/src/desktop_bridge.rs
git commit -m "protocol: rename desktop bridge socket to bridge.sock"
```

---

### Task 2: Add WebView Control Methods to Protocol

**Files:**
- Modify: `shared/protocol/src/desktop_bridge.rs`

**Step 1: Add method constants and capability types**

Append to `shared/protocol/src/desktop_bridge.rs` after the existing method constants:

```rust
// WebView control methods (Server → Bridge)
pub const METHOD_WEBVIEW_SHOW: &str = "webview.show";
pub const METHOD_WEBVIEW_HIDE: &str = "webview.hide";
pub const METHOD_WEBVIEW_NAVIGATE: &str = "webview.navigate";

// Tray control methods (Server → Bridge)
pub const METHOD_TRAY_UPDATE_STATUS: &str = "tray.update_status";

// Bridge lifecycle methods
pub const METHOD_BRIDGE_SHUTDOWN: &str = "bridge.shutdown";

// Capability types for desktop bridge
pub const METHOD_CAPABILITY_REGISTER: &str = "capability.register";

/// A single capability declared by the bridge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeCapabilityInfo {
    pub name: String,
    pub version: String,
}

/// Capability registration payload (bridge → server during handshake)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRegistration {
    pub platform: String,
    pub arch: String,
    pub capabilities: Vec<BridgeCapabilityInfo>,
}
```

**Step 2: Commit**

```bash
git add shared/protocol/src/desktop_bridge.rs
git commit -m "protocol: add webview, tray, and capability registration types"
```

---

### Task 3: Add WebView Control to Tauri Bridge Dispatch

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

**Step 1: Make Tauri AppHandle available to bridge**

In `apps/desktop/src-tauri/src/lib.rs`, store the `AppHandle` in a global so the bridge module can access it:

```rust
use std::sync::OnceLock;
use tauri::AppHandle;

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn get_app_handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}
```

In the Tauri `setup` closure (already in `lib.rs`), store the handle:

```rust
.setup(|app| {
    let _ = APP_HANDLE.set(app.handle().clone());
    // ... existing setup code ...
})
```

**Step 2: Add WebView handlers to bridge dispatch**

In `apps/desktop/src-tauri/src/bridge/mod.rs`, add imports and new dispatch arms:

```rust
use aleph_protocol::desktop_bridge::{
    self, ERR_METHOD_NOT_FOUND, ERR_NOT_IMPLEMENTED, ERR_INTERNAL,
    METHOD_WEBVIEW_SHOW, METHOD_WEBVIEW_HIDE, METHOD_WEBVIEW_NAVIGATE,
    METHOD_TRAY_UPDATE_STATUS, METHOD_BRIDGE_SHUTDOWN,
};
use tauri::Manager;
```

Add to `dispatch()` function:

```rust
METHOD_WEBVIEW_SHOW => handle_webview_show(params),
METHOD_WEBVIEW_HIDE => handle_webview_hide(params),
METHOD_WEBVIEW_NAVIGATE => handle_webview_navigate(params),
METHOD_BRIDGE_SHUTDOWN => {
    // Graceful shutdown
    std::process::exit(0);
}
```

Add handler functions:

```rust
fn handle_webview_show(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let label = params.get("label").and_then(|v| v.as_str()).unwrap_or("halo");
    let url = params.get("url").and_then(|v| v.as_str());

    let Some(app) = crate::get_app_handle() else {
        return Err((ERR_INTERNAL, "App handle not available".into()));
    };

    if let Some(window) = app.get_webview_window(label) {
        if let Some(url_str) = url {
            let _ = window.navigate(url_str.parse().map_err(|e| (ERR_INTERNAL, format!("{e}")))?);
        }
        let _ = window.show();
        let _ = window.set_focus();
        Ok(json!({"shown": true, "label": label}))
    } else {
        Err((ERR_INTERNAL, format!("Window '{}' not found", label)))
    }
}

fn handle_webview_hide(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let label = params.get("label").and_then(|v| v.as_str()).unwrap_or("halo");

    let Some(app) = crate::get_app_handle() else {
        return Err((ERR_INTERNAL, "App handle not available".into()));
    };

    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
        Ok(json!({"hidden": true, "label": label}))
    } else {
        Err((ERR_INTERNAL, format!("Window '{}' not found", label)))
    }
}

fn handle_webview_navigate(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let label = params.get("label").and_then(|v| v.as_str()).unwrap_or("halo");
    let url = params.get("url").and_then(|v| v.as_str())
        .ok_or((ERR_INTERNAL, "Missing 'url' parameter".into()))?;

    let Some(app) = crate::get_app_handle() else {
        return Err((ERR_INTERNAL, "App handle not available".into()));
    };

    if let Some(window) = app.get_webview_window(label) {
        window.navigate(url.parse().map_err(|e| (ERR_INTERNAL, format!("{e}")))?);
        Ok(json!({"navigated": true, "label": label, "url": url}))
    } else {
        Err((ERR_INTERNAL, format!("Window '{}' not found", label)))
    }
}
```

**Step 3: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/mod.rs apps/desktop/src-tauri/src/lib.rs
git commit -m "bridge: add webview.show/hide/navigate command handlers"
```

---

### Task 4: Add Handshake with Capability Registration

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge/mod.rs`

**Step 1: Add `aleph.handshake` handler to bridge dispatch**

The `BridgeSupervisor` sends `aleph.handshake` after connecting. Add it to `dispatch()`:

```rust
"aleph.handshake" => handle_handshake(params),
"system.ping" => Ok(json!({"pong": true})),
```

Add handler:

```rust
fn handle_handshake(params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    let protocol_version = params.get("protocol_version")
        .and_then(|v| v.as_str())
        .unwrap_or("1.0");

    tracing::info!(
        protocol_version,
        "Handshake received from server"
    );

    // Return capability registration
    Ok(json!({
        "protocol_version": protocol_version,
        "bridge_type": "desktop",
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "capabilities": [
            {"name": "screen_capture", "version": "1.0"},
            {"name": "webview", "version": "1.0"},
            {"name": "tray", "version": "1.0"},
            {"name": "global_hotkey", "version": "1.0"},
            {"name": "notification", "version": "1.0"}
        ]
    }))
}
```

**Step 2: Commit**

```bash
git add apps/desktop/src-tauri/src/bridge/mod.rs
git commit -m "bridge: add aleph.handshake and system.ping handlers"
```

---

### Task 5: Add Bridge-Mode Flag to Tauri App

**Files:**
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs`

**Step 1: Parse `--bridge-mode` CLI flag**

When launched by the server, the bridge receives `--bridge-mode --socket <path> --server-port <port>`. Modify `apps/desktop/src-tauri/src/lib.rs`:

```rust
/// Bridge mode configuration parsed from CLI args
#[derive(Debug, Clone)]
pub struct BridgeModeConfig {
    pub socket_path: Option<String>,
    pub server_port: Option<u16>,
}

/// Parse bridge-mode args from command line
pub fn parse_bridge_args() -> Option<BridgeModeConfig> {
    let args: Vec<String> = std::env::args().collect();
    if !args.iter().any(|a| a == "--bridge-mode") {
        return None;
    }

    let socket_path = args.iter()
        .position(|a| a == "--socket")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let server_port = args.iter()
        .position(|a| a == "--server-port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    Some(BridgeModeConfig { socket_path, server_port })
}
```

**Step 2: Use bridge config in startup**

In the `run()` function, check bridge mode and adjust behavior:

```rust
pub fn run() {
    let bridge_config = parse_bridge_args();
    let is_bridge_mode = bridge_config.is_some();

    // ... existing builder setup ...

    // In setup closure:
    if let Some(ref config) = bridge_config {
        // Override socket path if provided
        if let Some(ref path) = config.socket_path {
            std::env::set_var("ALEPH_SOCKET_PATH", path);
        }
        // Override server port for WebView URLs
        if let Some(port) = config.server_port {
            std::env::set_var("ALEPH_SERVER_PORT", port.to_string());
        }
    }
}
```

**Step 3: Commit**

```bash
git add apps/desktop/src-tauri/src/lib.rs apps/desktop/src-tauri/src/main.rs
git commit -m "bridge: add --bridge-mode CLI flag for subprocess mode"
```

---

### Task 6: Create DesktopBridgeManager in Server

**Files:**
- Create: `core/src/gateway/bridge/desktop_manager.rs`
- Modify: `core/src/gateway/bridge/mod.rs`

**Step 1: Create the desktop bridge manager**

Create `core/src/gateway/bridge/desktop_manager.rs`:

```rust
//! Desktop Bridge Manager
//!
//! Manages the lifecycle of the desktop bridge (Tauri) subprocess.
//! Uses [`BridgeSupervisor`] to spawn, monitor, and restart the bridge.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::{debug, error, info, warn};

use super::supervisor::{BridgeSupervisor, ManagedProcessConfig};
use super::types::TransportType;
use crate::gateway::link::LinkId;
use crate::gateway::transport::{Transport, TransportError};

/// Well-known link ID for the desktop bridge
const DESKTOP_BRIDGE_LINK_ID: &str = "desktop-bridge";

/// Manager for the desktop bridge subprocess
pub struct DesktopBridgeManager {
    supervisor: BridgeSupervisor,
    transport: Option<Arc<dyn Transport>>,
    server_port: u16,
}

impl DesktopBridgeManager {
    /// Create a new manager
    pub fn new(run_dir: PathBuf, server_port: u16) -> Self {
        Self {
            supervisor: BridgeSupervisor::new(run_dir),
            transport: None,
            server_port,
        }
    }

    /// Start the desktop bridge subprocess
    ///
    /// Locates the `aleph-bridge` binary, spawns it with `--bridge-mode`,
    /// and establishes IPC connection via the supervisor.
    pub async fn start(&mut self) -> Result<(), String> {
        let bridge_exe = Self::find_bridge_binary()
            .ok_or_else(|| "aleph-bridge binary not found".to_string())?;

        info!(exe = %bridge_exe.display(), "Starting desktop bridge");

        let link_id = LinkId::new(DESKTOP_BRIDGE_LINK_ID);
        let config = ManagedProcessConfig {
            executable: bridge_exe,
            args: vec![
                "--bridge-mode".into(),
                "--server-port".into(),
                self.server_port.to_string(),
            ],
            transport_type: TransportType::UnixSocket,
            max_restarts: 3,
            restart_delay: Duration::from_secs(5),
            health_check_interval: Duration::from_secs(30),
            env_vars: Default::default(),
        };

        match self.supervisor.spawn(&link_id, config).await {
            Ok(transport) => {
                info!("Desktop bridge started and connected");
                self.transport = Some(transport);
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "Failed to start desktop bridge — running headless");
                Err(e.to_string())
            }
        }
    }

    /// Stop the desktop bridge
    pub async fn stop(&mut self) {
        let link_id = LinkId::new(DESKTOP_BRIDGE_LINK_ID);

        // Try graceful shutdown first
        if let Some(ref transport) = self.transport {
            let _ = transport.request("bridge.shutdown", json!({})).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        if let Err(e) = self.supervisor.stop(&link_id).await {
            warn!(error = %e, "Error stopping desktop bridge");
        }
        self.transport = None;
        info!("Desktop bridge stopped");
    }

    /// Check if the bridge is connected
    pub fn is_connected(&self) -> bool {
        self.transport.as_ref().map_or(false, |t| t.is_connected())
    }

    /// Check if a specific capability is available
    pub fn has_capability(&self, _name: &str) -> bool {
        // TODO: Parse capabilities from handshake response
        self.is_connected()
    }

    /// Send a request to the bridge
    pub async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, TransportError> {
        let transport = self.transport.as_ref()
            .ok_or(TransportError::NotConnected)?;
        transport.request(method, params).await
    }

    /// Show the WebView panel
    pub async fn show_panel(&self, url: &str) -> Result<(), TransportError> {
        self.call("webview.show", json!({"label": "halo", "url": url})).await?;
        Ok(())
    }

    /// Hide the WebView panel
    pub async fn hide_panel(&self) -> Result<(), TransportError> {
        self.call("webview.hide", json!({"label": "halo"})).await?;
        Ok(())
    }

    /// Find the bridge binary
    ///
    /// Search order:
    /// 1. Same directory as server binary
    /// 2. `aleph-bridge` in PATH
    fn find_bridge_binary() -> Option<PathBuf> {
        // 1. Same directory as current executable
        if let Ok(exe) = std::env::current_exe() {
            let sibling = exe.parent()
                .map(|p| p.join("aleph-bridge"));
            if let Some(ref path) = sibling {
                if path.exists() {
                    return sibling;
                }
                // Try with platform extension
                let with_ext = p.join("aleph-bridge.app/Contents/MacOS/aleph-bridge");
                if with_ext.exists() {
                    return Some(with_ext);
                }
            }
        }

        // 2. PATH lookup
        which::which("aleph-bridge").ok()
    }
}
```

**Step 2: Export from bridge module**

Add to `core/src/gateway/bridge/mod.rs`:

```rust
pub mod desktop_manager;
```

**Step 3: Commit**

```bash
git add core/src/gateway/bridge/desktop_manager.rs core/src/gateway/bridge/mod.rs
git commit -m "core: add DesktopBridgeManager for spawning Tauri bridge"
```

---

### Task 7: Wire Bridge Manager into Server Startup

**Files:**
- Modify: `core/src/bin/aleph_server/commands.rs` (or wherever `start_server` lives)

**Step 1: Find the server startup code**

Read `core/src/bin/aleph_server/` to find where the gateway starts. The bridge manager should be initialized after the Gateway HTTP server starts (so server_port is known).

**Step 2: Add bridge manager initialization**

After Gateway is listening:

```rust
// Start desktop bridge (non-blocking — server runs headless if bridge not found)
let run_dir = dirs::home_dir()
    .unwrap_or_else(|| PathBuf::from("/tmp"))
    .join(".aleph")
    .join("run");

let mut bridge_manager = DesktopBridgeManager::new(run_dir, server_port);
if let Err(e) = bridge_manager.start().await {
    tracing::warn!("Desktop bridge not started: {e} — running headless");
}

// Store bridge_manager in server state for tool access
```

**Step 3: Add graceful shutdown**

In the server shutdown handler:

```rust
bridge_manager.stop().await;
```

**Step 4: Commit**

```bash
git add core/src/bin/aleph_server/
git commit -m "server: wire DesktopBridgeManager into startup and shutdown"
```

---

### Task 8: Verify End-to-End

**Step 1: Build bridge**

```bash
cd apps/desktop && cargo build --release
```

Expected: Successful build of `aleph-bridge` binary.

**Step 2: Build server**

```bash
cd core && cargo build --bin aleph-server --features control-plane
```

Expected: Successful build with `DesktopBridgeManager` compiled in.

**Step 3: Manual integration test**

```bash
# Terminal 1: Start server (will auto-spawn bridge)
cargo run --bin aleph-server --features control-plane

# Expected logs:
# INFO Starting desktop bridge
# INFO Desktop bridge started and connected
# (or)
# WARN Desktop bridge not started: ... — running headless
```

**Step 4: Test bridge independently**

```bash
# Start bridge standalone (for development)
cd apps/desktop && cargo run

# In another terminal, test UDS:
echo '{"jsonrpc":"2.0","id":"1","method":"desktop.ping"}' | socat - UNIX-CONNECT:~/.aleph/bridge.sock
# Expected: {"jsonrpc":"2.0","id":"1","result":"pong"}

echo '{"jsonrpc":"2.0","id":"2","method":"aleph.handshake","params":{"protocol_version":"1.0"}}' | socat - UNIX-CONNECT:~/.aleph/bridge.sock
# Expected: {"jsonrpc":"2.0","id":"2","result":{"protocol_version":"1.0","bridge_type":"desktop","platform":"macos",...}}
```

**Step 5: Commit**

```bash
git add -A
git commit -m "phase1: bridge skeleton complete — server spawns Tauri bridge via BridgeSupervisor"
```

---

## Summary of Changes

| File | Action | Purpose |
|------|--------|---------|
| `shared/protocol/src/desktop_bridge.rs` | Modify | Rename socket, add WebView/capability types |
| `apps/desktop/src-tauri/src/bridge/mod.rs` | Modify | Add handshake, webview, ping handlers |
| `apps/desktop/src-tauri/src/lib.rs` | Modify | Add AppHandle global, bridge-mode flag |
| `core/src/gateway/bridge/desktop_manager.rs` | Create | DesktopBridgeManager using BridgeSupervisor |
| `core/src/gateway/bridge/mod.rs` | Modify | Export desktop_manager module |
| `core/src/bin/aleph_server/` | Modify | Wire bridge into startup/shutdown |

## Dependencies on Existing Code (Do Not Modify)

- `core/src/gateway/bridge/supervisor.rs` — BridgeSupervisor (spawn, health, restart)
- `core/src/gateway/transport/unix_socket.rs` — UnixSocketTransport (JSON-RPC over UDS)
- `core/src/gateway/transport/traits.rs` — Transport trait
- `apps/desktop/src-tauri/src/bridge/perception.rs` — Screenshot handler
- `apps/desktop/src-tauri/src/tray.rs` — System tray
- `apps/desktop/src-tauri/src/shortcuts.rs` — Global shortcuts
