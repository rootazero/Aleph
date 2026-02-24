# Tauri Cross-Platform Shell & DesktopBridge Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restructure Tauri from a fat client with RPC proxy commands to a thin shell + DesktopBridge driver, symmetric with the macOS Swift app.

**Architecture:** Delete the entire `core/` module (RPC proxy commands, GatewayState, event_handler — all dead code since React was removed). Add a new `bridge/` module implementing a UDS JSON-RPC 2.0 server on `~/.aleph/desktop.sock`. The `commands/` module is retained (window management used by tray.rs + shortcuts.rs). Protocol types are shared via `aleph-protocol` crate.

**Tech Stack:** Rust, Tauri 2, tokio UnixListener, aleph-protocol, serde_json

**Design doc:** `docs/plans/2026-02-24-tauri-cross-platform-shell-design.md`

---

## Task 1: Delete `core/` module and clean up lib.rs

**Files:**
- Delete: `apps/desktop/src-tauri/src/core/mod.rs`
- Delete: `apps/desktop/src-tauri/src/core/state.rs`
- Delete: `apps/desktop/src-tauri/src/core/event_handler.rs`
- Delete: `apps/desktop/src-tauri/src/core/event_handler.rs.backup`
- Delete: `apps/desktop/src-tauri/src/core/mod.rs.backup`
- Delete: `apps/desktop/src-tauri/src/core/` (directory)
- Modify: `apps/desktop/src-tauri/src/lib.rs`

**Step 1: Delete the core/ directory**

```bash
rm -rf apps/desktop/src-tauri/src/core/
```

**Step 2: Rewrite lib.rs to remove all core references**

Remove `mod core;` declaration, remove `.manage(core::GatewayState::new())`, remove the Gateway initialization block (lines 48-61), remove the settings window close handler that references commands indirectly through core (but keep the `commands::save_window_position` reference), and strip the `invoke_handler` down to only `commands::*` functions.

New `lib.rs`:

