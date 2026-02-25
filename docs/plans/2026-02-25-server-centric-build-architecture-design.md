# Server-Centric Build Architecture Design

> Date: 2026-02-25
> Status: Approved
> Supersedes: Current macOS Swift App architecture

## Summary

Aleph adopts a **Daemon + Bridge** architecture where `aleph-server` (single Rust binary) is the brain and `aleph-bridge` (Tauri, cross-platform) is the body. The macOS Swift app (`apps/macos/`) is deprecated in favor of a unified Tauri bridge.

## Core Principle

> App exists for **Aleph to operate the system**, not for users to operate the system. Users interact only through Chat (social bots + Panel chat window).

## Architecture

```
┌─────────────────────────────────────────────────┐
│              aleph-server (Rust)                 │
│  ┌───────────┐ ┌──────────┐ ┌────────────────┐  │
│  │ Core Brain│ │ Gateway  │ │ Panel UI       │  │
│  │ Agent Loop│ │ WS:18789 │ │ Leptos/WASM    │  │
│  │ Memory    │ │ JSON-RPC │ │ (rust-embed)   │  │
│  │ Tools     │ │ HTTP     │ │ Chat + Settings│  │
│  │ Providers │ │ mDNS     │ │ + Dashboard    │  │
│  └─────┬─────┘ └────┬─────┘ └────────────────┘  │
│        │       ┌─────┴──────┐                    │
│        │       │ Bridge Mgr │ spawn + supervise  │
│        │       └─────┬──────┘                    │
└────────┼─────────────┼───────────────────────────┘
         │         UDS/IPC (AF_UNIX)
         │         ┌───┴────────────────────────┐
         │         │    aleph-bridge (Tauri)     │
         │         │  ┌──────────┐ ┌──────────┐  │
         │         │  │ WebView  │ │ Desktop  │  │
         │         │  │ (Panel)  │ │ Caps     │  │
         │         │  └──────────┘ │ OCR      │  │
         │         │  ┌──────────┐ │ Keyboard │  │
         │         │  │ Tray Icon│ │ Screen   │  │
         │         │  └──────────┘ │ AX API   │  │
         │         │               └──────────┘  │
         │         └─────────────────────────────┘
    WebSocket
    ┌────┴─────────────┐
    │ Social Bots      │
    │ CLI Client       │
    └──────────────────┘
```

## Build Model

### Build Commands

```bash
# Brain: single Rust binary, all platforms
cargo build --bin aleph-server --features control-plane

# Body: Tauri binary, all platforms
cd apps/desktop && cargo tauri build
```

### Distribution Package

```
aleph-v{version}-{platform}-{arch}/
├── aleph-server              # Rust binary (~20-30MB)
├── aleph-bridge{.app/.exe}   # Tauri binary (~5-10MB)
└── install script            # Platform-specific installer
```

### Startup Flow

1. User starts `aleph-server`
2. Server initializes Core Brain + Gateway + Panel UI
3. Server creates UDS listener at `~/.aleph/bridge.sock`
4. Server locates and spawns `aleph-bridge` subprocess
5. Bridge connects to UDS, registers capabilities
6. Bridge creates Tray icon (status indicator)
7. Bridge WebView ready (awaiting show command)
8. User presses hotkey → Bridge shows WebView → loads Panel UI from server

## Server ↔ Bridge Communication

### Protocol: AF_UNIX Socket + JSON-RPC 2.0

```
Unified socket path (all platforms):
  {HOME}/.aleph/bridge.sock
```

Windows 10 1803+ supports AF_UNIX, so no platform-specific pipe needed.

```rust
fn bridge_socket_path() -> PathBuf {
    dirs::home_dir().unwrap().join(".aleph").join("bridge.sock")
}
```

### RPC Methods

