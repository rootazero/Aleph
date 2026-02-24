# Tauri Cross-Platform Shell & DesktopBridge Design

> Date: 2026-02-24
> Status: Approved

## Motivation

Aleph's 1-2-3-4 architecture establishes a "multi-shell, one-heart" model where native shells (Swift on macOS, Tauri on Windows/Linux) host a unified Leptos/WASM UI and provide platform-specific desktop capabilities via DesktopBridge.

The macOS Swift implementation is complete: SwiftUI shell + WKWebView loading Leptos + DesktopBridge over UDS. However, the Tauri app still contains ~500 lines of dead RPC proxy commands from its former React frontend, and lacks a DesktopBridge implementation.

This design restructures Tauri from a "fat client with proxy commands" to a "thin shell + bridge driver" — symmetric with the macOS app.

## Architecture

### Tauri's Two Roles

```
┌─────────────────────────────────────────────────────────────┐
│                    apps/desktop (Tauri)                       │
│                                                               │
│  ┌────────────────────────┐    ┌────────────────────────┐    │
│  │  Window Shell           │    │  DesktopBridge          │    │
│  │                        │    │                        │    │
│  │  • WebView loading     │    │  • UDS Server          │    │
│  │    Leptos URLs         │    │  • ~/.aleph/desktop.sock│    │
│  │  • Transparent window  │    │  • JSON-RPC 2.0        │    │
│  │  • System tray         │    │                        │    │
│  │  • Global shortcuts    │    │  perception/           │    │
│  │  • Autostart           │    │  action/ (stubs)       │    │
│  └────────────────────────┘    └────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### Platform Symmetry

| Platform | UI Layer       | Native Shell  | Desktop Capabilities           | Bridge Transport |
|----------|---------------|---------------|-------------------------------|-----------------|
| Web      | Leptos        | Browser       | None (sandbox)                | N/A             |
| macOS    | Leptos (WKWebView) | Swift App | Swift (AppKit/Vision/AX)     | UDS JSON-RPC    |
| Windows  | Leptos (WebView2)  | Tauri     | Rust (windows-rs/UI Automation) | UDS JSON-RPC |
| Linux    | Leptos (WebView)   | Tauri     | Rust (x11-dl/atspi)          | UDS JSON-RPC    |

### Directory Structure (After)

```
apps/desktop/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs              # Entry point (unchanged)
│   │   ├── lib.rs               # Minimal: window + tray + shortcuts + bridge
│   │   ├── tray.rs              # System tray (retained)
│   │   ├── shortcuts.rs         # Global shortcuts (retained)
│   │   ├── settings.rs          # Path utilities (retained)
│   │   ├── error.rs             # Simplified error types
│   │   └── bridge/              # NEW: Desktop Bridge
│   │       ├── mod.rs           # UDS server + method dispatch
│   │       ├── protocol.rs      # Re-export types from aleph-protocol
│   │       └── perception/      # Perception capabilities
│   │           ├── mod.rs
│   │           └── screenshot.rs # MVP implementation
│   └── Cargo.toml               # Updated dependencies
└── .gitignore
```

### Deleted Code

| File | Lines | Reason |
|------|-------|--------|
| `core/mod.rs` | ~567 | RPC proxy commands for deleted React frontend |
| `core/event_handler.rs` | ~30 | Gateway event forwarding to React |
| `core/state.rs` | ~50 | GatewayState for proxy pattern |
| `core/event_handler.rs.backup` | ~400 | Backup file |
| `core/mod.rs.backup` | ~360 | Backup file |
| `commands/mod.rs` | ~280 | Window management + React commands |

Total: ~1700 lines of dead code removed.

## Protocol Layer

### shared/protocol additions

New module `shared/protocol/src/desktop_bridge.rs`:

```rust
use serde::{Deserialize, Serialize};

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
    pub error: BridgeError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: i32,
    pub message: String,
}

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

/// All supported Bridge methods
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
```

Swift side manually aligns with these definitions (already doing so).

## Bridge UDS Server

### Communication Flow

```
Aleph Core (UDS client)
    │
    │  connect to ~/.aleph/desktop.sock
    │  write: {"jsonrpc":"2.0","id":"1","method":"desktop.ping","params":null}\n
    │
    ▼
Tauri Bridge (UDS server)
    │
    │  read line → parse JSON-RPC → dispatch → write response line
    │
    ▼