```rust
mod commands;
mod error;
mod settings;
mod shortcuts;
mod tray;

use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub fn run() {
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aleph_tauri=debug,tauri=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();

    tracing::info!("Starting Aleph Tauri application");

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let _tray = tray::create_tray(app.handle())?;

            if let Err(e) = shortcuts::register_shortcuts(app.handle()) {
                tracing::error!("Failed to register shortcuts: {:?}", e);
            }

            let halo_window = app.get_webview_window("halo");
            let settings_window = app.get_webview_window("settings");

            tracing::info!(
                "Windows initialized - halo: {}, settings: {}",
                halo_window.is_some(),
                settings_window.is_some()
            );

            if let Some(settings) = settings_window {
                let app_handle = app.handle().clone();
                settings.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { .. } = event {
                        let handle = app_handle.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = commands::save_window_position(handle, "settings".to_string()).await;
                        });
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_version,
            commands::get_cursor_position,
            commands::show_halo_window,
            commands::hide_halo_window,
            commands::open_settings_window,
            commands::get_settings,
            commands::save_settings,
            commands::save_window_position,
            commands::get_window_position,
            commands::send_notification,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled,
            commands::get_aleph_paths,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**Step 3: Simplify error.rs — remove Gateway-specific error variants**

Remove these variants from `AlephError`:
- `Connection(String)`
- `Auth(String)`
- `RPC(String)`
- `NotInitialized(String)`
- `InvalidResponse(String)`

Add a new variant for the bridge:
- `Bridge(String)`

New `error.rs`:

```rust
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Serialize)]
pub enum AlephError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("API timeout after {0}ms")]
    Timeout(u64),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Window error: {0}")]
    Window(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Bridge error: {0}")]
    Bridge(String),

    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl From<std::io::Error> for AlephError {
    fn from(e: std::io::Error) -> Self {
        AlephError::Io(e.to_string())
    }
}

impl From<serde_json::Error> for AlephError {
    fn from(e: serde_json::Error) -> Self {
        AlephError::Serialization(e.to_string())
    }
}

impl From<tauri::Error> for AlephError {
    fn from(e: tauri::Error) -> Self {
        AlephError::Window(e.to_string())
    }
}

impl From<AlephError> for String {
    fn from(e: AlephError) -> Self {
        serde_json::to_string(&e).unwrap_or_else(|_| e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AlephError>;
```

**Step 4: Update Cargo.toml — remove dead dependencies**

Remove these dependencies:
- `aleph-client-sdk` (commented out, but also remove the comment)
- `async-trait` (only used by core/mod.rs ConfigStore impl)
- `tauri-plugin-fs` (no frontend file operations)
- `base64` (only used by core/mod.rs FFI serialization)

Also remove `aleph-protocol` for now (will be re-added in Task 2).

Updated `[dependencies]` section:

```toml
[dependencies]
# Tauri
tauri = { version = "2", features = ["tray-icon", "image-png"] }
tauri-plugin-global-shortcut = "2"
tauri-plugin-shell = "2"
tauri-plugin-dialog = "2"
tauri-plugin-notification = "2"
tauri-plugin-autostart = "2"
tauri-plugin-store = "2"

# Directories
dirs = "5"

# Async
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Error handling
thiserror = "1"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Utilities
uuid = { version = "1.7", features = ["v4"] }
```

Keep platform-specific deps (`cocoa`/`objc` for macOS, `windows` for Windows) as they are used by `commands/mod.rs::get_cursor_position`.

**Step 5: Delete Cargo.toml.bak**

```bash
rm apps/desktop/src-tauri/Cargo.toml.bak
```

**Step 6: Verify it compiles**

```bash
cd apps/desktop/src-tauri && cargo check
```

Expected: compiles with no errors.

**Step 7: Commit**

```bash
git add -A apps/desktop/src-tauri/
git commit -m "refactor(desktop): delete RPC proxy commands and clean up dead code (~1700 lines)"
```

---

## Task 2: Add desktop_bridge protocol types to shared/protocol

**Files:**
- Create: `shared/protocol/src/desktop_bridge.rs`
- Modify: `shared/protocol/src/lib.rs`

**Step 1: Create desktop_bridge.rs**

```rust
//! Desktop Bridge protocol types
//!
//! Shared type definitions for the Desktop Bridge JSON-RPC 2.0 protocol.
//! Used by:
//! - Tauri Bridge (UDS server, direct Rust import)
//! - Core (UDS client, direct Rust import)
//! - Swift Bridge (manual alignment)

use serde::{Deserialize, Serialize};

// ============================================================================
// JSON-RPC 2.0 Types
// ============================================================================

/// JSON-RPC 2.0 request for Desktop Bridge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 success response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSuccessResponse {
    pub jsonrpc: String,
    pub id: String,
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeErrorResponse {
    pub jsonrpc: String,
    pub id: String,
    pub error: BridgeRpcError,
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeRpcError {
    pub code: i32,
    pub message: String,
}

// ============================================================================
// Shared Value Types
// ============================================================================

/// Screen region for screenshot/OCR
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Canvas overlay position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

// ============================================================================
// Method Constants
// ============================================================================

pub const METHOD_PING: &str = "desktop.ping";
pub const METHOD_SCREENSHOT: &str = "desktop.screenshot";
pub const METHOD_OCR: &str = "desktop.ocr";
pub const METHOD_AX_TREE: &str = "desktop.ax_tree";
pub const METHOD_CLICK: &str = "desktop.click";
pub const METHOD_TYPE_TEXT: &str = "desktop.type_text";
pub const METHOD_KEY_COMBO: &str = "desktop.key_combo";
pub const METHOD_LAUNCH_APP: &str = "desktop.launch_app";
pub const METHOD_WINDOW_LIST: &str = "desktop.window_list";
pub const METHOD_FOCUS_WINDOW: &str = "desktop.focus_window";
pub const METHOD_CANVAS_SHOW: &str = "desktop.canvas_show";
pub const METHOD_CANVAS_HIDE: &str = "desktop.canvas_hide";
pub const METHOD_CANVAS_UPDATE: &str = "desktop.canvas_update";

// ============================================================================
// Error Codes
// ============================================================================

pub const ERR_PARSE: i32 = -32700;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INTERNAL: i32 = -32603;
pub const ERR_NOT_IMPLEMENTED: i32 = -32000;

// ============================================================================
// Socket Path
// ============================================================================

/// Get the default Desktop Bridge socket path (~/.aleph/desktop.sock)
pub fn default_socket_path() -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    home.join(".aleph").join("desktop.sock")
}
```

**Step 2: Add `dirs` dependency to shared/protocol Cargo.toml**

Add under `[dependencies]`:
```toml
dirs = "5"
```

**Step 3: Register module in lib.rs**

Add to `shared/protocol/src/lib.rs`:

```rust
pub mod desktop_bridge;
```

And add re-exports:

```rust
pub use desktop_bridge::{
    BridgeRequest, BridgeSuccessResponse, BridgeErrorResponse, BridgeRpcError,
    ScreenRegion, CanvasPosition,
};
```

**Step 4: Verify it compiles**

```bash
cd shared/protocol && cargo check
```

Expected: compiles with no errors.

**Step 5: Commit**

```bash
git add shared/protocol/
git commit -m "feat(protocol): add desktop_bridge types for cross-platform Bridge"
```

---

## Task 3: Create bridge/ module with UDS server and ping

**Files:**
- Create: `apps/desktop/src-tauri/src/bridge/mod.rs`
- Create: `apps/desktop/src-tauri/src/bridge/protocol.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src-tauri/Cargo.toml`

**Step 1: Re-add aleph-protocol dependency to Cargo.toml**

Add back under `[dependencies]`:
```toml
aleph-protocol = { path = "../../../shared/protocol" }
```

**Step 2: Create bridge/protocol.rs — helper functions**

```rust
//! JSON-RPC 2.0 protocol helpers for Desktop Bridge

use aleph_protocol::desktop_bridge::{
    BridgeErrorResponse, BridgeRpcError, BridgeRequest, BridgeSuccessResponse,
    ERR_INTERNAL, ERR_METHOD_NOT_FOUND, ERR_PARSE,
};
use serde_json::Value;

/// Parse a JSON line into a BridgeRequest
pub fn parse_request(line: &str) -> Result<BridgeRequest, BridgeErrorResponse> {
    serde_json::from_str::<BridgeRequest>(line).map_err(|e| BridgeErrorResponse {
        jsonrpc: "2.0".into(),
        id: "null".into(),
        error: BridgeRpcError {
            code: ERR_PARSE,
            message: format!("Parse error: {}", e),
        },
    })
}

/// Create a success response
pub fn success_response(id: &str, result: Value) -> String {
    let resp = BridgeSuccessResponse {
        jsonrpc: "2.0".into(),
        id: id.into(),
        result,
    };
    serde_json::to_string(&resp).unwrap_or_else(|_| error_response_str(id, ERR_INTERNAL, "encode failed"))
}

/// Create an error response
pub fn error_response(id: &str, code: i32, message: &str) -> String {
    error_response_str(id, code, message)
}

fn error_response_str(id: &str, code: i32, message: &str) -> String {
    let resp = BridgeErrorResponse {
        jsonrpc: "2.0".into(),
        id: id.into(),
        error: BridgeRpcError {
            code,
            message: message.into(),
        },
    };
    serde_json::to_string(&resp).unwrap_or_else(|_| {
        format!(
            r#"{{"jsonrpc":"2.0","id":"{}","error":{{"code":{},"message":"encode failed"}}}}"#,
            id, code
        )
    })
}

/// Create a "method not found" error for unimplemented methods
pub fn not_implemented_response(id: &str, method: &str) -> String {
    error_response(
        id,
        ERR_METHOD_NOT_FOUND,
        &format!("{} not implemented on this platform", method),
    )
}
```

**Step 3: Create bridge/mod.rs — UDS server + dispatch**

```rust
//! Desktop Bridge — UDS JSON-RPC 2.0 server
//!
//! Symmetric with macOS Swift DesktopBridgeServer.
//! Listens on ~/.aleph/desktop.sock, dispatches JSON-RPC requests
//! to perception/action handlers.

pub mod protocol;

use aleph_protocol::desktop_bridge::{self, ERR_METHOD_NOT_FOUND};
use serde_json::json;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{error, info, warn};

/// Start the Desktop Bridge UDS server
///
/// Listens on ~/.aleph/desktop.sock and dispatches JSON-RPC 2.0 requests.
/// This function runs forever; call it from a spawned task.
pub async fn start_bridge_server() {
    let socket_path = desktop_bridge::default_socket_path();

    // Ensure ~/.aleph/ directory exists
    if let Some(parent) = socket_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            error!("Failed to create directory {:?}: {}", parent, e);
            return;
        }
    }

    // Remove stale socket file
    let _ = tokio::fs::remove_file(&socket_path).await;

    // Bind listener
    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind UDS {:?}: {}", socket_path, e);
            return;
        }
    };

    // Restrict socket file to owner-only access
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        if let Err(e) = std::fs::set_permissions(&socket_path, perms) {
            warn!("Failed to set socket permissions: {}", e);
        }
    }

    info!("DesktopBridge listening on {:?}", socket_path);

    // Accept loop
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    handle_connection(stream).await;
                });
            }
            Err(e) => {
                error!("Failed to accept connection: {}", e);
            }
        }
    }
}