| Direction | Prefix | Examples | Purpose |
|-----------|--------|----------|---------|
| Server → Bridge | `desktop.*` | `desktop.screenshot`, `desktop.type_text`, `desktop.click` | System operations |
| Server → Bridge | `webview.*` | `webview.show`, `webview.hide`, `webview.navigate` | Panel window control |
| Server → Bridge | `tray.*` | `tray.update_status`, `tray.set_menu` | Tray state updates |
| Bridge → Server | `hotkey.*` | `hotkey.triggered` | Hotkey event reporting |
| Bridge → Server | `tray.*` | `tray.clicked` | Tray interaction events |
| Bridge → Server | `bridge.*` | `bridge.ready`, `bridge.error` | Lifecycle events |
| Bidirectional | `capability.*` | `capability.register`, `capability.query` | Capability negotiation |

### Capability Negotiation

Bridge registers capabilities on connect. Server never assumes what Bridge can do.

```json
{
  "method": "capability.register",
  "params": {
    "platform": "macos",
    "arch": "aarch64",
    "capabilities": [
      {"name": "screen_capture", "version": "1.0"},
      {"name": "keyboard_control", "version": "1.0"},
      {"name": "mouse_control", "version": "1.0"},
      {"name": "ax_inspect", "version": "1.0"},
      {"name": "ocr", "version": "1.0"},
      {"name": "webview", "version": "1.0"},
      {"name": "tray", "version": "1.0"},
      {"name": "notification", "version": "1.0"},
      {"name": "global_hotkey", "version": "1.0"}
    ]
  }
}
```

### Graceful Degradation

Server runs fully without Bridge:
- Gateway, Panel UI, social bots, CLI all functional
- Desktop capability tools return `CapabilityUnavailable`
- Agent adapts: suggests manual alternatives when no "hands" available

## Bridge Internal Architecture

```
apps/desktop/src-tauri/
├── src/
│   ├── main.rs                 # Entry: connect to server, start Tauri
│   ├── ipc_client.rs           # UDS connection to aleph-server
│   ├── capability_registry.rs  # Capability registration and dispatch
│   ├── commands/               # Desktop capability implementations
│   │   ├── screen_capture.rs   # xcap crate
│   │   ├── keyboard.rs         # enigo / rdev
│   │   ├── mouse.rs            # enigo
│   │   ├── ocr.rs              # Platform OCR
│   │   └── notification.rs     # Tauri notification plugin
│   ├── platform/               # Platform-specific code (#[cfg])
│   │   ├── macos.rs            # AX API (objc2), Vision OCR
│   │   ├── linux.rs            # X11/Wayland specifics
│   │   └── windows.rs          # Win32 specifics
│   └── tray.rs                 # System tray management
├── Cargo.toml
└── tauri.conf.json
```

Key design decisions:
- **No frontend code** — WebView loads `http://localhost:{port}` directly from server
- **No React/pnpm** — Bridge has zero JavaScript dependencies
- **macOS AX/Vision** — via `objc2` crate in Rust, no Swift needed

## Bridge Manager (Server Side)

New module: `core/src/gateway/bridge/manager.rs`

```rust
pub struct BridgeManager {
    socket_path: PathBuf,           // ~/.aleph/bridge.sock
    bridge_process: Option<Child>,  // Subprocess handle
    connection: Option<UdsStream>,  // Active connection
    capabilities: Vec<Capability>,  // Registered capabilities
    state: BridgeState,             // disconnected/connecting/ready
}
```

Lifecycle:
1. Create UDS listener (bind `~/.aleph/bridge.sock`)
2. Locate bridge binary (same dir → PATH → warn if not found)
3. Spawn bridge subprocess with `--socket` and `--server-port` args
4. Wait for UDS connection (10s timeout)
5. Capability negotiation → state = ready
6. Auto-restart on unexpected exit (max 3, exponential backoff)
7. Graceful shutdown: send `bridge.shutdown` → wait → kill

## Agent Integration

