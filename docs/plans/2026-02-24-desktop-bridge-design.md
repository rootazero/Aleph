# Desktop Bridge Design

**Date:** 2026-02-24
**Status:** Approved
**Decision:** Re-enable desktop capabilities via UDS-based Swift Bridge (Method B)

---

## Background

On 2026-02-23, the "Server Purification" refactoring deleted ~13,000 lines of desktop control code
to simplify Aleph Core into a pure server. This included:

- Perception Layer (AX tree, screen capture, OCR, State Bus, PAL)
- Browser module (chromiumoxide CDP)
- Canvas A2UI rendering
- Client routing infrastructure (ExecutionPolicy, ToolRouter, RoutedExecutor)

While architecturally clean, this decision removed Aleph's "hands and feet" — its ability to
operate computers like a human. The original code ran desktop APIs inside Rust Core, creating
complexity and macOS-specific dependencies. The deleted code is archived at
`archive/2026-02-23-desktop-control-code/`.

## Decision

**Restore desktop capabilities via a three-layer architecture** — Rust Core (Brain) communicates
with macOS App (Limbs) over Unix Domain Socket (UDS) using JSON-RPC 2.0. Browser automation
continues through Playwright MCP Server.

This approach was validated by OpenClaw's `control.sock` UDS pattern and confirmed by the
architecture evolution decision record.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     ALEPH AGENT LOOP                             │
│   Observe → Think → Act → Feedback                               │
└────────────────────────────┬────────────────────────────────────┘
                             │ tool call
                    ┌────────▼────────┐
                    │  BuiltinTools   │
                    │  DesktopTool    │
                    └────────┬────────┘
                             │ JSON-RPC 2.0 over UDS
              ╔══════════════▼════════════════╗
              ║  Unix Domain Socket            ║
              ║  ~/.aleph/desktop.sock         ║
              ╚══════════════╤════════════════╝
                             │
┌────────────────────────────▼────────────────────────────────────┐
│                    macOS App (Swift)                              │
│              DesktopBridgeServer                                  │
│                                                                   │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────────────────┐ │
│  │  Perception  │ │   Action     │ │       Canvas             │ │
│  │ ─────────── │ │ ──────────── │ │ ────────────────────────  │ │
│  │ screenshot   │ │ click/type   │ │ WKWebView overlay         │ │
│  │ OCR (Vision) │ │ key combo    │ │ HTML/CSS/JS rendering     │ │
│  │ AX tree      │ │ app launch   │ │ A2UI v0.8 protocol        │ │
│  │ screen rec   │ │ window mgmt  │ │                           │ │
│  └──────────────┘ └──────────────┘ └──────────────────────────┘ │
│                                                                   │
│  TCC permissions: Screen Recording, Accessibility, Microphone    │
└─────────────────────────────────────────────────────────────────┘
                             │ MCP
                    ┌────────▼────────┐
                    │  Playwright MCP │ (browser automation)
                    │  Server Plugin  │
                    └─────────────────┘
```

### Layer Responsibilities

| Layer | Technology | Responsibility |
|-------|-----------|----------------|
| **Brain (Core)** | Rust | Reasoning, planning, memory, UDS client, tool dispatch |
| **Limbs (App)** | Swift + macOS APIs | Perception, action, canvas, TCC permission holder |
| **Tools (MCP)** | Playwright | Browser automation (already configured) |

### Key Design Decisions

1. **UDS over WebSocket**: Local IPC uses Unix Domain Socket for lower latency and simpler
   security model. Socket path: `~/.aleph/desktop.sock`
2. **JSON-RPC 2.0**: Same protocol as the WebSocket Gateway for consistency. Newline-delimited
   messages over the socket stream.
3. **Sidecar Pattern**: macOS App manages the UDS server lifecycle. The embedded Rust Core binary
   (`Aleph.app/Contents/Resources/bin/aleph-core`) ensures IPC protocol version sync.
4. **Graceful degradation**: When macOS App is not running, `DesktopTool` detects the missing
   socket and returns a friendly message instead of crashing.
5. **No code restoration**: The archived desktop code is NOT restored into Rust Core. Swift
   native APIs (Vision.framework, ScreenCaptureKit, NSAccessibility, CGEvent) are cleaner
   than Rust FFI bindings for macOS-specific capabilities.

---

## IPC Protocol

### Request Types (Rust enum)

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum DesktopRequest {
    // Perception
    #[serde(rename = "desktop.screenshot")]
    Screenshot { region: Option<ScreenRegion> },

    #[serde(rename = "desktop.ocr")]
    Ocr { image_base64: Option<String> },   // None = capture current screen

    #[serde(rename = "desktop.ax_tree")]
    AxTree { app_bundle_id: Option<String> },

    #[serde(rename = "desktop.screen_record")]
    ScreenRecord { duration_secs: u32, fps: u8 },

    // Action
    #[serde(rename = "desktop.click")]
    Click { x: f64, y: f64, button: MouseButton },

    #[serde(rename = "desktop.type_text")]
    TypeText { text: String },

    #[serde(rename = "desktop.key_combo")]
    KeyCombo { keys: Vec<String> },         // e.g. ["cmd", "c"]

    #[serde(rename = "desktop.launch_app")]
    LaunchApp { bundle_id: String, args: Vec<String> },

    #[serde(rename = "desktop.window_list")]
    WindowList {},

    #[serde(rename = "desktop.focus_window")]
    FocusWindow { window_id: u32 },

    // Canvas
    #[serde(rename = "desktop.canvas_show")]
    CanvasShow { html: String, position: CanvasPosition },

    #[serde(rename = "desktop.canvas_hide")]
    CanvasHide {},

    #[serde(rename = "desktop.canvas_update")]
    CanvasUpdate { patch: serde_json::Value },  // A2UI RFC 6902 patch
}
```