Response: {"jsonrpc":"2.0","id":"1","result":"pong"}\n
```

### Server Implementation

The UDS server follows the same pattern as Swift's DesktopBridgeServer:

1. Ensure `~/.aleph/` directory exists
2. Remove stale socket file
3. Bind Unix listener to `~/.aleph/desktop.sock`
4. `chmod 0700` for security
5. Accept loop: spawn async task per connection
6. Each connection: read one line, dispatch, write one response line

### Method Dispatch

```rust
fn dispatch(method: &str, params: Value) -> Result<Value, BridgeError> {
    match method {
        "desktop.ping" => Ok(json!("pong")),
        "desktop.screenshot" => perception::screenshot(params),
        // All other methods return "not implemented" in MVP
        _ => Err(BridgeError {
            code: -32601,
            message: format!("{} not implemented on this platform", method),
        }),
    }
}
```

## MVP Scope

### Implemented

| Method | Description | Platform |
|--------|-------------|----------|
| `desktop.ping` | Health check | All |
| `desktop.screenshot` | Capture screen | Windows (windows-rs), Linux (x11/scrap), macOS (CGWindow fallback) |

### Stubbed (Graceful Degradation)

All other methods return structured error:
```json
{"jsonrpc":"2.0","id":"1","error":{"code":-32601,"message":"desktop.ocr not implemented on linux"}}
```

This allows Core to detect platform capabilities and degrade gracefully.

## lib.rs After Restructuring

```rust
pub fn run() {
    // Initialize logging
    // ...

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            // System tray
            let _tray = tray::create_tray(app.handle())?;

            // Global shortcuts
            if let Err(e) = shortcuts::register_shortcuts(app.handle()) {
                tracing::error!("Failed to register shortcuts: {:?}", e);
            }

            // Start Desktop Bridge UDS server
            tauri::async_runtime::spawn(async {
                bridge::start_bridge_server().await;
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

No invoke_handler needed — Leptos UI communicates directly with Gateway via WebSocket.

## Cargo.toml Changes

### Dependencies to Remove

- `aleph-client-sdk` (was: WebSocket client for proxy pattern)
- `async-trait` (no longer needed without GatewayState)
- `tauri-plugin-fs` (no frontend file operations)

### Dependencies to Add

- `tokio::net::UnixListener` (already available via tokio "full")

### Dependencies to Keep

- `tauri` + plugins (shell, dialog, notification, autostart, store, global-shortcut)
- `aleph-protocol` (for bridge types)
- `serde` / `serde_json`
- `tracing`
- `uuid`
- Platform deps: `cocoa`/`objc` (macOS), `windows` (Windows)

## Relationship to 1-2-3-4 Architecture

| Component | 1-2-3-4 Role | Status |
|-----------|-------------|--------|
| Leptos UI | Face (2) | Done |
| Tauri Shell | Face host | Restructuring (this design) |
| DesktopBridge | Limb (3) | MVP (this design) |
| UDS Protocol | Nerve (4) | Symmetric with Swift |
| Core | Brain (1) | Unchanged |

## Implementation Approach

**Phase 1 — Clean**: Delete all RPC proxy commands, FFI types, GatewayState, event_handler. Remove `commands/` module.

**Phase 2 — Protocol**: Add `desktop_bridge.rs` to `shared/protocol`. Re-export in `lib.rs`.

**Phase 3 — Bridge**: Create `bridge/` module with UDS server, dispatch, and `desktop.ping` implementation.

**Phase 4 — Screenshot MVP**: Implement `desktop.screenshot` with platform-specific backends.

**Phase 5 — Integration**: Wire bridge server startup into `lib.rs` setup. Verify with manual UDS test.

## Implementation Result

> Date: 2026-02-24
> Status: Implemented & Verified

### Changes Summary

- **14 files changed, 306 insertions(+), 1,611 deletions(-)**
- Net reduction: 1,305 lines of code

### Commits

```
c355a6c5 chore(desktop): remove unused anyhow and uuid dependencies
1b1e83b8 fix(desktop): use ERR_NOT_IMPLEMENTED for stubbed methods, add debug logging
1c308342 feat(desktop): add DesktopBridge UDS server with ping support
63d8b1e1 feat(protocol): add desktop_bridge types for cross-platform Bridge
1a96f3bd refactor(desktop): delete RPC proxy commands and clean up dead code (~1600 lines)
```

### Verification Tests (2026-02-24)

All tests performed via Python UDS client against running Tauri app.

| Test | Request | Response | Status |
|------|---------|----------|--------|
| Ping | `{"method":"desktop.ping"}` | `{"result":"pong"}` | PASS |
| Stubbed method | `{"method":"desktop.screenshot"}` | `{"error":{"code":-32000,"message":"desktop.screenshot not implemented on this platform"}}` | PASS |
| Unknown method | `{"method":"desktop.does_not_exist"}` | `{"error":{"code":-32601,"message":"Method not found: desktop.does_not_exist"}}` | PASS |
| Parse error | `not json at all` | `{"error":{"code":-32700,"message":"Parse error: ..."}}` | PASS |

Error code semantics verified:
- **-32000** (`ERR_NOT_IMPLEMENTED`): known method, not yet implemented on this platform
- **-32601** (`ERR_METHOD_NOT_FOUND`): unknown method
- **-32700** (`ERR_PARSE`): invalid JSON

### Remaining Work

1. Implement `desktop.screenshot` with platform-specific backends (windows-rs, x11-dl)
2. Implement remaining desktop capabilities (OCR, AXTree, click, typeText, etc.)
3. Add `#[cfg(unix)]` guard for Windows compatibility (named pipes fallback)