/// Handle a single connection: read one line, dispatch, write response
async fn handle_connection(stream: tokio::net::UnixStream) {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    match buf_reader.read_line(&mut line).await {
        Ok(0) | Err(_) => return,
        Ok(_) => {}
    }

    let line = line.trim_end();
    if line.is_empty() {
        return;
    }

    let response = match protocol::parse_request(line) {
        Ok(req) => {
            let result = dispatch(&req.method, req.params.unwrap_or(json!({})));
            match result {
                Ok(value) => protocol::success_response(&req.id, value),
                Err((code, msg)) => protocol::error_response(&req.id, code, &msg),
            }
        }
        Err(err_resp) => serde_json::to_string(&err_resp).unwrap_or_default(),
    };

    let response_line = format!("{}\n", response);
    let _ = writer.write_all(response_line.as_bytes()).await;
}

/// Dispatch a method call to the appropriate handler
fn dispatch(method: &str, _params: serde_json::Value) -> Result<serde_json::Value, (i32, String)> {
    match method {
        desktop_bridge::METHOD_PING => Ok(json!("pong")),

        // All other methods return "not implemented" for MVP
        desktop_bridge::METHOD_SCREENSHOT
        | desktop_bridge::METHOD_OCR
        | desktop_bridge::METHOD_AX_TREE
        | desktop_bridge::METHOD_CLICK
        | desktop_bridge::METHOD_TYPE_TEXT
        | desktop_bridge::METHOD_KEY_COMBO
        | desktop_bridge::METHOD_LAUNCH_APP
        | desktop_bridge::METHOD_WINDOW_LIST
        | desktop_bridge::METHOD_FOCUS_WINDOW
        | desktop_bridge::METHOD_CANVAS_SHOW
        | desktop_bridge::METHOD_CANVAS_HIDE
        | desktop_bridge::METHOD_CANVAS_UPDATE => Err((
            ERR_METHOD_NOT_FOUND,
            format!("{} not implemented on this platform", method),
        )),

        _ => Err((
            ERR_METHOD_NOT_FOUND,
            format!("Method not found: {}", method),
        )),
    }
}
```

**Step 4: Wire bridge into lib.rs**

Add `mod bridge;` at the top of `lib.rs` and add bridge startup to the `setup` closure, after shortcuts registration:

```rust
// Start Desktop Bridge UDS server
tauri::async_runtime::spawn(async {
    bridge::start_bridge_server().await;
});
```

**Step 5: Verify it compiles**

```bash
cd apps/desktop/src-tauri && cargo check
```

Expected: compiles with no errors. Note: `UnixListener` requires `cfg(unix)`. For Windows, the UDS code will need a `#[cfg(unix)]` guard. For MVP, this is acceptable — Windows named pipes can be added later.