### Wire Format (JSON-RPC 2.0)

Request:
```json
{"jsonrpc":"2.0","id":"req-1","method":"desktop.screenshot","params":{"region":null}}
```

Response (success):
```json
{"jsonrpc":"2.0","id":"req-1","result":{"image_base64":"...","width":2560,"height":1600}}
```

Response (error):
```json
{"jsonrpc":"2.0","id":"req-1","error":{"code":-32000,"message":"Screen recording permission denied"}}
```

### Swift Implementation Entry Point

```swift
// DesktopBridgeServer.swift
class DesktopBridgeServer {
    let socketPath = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".aleph/desktop.sock").path

    func handleRequest(_ method: String, params: JSON) async -> JSON {
        switch method {
        case "desktop.screenshot":   return await takeScreenshot(params)
        case "desktop.ocr":          return await performOCR(params)
        case "desktop.ax_tree":      return await captureAXTree(params)
        case "desktop.click":        return await simulateClick(params)
        case "desktop.type_text":    return await typeText(params)
        case "desktop.key_combo":    return await pressKeyCombo(params)
        case "desktop.launch_app":   return await launchApp(params)
        case "desktop.window_list":  return await listWindows()
        case "desktop.canvas_show":  return await showCanvas(params)
        default:
            return .error(code: -32601, message: "Method not found: \(method)")
        }
    }
}
```

### macOS APIs Used

| Capability | macOS API |
|-----------|-----------|
| Screenshot | `ScreenCaptureKit` (macOS 13+), fallback `CGWindowListCreateImage` |
| OCR | `Vision.framework` `VNRecognizeTextRequest` |
| Accessibility Tree | `NSAccessibility`, `AXUIElement` |
| Mouse events | `CGEvent(mouseEventSource:)` |
| Keyboard events | `CGEvent(keyboardEventSource:)` |
| App launch | `NSWorkspace.shared.launchApplication(withBundleIdentifier:)` |
| Window management | `NSWorkspace`, `CGWindowListCopyWindowInfo` |
| Canvas rendering | `WKWebView` overlay window |
| Screen recording | `ScreenCaptureKit` stream |

---

## Rust Core Changes

### New Files

```
core/src/
└── desktop/
    ├── mod.rs              # pub mod
    ├── client.rs           # DesktopBridgeClient (UDS connection + request/response)
    ├── types.rs            # DesktopRequest, DesktopResponse, ScreenRegion, etc.
    └── error.rs            # DesktopError variants

core/src/builtin_tools/
└── desktop.rs              # DesktopTool: AlephTool impl, dispatches to DesktopBridgeClient
```

### DesktopTool Agent Schema

The agent calls `DesktopTool` with a flat JSON schema:

```json
{
  "action": "screenshot",
  "region": { "x": 0, "y": 0, "width": 1920, "height": 1080 }
}
```

```json
{
  "action": "click",
  "x": 500,
  "y": 300,
  "button": "left"
}
```

```json
{
  "action": "type_text",
  "text": "Hello, world!"
}
```

```json
{
  "action": "ocr"
}
```

```json
{
  "action": "canvas_show",
  "html": "<h1>Hello</h1>",
  "position": { "x": 100, "y": 100, "width": 800, "height": 600 }
}
```

### Graceful Degradation

```rust
impl DesktopBridgeClient {
    pub async fn connect() -> Result<Self, DesktopError> {
        let path = dirs::home_dir()
            .unwrap()
            .join(".aleph/desktop.sock");

        if !path.exists() {
            return Err(DesktopError::AppNotRunning);
        }
        // connect...
    }
}

// DesktopTool returns user-friendly message when App not available
impl AlephTool for DesktopTool {
    async fn execute(&self, params: Value) -> ToolResult {
        match DesktopBridgeClient::connect().await {
            Err(DesktopError::AppNotRunning) => {
                ToolResult::text("Desktop capabilities require the Aleph macOS App. \
                    Please open Aleph.app to use screen, keyboard, and UI automation features.")
            }
            Ok(client) => client.send(params).await,
        }
    }
}
```

---

## macOS App Changes

### New Files