```rust
// Tools check bridge capability before execution
impl AlephTool for ScreenshotTool {
    async fn execute(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        let bridge = ctx.bridge_manager();
        if !bridge.has_capability("screen_capture") {
            return Err(ToolError::capability_unavailable("screen_capture"));
        }
        let result: ScreenshotResult = bridge
            .call("desktop.screenshot", json!({"region": "full"}))
            .await?;
        Ok(ToolOutput::image(result.image_data))
    }
}
```

## Responsibility Boundaries

| | aleph-server | aleph-bridge |
|---|---|---|
| **Role** | AI brain + UI service | System operation shell |
| **Contains** | Core Brain, Gateway, Panel UI (WASM), Bridge Manager | WebView container, Desktop Caps, Tray, Hotkey |
| **Forbidden** | Platform-specific API calls (R1) | Business logic (R4) |
| **Build** | `cargo build --bin aleph-server` | `cargo tauri build` |
| **Language** | Pure Rust | Rust (Tauri) + objc2 (macOS supplement) |
| **Standalone** | Yes (headless, no desktop caps) | No (must connect to server) |

## Migration from Swift App

| Current Swift Implementation | Migration Target | Method |
|------------------------------|------------------|--------|
| ScreenCaptureCoordinator (SCKit) | `commands/screen_capture.rs` | xcap crate |
| DesktopBridge/Keyboard (CGEvent) | `commands/keyboard.rs` | enigo crate |
| DesktopBridge/Perception (Vision) | `commands/ocr.rs` + `platform/macos.rs` | objc2 + Vision |
| DesktopBridge/Canvas (WKWebView) | Tauri WebView | Native Tauri |
| HaloWindow (floating window) | Tauri Window API | Transparent/borderless |
| MenuBarManager (NSStatusItem) | `tray.rs` | Tauri system-tray plugin |
| HotkeyManager (global hotkeys) | Tauri global-shortcut plugin | Already available |
| PermissionChecker | `platform/macos.rs` | objc2 + AX/ScreenRecording |
| All SwiftUI business pages | **Deleted** — replaced by Panel UI | Leptos/WASM |
| GatewayClient/WebSocket | `ipc_client.rs` | UDS replaces WebSocket |

## Deprecation List

| Deprecated | Reason |
|------------|--------|
| `apps/macos/` (45+ SwiftUI dirs) | Replaced by Panel UI + Tauri Bridge |
| `core/bindings/` (UniFFI bindings) | UniFFI removed, no FFI needed |
| `libalephcore.dylib` (C FFI) | Server and Bridge are separate processes |

## Retained

| Kept | Reason |
|------|--------|
| `apps/desktop/src-tauri/` | Promoted to cross-platform Bridge |
| `apps/cli/` | Pure I/O reference client |
| `apps/shared/` (SDK) | Client discovery mechanism |
| `core/ui/control_plane/` | Panel UI, the single source of UI truth |

## Implementation Phases

### Phase 1: Tauri Bridge Skeleton
- IPC client + UDS connection to server
- WebView loading server Panel URL
- System tray with status indicator
- Global hotkey to show/hide WebView

### Phase 2: Server Bridge Manager
- UDS listener in server
- Spawn and supervise bridge subprocess
- Capability negotiation protocol
- Graceful degradation when no bridge

### Phase 3: Migrate Desktop Capabilities
- Screen capture (xcap)
- Keyboard/mouse control (enigo)
- OCR (objc2 + Vision on macOS)
- AX inspect (objc2 on macOS)

### Phase 4: Cleanup
- Deprecate `apps/macos/`
- Remove `core/bindings/`
- Update CLAUDE.md architecture docs
- Update build scripts and CI

## Architectural Redline Compliance

- **R1 (Brain-Limb Separation)**: Server contains zero platform APIs. All desktop ops via IPC to Bridge.
- **R2 (Single Source of UI Truth)**: All business UI in Leptos/WASM Panel. Bridge only hosts WebView container + tray.
- **R3 (Core Minimalism)**: No heavy platform dependencies in core.
- **R4 (I/O-Only Interfaces)**: Bridge does zero business logic. Receives commands, executes system operations, returns results.