**Step 6: Commit**

```bash
git add apps/desktop/src-tauri/
git commit -m "feat(desktop): add DesktopBridge UDS server with ping support"
```

---

## Task 4: Manual integration test

**Files:** None (testing only)

**Step 1: Build and run the Tauri app**

This step requires the Aleph Server to be running (for Leptos UI). If server is not available, just verify the binary starts without crash:

```bash
cd apps/desktop/src-tauri && cargo build
```

**Step 2: Test UDS communication manually**

In a separate terminal, send a ping request to the bridge:

```bash
echo '{"jsonrpc":"2.0","id":"test-1","method":"desktop.ping"}' | socat - UNIX-CONNECT:~/.aleph/desktop.sock
```

Expected response:
```json
{"jsonrpc":"2.0","id":"test-1","result":"pong"}
```

**Step 3: Test method not found**

```bash
echo '{"jsonrpc":"2.0","id":"test-2","method":"desktop.screenshot"}' | socat - UNIX-CONNECT:~/.aleph/desktop.sock
```

Expected response:
```json
{"jsonrpc":"2.0","id":"test-2","error":{"code":-32601,"message":"desktop.screenshot not implemented on this platform"}}
```

**Step 4: Test parse error**

```bash
echo 'not json' | socat - UNIX-CONNECT:~/.aleph/desktop.sock
```

Expected response:
```json
{"jsonrpc":"2.0","id":"null","error":{"code":-32700,"message":"Parse error: ..."}}
```

---

## Task 5: Final cleanup and verification

**Files:**
- Verify: all files in `apps/desktop/src-tauri/src/`

**Step 1: Run cargo clippy**

```bash
cd apps/desktop/src-tauri && cargo clippy -- -D warnings
```

Fix any warnings.

**Step 2: Verify final directory structure**

```
apps/desktop/src-tauri/src/
├── main.rs
├── lib.rs
├── error.rs
├── settings.rs
├── shortcuts.rs
├── tray.rs
├── commands/
│   └── mod.rs
└── bridge/
    ├── mod.rs
    └── protocol.rs
```

No `core/` directory. No `.backup` files. No dead code.

**Step 3: Commit if any clippy fixes were made**

```bash
git add apps/desktop/src-tauri/
git commit -m "fix(desktop): address clippy warnings"
```

---

## Summary

| Task | Description | ~Lines Changed |
|------|-------------|----------------|
| 1 | Delete core/, clean lib.rs, simplify error.rs, prune Cargo.toml | -1700, +80 |
| 2 | Add desktop_bridge types to shared/protocol | +120 |
| 3 | Create bridge/ with UDS server + ping dispatch | +180 |
| 4 | Manual integration test (no code changes) | 0 |
| 5 | Clippy cleanup | ~10 |

**Net result:** ~1300 lines of dead code removed, ~380 lines of new focused code added.