```
apps/macos/Sources/AlephCore/
└── DesktopBridge/
    ├── DesktopBridgeServer.swift   # UDS server, JSON-RPC dispatcher
    ├── Perception.swift            # Screenshot, OCR, AX tree
    ├── Action.swift                # Mouse, keyboard, app launch, windows
    ├── Canvas.swift                # WKWebView overlay, A2UI protocol
    └── BridgeTypes.swift           # Shared types (ScreenRegion, WindowInfo, etc.)
```

### TCC Permission Entries (Info.plist)

```xml
<key>NSScreenCaptureUsageDescription</key>
<string>Aleph needs screen recording to capture what's on your screen.</string>

<key>NSAccessibilityUsageDescription</key>
<string>Aleph needs accessibility access to interact with other apps on your behalf.</string>

<key>NSMicrophoneUsageDescription</key>
<string>Aleph needs microphone access for voice input.</string>

<key>NSAppleEventsUsageDescription</key>
<string>Aleph needs automation access to control other applications.</string>
```

---

## What Is NOT Restored

| Deleted Component | Reason Not Restored |
|------------------|---------------------|
| `core/src/perception/` Rust code | Moved to Swift; native APIs are cleaner |
| `core/src/browser/` chromiumoxide | Playwright MCP already handles browser automation |
| `core/src/vision/` OCR | Vision.framework in Swift is simpler |
| `gateway/handlers/browser.rs` | MCP handles browser; no Gateway handler needed |
| `executor/router.rs` | Keep single execution point; no client routing |
| `executor/routed_executor.rs` | Same reason |
| `gateway/reverse_rpc.rs` | UDS is one-directional request/response; simpler |

The archived code at `archive/2026-02-23-desktop-control-code/` serves as reference for:
- PAL trait design patterns
- A2UI protocol message format
- State Bus JSON Patch RFC 6902 format

---

## Implementation Roadmap

### Phase 1: UDS Infrastructure (1-2 days)

**Goal:** Establish bidirectional JSON-RPC 2.0 communication over UDS.

Rust Core:
- `core/src/desktop/client.rs`: async UDS connect, read/write framing, request/response matching
- `core/src/desktop/types.rs`: `DesktopRequest`, `DesktopResponse` enums
- `core/src/builtin_tools/desktop.rs`: `DesktopTool` skeleton (always returns "App not running" for now)

Swift App:
- `DesktopBridgeServer.swift`: listen on `~/.aleph/desktop.sock`, parse JSON-RPC 2.0, stub responses

Validation: Rust sends `{"method":"desktop.ping"}` → Swift returns `{"result":"pong"}`

### Phase 2: Perception Capabilities (2-3 days)

**Goal:** Agent can see the screen.

- `screenshot`: ScreenCaptureKit (macOS 13+) with CGWindowListCreateImage fallback
- `ocr`: VNRecognizeTextRequest with Chinese + English language support
- `ax_tree`: NSAccessibility tree traversal, structured JSON output
- Register all three as `DesktopTool` sub-commands

### Phase 3: Action Capabilities (2-3 days)

**Goal:** Agent can control the computer.

- `click`, `right_click`, `double_click`: CGEvent mouse events
- `type_text`: CGEvent keyboard events with Unicode support
- `key_combo`: modifier + key sequences (cmd+c, cmd+v, etc.)
- `launch_app`, `window_list`, `focus_window`: NSWorkspace + CGWindowList

Optional: command approval workflow (configurable in `~/.aleph/config.toml`)

### Phase 4: Canvas Visualization (1-2 days)

**Goal:** Agent can render custom UI panels.

- `canvas_show`: Create NSPanel with WKWebView, load HTML content
- `canvas_hide`: Close or hide the panel
- `canvas_update`: Apply A2UI v0.8 surface patches via JavaScript bridge

### Phase 5: Browser (Already Done)

Playwright MCP Server is already configured. No additional work required.

---

## Testing Strategy

| Phase | Test Approach |
|-------|---------------|
| Phase 1 | Unit test: mock Swift server, verify Rust client request/response framing |
| Phase 2 | Integration test: take screenshot, verify PNG data returned |
| Phase 3 | Manual test: agent types text into TextEdit, verifies content |
| Phase 4 | Manual test: agent renders HTML panel, confirms visible on screen |

---

## Configuration

```toml
# ~/.aleph/config.toml
[desktop]
# Socket path override (default: ~/.aleph/desktop.sock)
socket_path = "~/.aleph/desktop.sock"

# Action approval mode: "off" | "on-sensitive" | "always"
# "on-sensitive" = prompt for mouse clicks and key combos only
action_approval = "on-sensitive"

# Screenshot quality: 0.0-1.0 (JPEG) or "png"
screenshot_format = "png"
```

---

## OpenClaw Reference

This design is validated by OpenClaw's architecture:
- `control.sock` UDS connection between Daemon and macOS App
- Browser automation in Core via Playwright
- Canvas rendering via WKWebView in native App
- Permission TCC centralized in macOS App

Key difference: Aleph uses JSON-RPC 2.0 (matching Gateway protocol) vs OpenClaw's custom protocol.
