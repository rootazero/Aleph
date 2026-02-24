# Desktop Bridge Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Give Aleph "hands and feet" — the ability to see and control the macOS desktop via a
Swift Bridge in the macOS App, communicating with Rust Core over Unix Domain Socket.

**Architecture:** Three layers: Rust Core (Brain) defines `DesktopTool` and a UDS client;
macOS App (Limbs) runs a UDS server implementing native macOS APIs; Browser automation
continues via existing Playwright MCP Server.

**Tech Stack:** Rust (tokio UDS, serde_json, async_trait), Swift (ScreenCaptureKit,
Vision.framework, NSAccessibility, CGEvent, WKWebView), JSON-RPC 2.0 over UDS.

**Design doc:** `docs/plans/2026-02-24-desktop-bridge-design.md`

---

## Phase 1: UDS Infrastructure

### Task 1: Desktop types module (Rust)

**Files:**
- Create: `core/src/desktop/mod.rs`
- Create: `core/src/desktop/types.rs`
- Create: `core/src/desktop/error.rs`

**Step 1: Create the desktop module directory and mod.rs**

```rust
// core/src/desktop/mod.rs
pub mod client;
pub mod error;
pub mod types;

pub use client::DesktopBridgeClient;
pub use error::DesktopError;
pub use types::{DesktopRequest, DesktopResponse, ScreenRegion, MouseButton, CanvasPosition};
```

**Step 2: Create types.rs with request/response enums**

```rust
// core/src/desktop/types.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasPosition {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum DesktopRequest {
    // Perception
    #[serde(rename = "desktop.screenshot")]
    Screenshot { region: Option<ScreenRegion> },
    #[serde(rename = "desktop.ocr")]
    Ocr { image_base64: Option<String> },
    #[serde(rename = "desktop.ax_tree")]
    AxTree { app_bundle_id: Option<String> },

    // Action
    #[serde(rename = "desktop.click")]
    Click { x: f64, y: f64, button: MouseButton },
    #[serde(rename = "desktop.type_text")]
    TypeText { text: String },
    #[serde(rename = "desktop.key_combo")]
    KeyCombo { keys: Vec<String> },
    #[serde(rename = "desktop.launch_app")]
    LaunchApp { bundle_id: String },
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
    CanvasUpdate { patch: serde_json::Value },

    // Internal
    #[serde(rename = "desktop.ping")]
    Ping {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DesktopResponse {
    Success { result: serde_json::Value },
    Error { error: DesktopRpcError },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopRpcError {
    pub code: i32,
    pub message: String,
}
```

**Step 3: Create error.rs**

```rust
// core/src/desktop/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DesktopError {
    #[error("Aleph macOS App is not running. Open Aleph.app to use desktop capabilities.")]
    AppNotRunning,

    #[error("Desktop bridge connection failed: {0}")]
    ConnectionFailed(#[from] std::io::Error),

    #[error("Desktop bridge protocol error: {0}")]
    Protocol(String),

    #[error("Desktop operation failed: {0}")]
    Operation(String),
}
```

**Step 4: Add `desktop` module to `core/src/lib.rs`**

Find the existing `pub mod` declarations in `core/src/lib.rs` and add:
```rust
pub mod desktop;
```

**Step 5: Compile to verify no errors**

```bash
cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -30
```

Expected: compiles with no errors (possibly warnings about unused items — acceptable).

**Step 6: Commit**

```bash
git add core/src/desktop/ core/src/lib.rs
git commit -m "feat(desktop): add types, error, and module scaffold"
```

---

### Task 2: UDS client (Rust)

**Files:**
- Create: `core/src/desktop/client.rs`

**Step 1: Write the client**

```rust
// core/src/desktop/client.rs
//! Unix Domain Socket client for communicating with the macOS App Desktop Bridge.

use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::debug;
use uuid::Uuid;

use crate::error::Result;
use super::error::DesktopError;
use super::types::{DesktopRequest, DesktopResponse};

/// Client that connects to the macOS App's UDS Desktop Bridge server.
pub struct DesktopBridgeClient {
    socket_path: PathBuf,
}

impl DesktopBridgeClient {
    /// Create a new client using the default socket path (~/.aleph/desktop.sock).
    pub fn new() -> Self {
        let socket_path = dirs::home_dir()
            .expect("home dir")
            .join(".aleph/desktop.sock");
        Self { socket_path }
    }

    /// Check if the macOS App is available (socket exists and accepts connections).
    pub fn is_available(&self) -> bool {
        self.socket_path.exists()
    }

    /// Send a request and receive a response.
    pub async fn send(&self, request: DesktopRequest) -> std::result::Result<serde_json::Value, DesktopError> {
        if !self.socket_path.exists() {
            return Err(DesktopError::AppNotRunning);
        }

        let stream = UnixStream::connect(&self.socket_path).await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::ConnectionRefused {
                    DesktopError::AppNotRunning
                } else {
                    DesktopError::ConnectionFailed(e)
                }
            })?;

        let id = Uuid::new_v4().to_string();

        // Build JSON-RPC 2.0 envelope
        let (method, params) = request_to_jsonrpc(&request);
        let rpc_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut wire = serde_json::to_string(&rpc_request).unwrap();
        wire.push('\n');

        debug!("Desktop bridge → {}", wire.trim());

        let (reader, mut writer) = stream.into_split();
        writer.write_all(wire.as_bytes()).await
            .map_err(DesktopError::ConnectionFailed)?;

        let mut lines = BufReader::new(reader).lines();
        let line = lines.next_line().await
            .map_err(DesktopError::ConnectionFailed)?
            .ok_or_else(|| DesktopError::Protocol("Connection closed without response".into()))?;

        debug!("Desktop bridge ← {}", line.trim());

        let rpc_response: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| DesktopError::Protocol(e.to_string()))?;

        if let Some(error) = rpc_response.get("error") {
            let msg = error.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(DesktopError::Operation(msg.to_string()));
        }

        Ok(rpc_response["result"].clone())
    }
}

fn request_to_jsonrpc(request: &DesktopRequest) -> (&'static str, serde_json::Value) {
    match request {
        DesktopRequest::Ping {} => ("desktop.ping", serde_json::json!({})),
        DesktopRequest::Screenshot { region } =>
            ("desktop.screenshot", serde_json::json!({ "region": region })),
        DesktopRequest::Ocr { image_base64 } =>
            ("desktop.ocr", serde_json::json!({ "image_base64": image_base64 })),
        DesktopRequest::AxTree { app_bundle_id } =>
            ("desktop.ax_tree", serde_json::json!({ "app_bundle_id": app_bundle_id })),
        DesktopRequest::Click { x, y, button } =>
            ("desktop.click", serde_json::json!({ "x": x, "y": y, "button": button })),
        DesktopRequest::TypeText { text } =>
            ("desktop.type_text", serde_json::json!({ "text": text })),
        DesktopRequest::KeyCombo { keys } =>
            ("desktop.key_combo", serde_json::json!({ "keys": keys })),
        DesktopRequest::LaunchApp { bundle_id } =>
            ("desktop.launch_app", serde_json::json!({ "bundle_id": bundle_id })),
        DesktopRequest::WindowList {} =>
            ("desktop.window_list", serde_json::json!({})),
        DesktopRequest::FocusWindow { window_id } =>
            ("desktop.focus_window", serde_json::json!({ "window_id": window_id })),
        DesktopRequest::CanvasShow { html, position } =>
            ("desktop.canvas_show", serde_json::json!({ "html": html, "position": position })),
        DesktopRequest::CanvasHide {} =>
            ("desktop.canvas_hide", serde_json::json!({})),
        DesktopRequest::CanvasUpdate { patch } =>
            ("desktop.canvas_update", serde_json::json!({ "patch": patch })),
    }
}
```

**Step 2: Add `uuid` dependency if not present**

Check `core/Cargo.toml` for uuid. If missing:
```toml
uuid = { version = "1", features = ["v4"] }
```

Also check for `dirs` crate. If missing:
```toml
dirs = "5"
```

**Step 3: Compile**

```bash
cargo check -p alephcore 2>&1 | head -30
```

Expected: no errors.

**Step 4: Commit**

```bash
git add core/src/desktop/client.rs core/Cargo.toml
git commit -m "feat(desktop): add UDS client with JSON-RPC 2.0"
```

---

### Task 3: DesktopTool (Rust builtin tool)

**Files:**
- Create: `core/src/builtin_tools/desktop.rs`
- Modify: `core/src/builtin_tools/mod.rs`
- Modify: `core/src/tools/builtin.rs`

**Step 1: Create the tool**

```rust
// core/src/builtin_tools/desktop.rs
//! Desktop Bridge tool — controls macOS desktop via Swift Bridge (UDS).
//!
//! When the Aleph macOS App is running, this tool can:
//! - Take screenshots and perform OCR
//! - Read the accessibility tree of any app
//! - Simulate mouse clicks and keyboard input
//! - Launch apps and manage windows
//! - Render HTML canvas overlays
//!
//! When the App is not running, all operations return a friendly message.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::desktop::{DesktopBridgeClient, DesktopRequest};
use crate::desktop::types::{CanvasPosition, MouseButton, ScreenRegion};
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for the desktop tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct DesktopArgs {
    /// The desktop operation to perform.
    ///
    /// Perception: "screenshot", "ocr", "ax_tree"
    /// Action:     "click", "type_text", "key_combo", "launch_app", "window_list", "focus_window"
    /// Canvas:     "canvas_show", "canvas_hide", "canvas_update"
    pub action: String,

    // Perception params
    /// Screen region for screenshot (optional). Example: {"x":0,"y":0,"width":1920,"height":1080}
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<ScreenRegion>,

    /// Base64-encoded image for OCR (optional). If absent, captures current screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,

    /// App bundle ID for ax_tree (optional). Example: "com.apple.Safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_bundle_id: Option<String>,

    // Action params
    /// X coordinate for click (pixels)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,

    /// Y coordinate for click (pixels)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,

    /// Mouse button for click: "left", "right", "middle"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<MouseButton>,

    /// Text to type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// Keys to press simultaneously. Example: ["cmd", "c"]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keys: Option<Vec<String>>,

    /// App bundle ID to launch. Example: "com.apple.Safari"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,

    /// Window ID to focus (from window_list results)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,

    // Canvas params
    /// HTML content to render in the canvas overlay
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,

    /// Canvas position and size. Example: {"x":100,"y":100,"width":800,"height":600}
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<CanvasPosition>,

    /// A2UI patch for canvas_update (RFC 6902 JSON Patch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<Value>,
}

/// Output from desktop operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Desktop Bridge tool — sees and controls the macOS desktop.
#[derive(Clone)]
pub struct DesktopTool {
    client: DesktopBridgeClient,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
        }
    }
}

impl Default for DesktopTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for DesktopTool {
    const NAME: &'static str = "desktop";
    const DESCRIPTION: &'static str = r#"Control the macOS desktop: take screenshots, read screen text, interact with UI elements, simulate keyboard/mouse, launch apps, and render custom UI panels.

Requires the Aleph macOS App to be running.

Actions:
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen (or provided image_base64)
- ax_tree: Get accessibility tree of an app (optional app_bundle_id)
- click: Click at {x,y} with optional button (left/right/middle)
- type_text: Type text using keyboard
- key_combo: Press key combination, e.g. keys=["cmd","c"] for Copy
- launch_app: Launch app by bundle_id, e.g. "com.apple.Safari"
- window_list: List all open windows with IDs and titles
- focus_window: Bring window_id to front
- canvas_show: Render HTML overlay panel at position {x,y,width,height}
- canvas_hide: Hide the canvas overlay
- canvas_update: Apply A2UI patch to canvas content

Examples:
{"action":"screenshot"}
{"action":"ocr"}
{"action":"click","x":500,"y":300,"button":"left"}
{"action":"type_text","text":"Hello, world!"}
{"action":"key_combo","keys":["cmd","c"]}
{"action":"launch_app","bundle_id":"com.apple.Safari"}
{"action":"window_list"}
{"action":"canvas_show","html":"<h1>Hello</h1>","position":{"x":100,"y":100,"width":800,"height":600}}"#;

    type Args = DesktopArgs;
    type Output = DesktopOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if !self.client.is_available() {
            return Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some("Desktop capabilities require the Aleph macOS App. \
                    Please open Aleph.app and ensure Desktop Bridge is enabled.".to_string()),
            });
        }

        let request = build_request(&args)?;
        match self.client.send(request).await {
            Ok(result) => Ok(DesktopOutput {
                success: true,
                data: Some(result),
                message: None,
            }),
            Err(e) => Ok(DesktopOutput {
                success: false,
                data: None,
                message: Some(e.to_string()),
            }),
        }
    }
}

fn build_request(args: &DesktopArgs) -> Result<DesktopRequest> {
    let req = match args.action.as_str() {
        "screenshot" => DesktopRequest::Screenshot {
            region: args.region.clone(),
        },
        "ocr" => DesktopRequest::Ocr {
            image_base64: args.image_base64.clone(),
        },
        "ax_tree" => DesktopRequest::AxTree {
            app_bundle_id: args.app_bundle_id.clone(),
        },
        "click" => DesktopRequest::Click {
            x: args.x.unwrap_or(0.0),
            y: args.y.unwrap_or(0.0),
            button: args.button.clone().unwrap_or(MouseButton::Left),
        },
        "type_text" => DesktopRequest::TypeText {
            text: args.text.clone().unwrap_or_default(),
        },
        "key_combo" => DesktopRequest::KeyCombo {
            keys: args.keys.clone().unwrap_or_default(),
        },
        "launch_app" => DesktopRequest::LaunchApp {
            bundle_id: args.bundle_id.clone().unwrap_or_default(),
        },
        "window_list" => DesktopRequest::WindowList {},
        "focus_window" => DesktopRequest::FocusWindow {
            window_id: args.window_id.unwrap_or(0),
        },
        "canvas_show" => DesktopRequest::CanvasShow {
            html: args.html.clone().unwrap_or_default(),
            position: args.position.clone().unwrap_or(CanvasPosition {
                x: 100.0, y: 100.0, width: 800.0, height: 600.0,
            }),
        },
        "canvas_hide" => DesktopRequest::CanvasHide {},
        "canvas_update" => DesktopRequest::CanvasUpdate {
            patch: args.patch.clone().unwrap_or(serde_json::json!([])),
        },
        other => {
            return Err(crate::error::AlephError::invalid_input(
                format!("Unknown desktop action: '{}'. Valid actions: screenshot, ocr, ax_tree, click, type_text, key_combo, launch_app, window_list, focus_window, canvas_show, canvas_hide, canvas_update", other)
            ));
        }
    };
    Ok(req)
}
```

**Step 2: Register in `builtin_tools/mod.rs`**

Add to the `pub mod` list:
```rust
pub mod desktop;
```

Add to the `pub use` list:
```rust
pub use desktop::{DesktopArgs, DesktopOutput, DesktopTool};
```

**Step 3: Add `with_desktop()` to `tools/builtin.rs`**

```rust
/// Register the desktop bridge tool (requires macOS App running).
pub fn with_desktop(self) -> Self {
    self.tool(DesktopTool::new())
}
```

**Step 4: Check how `AlephError::invalid_input` works in this codebase**

```bash
grep -r "invalid_input\|AlephError" /Volumes/TBU4/Workspace/Aleph/core/src/error.rs | head -20
```

Adjust the error type call to match the actual error API.

**Step 5: Compile**

```bash
cargo check -p alephcore 2>&1 | head -40
```

Expected: no errors.

**Step 6: Commit**

```bash
git add core/src/builtin_tools/desktop.rs core/src/builtin_tools/mod.rs core/src/tools/builtin.rs
git commit -m "feat(desktop): add DesktopTool builtin with graceful degradation"
```

---

### Task 4: Wire DesktopTool into the executor's tool registry

**Files:**
- Explore: `core/src/executor/` to find where builtin tools are registered
- Modify: the registry file (likely `core/src/executor/builtin_registry.rs`)

**Step 1: Find the tool registry**

```bash
grep -r "BashExecTool\|with_bash\|create_tool_boxed" /Volumes/TBU4/Workspace/Aleph/core/src/executor/ | head -20
```

**Step 2: Add DesktopTool to the registry**

In whichever file registers built-in tools, add `DesktopTool` following the same pattern as `BashExecTool`. Look for a `match` or `HashMap` that maps tool names to factory functions.

**Step 3: Compile and run tests**

```bash
cargo test -p alephcore -- desktop 2>&1
```

Expected: tests compile (no desktop-specific tests yet, but everything else passes).

**Step 4: Commit**

```bash
git add core/src/executor/
git commit -m "feat(desktop): register DesktopTool in executor registry"
```

---

### Task 5: Swift UDS server skeleton

**Files:**
- Create: `apps/macos/Aleph/Sources/DesktopBridge/DesktopBridgeServer.swift`
- Create: `apps/macos/Aleph/Sources/DesktopBridge/BridgeTypes.swift`
- Modify: `apps/macos/project.yml` (add new source group if needed)

**Step 1: Create BridgeTypes.swift**

```swift
// apps/macos/Aleph/Sources/DesktopBridge/BridgeTypes.swift
import Foundation

struct ScreenRegion: Codable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

struct CanvasPosition: Codable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

enum MouseButtonType: String, Codable {
    case left, right, middle
}

struct JSONRPCRequest: Codable {
    let jsonrpc: String
    let id: String
    let method: String
    let params: AnyCodable
}

struct JSONRPCResponse: Codable {
    let jsonrpc: String
    let id: String
    var result: AnyCodable?
    var error: JSONRPCError?
}

struct JSONRPCError: Codable {
    let code: Int
    let message: String
}

/// Helper to hold arbitrary JSON values
struct AnyCodable: Codable {
    let value: Any

    init(_ value: Any) { self.value = value }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let dict = try? container.decode([String: AnyCodable].self) {
            value = dict.mapValues { $0.value }
        } else if let array = try? container.decode([AnyCodable].self) {
            value = array.map { $0.value }
        } else if let string = try? container.decode(String.self) {
            value = string
        } else if let double = try? container.decode(Double.self) {
            value = double
        } else if let bool = try? container.decode(Bool.self) {
            value = bool
        } else {
            value = NSNull()
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch value {
        case let dict as [String: Any]:
            try container.encode(dict.mapValues { AnyCodable($0) })
        case let array as [Any]:
            try container.encode(array.map { AnyCodable($0) })
        case let string as String:
            try container.encode(string)
        case let double as Double:
            try container.encode(double)
        case let int as Int:
            try container.encode(int)
        case let bool as Bool:
            try container.encode(bool)
        default:
            try container.encodeNil()
        }
    }
}
```

**Step 2: Create DesktopBridgeServer.swift**

```swift
// apps/macos/Aleph/Sources/DesktopBridge/DesktopBridgeServer.swift
import Foundation
import Network

/// Listens on ~/.aleph/desktop.sock and dispatches JSON-RPC 2.0 requests.
@MainActor
class DesktopBridgeServer {
    static let shared = DesktopBridgeServer()
    private var listener: NSFileHandle?
    private var serverSocket: Int32 = -1

    private var socketPath: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return "\(home)/.aleph/desktop.sock"
    }

    func start() {
        // Ensure ~/.aleph directory exists
        let dir = URL(fileURLWithPath: socketPath).deletingLastPathComponent().path
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)

        // Remove stale socket
        try? FileManager.default.removeItem(atPath: socketPath)

        // Create UNIX domain socket
        serverSocket = socket(AF_UNIX, SOCK_STREAM, 0)
        guard serverSocket >= 0 else { return }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        socketPath.withCString { ptr in
            withUnsafeMutablePointer(to: &addr.sun_path.0) { dst in
                _ = strcpy(dst, ptr)
            }
        }

        let bindResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockAddr in
                bind(serverSocket, sockAddr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }

        guard bindResult == 0 else { return }
        listen(serverSocket, 5)

        // Accept connections in background
        Task.detached { [weak self] in
            await self?.acceptLoop()
        }

        print("[DesktopBridge] Listening on \(socketPath)")
    }

    func stop() {
        if serverSocket >= 0 {
            Darwin.close(serverSocket)
            serverSocket = -1
        }
        try? FileManager.default.removeItem(atPath: socketPath)
    }

    private func acceptLoop() async {
        while serverSocket >= 0 {
            let clientFd = Darwin.accept(serverSocket, nil, nil)
            guard clientFd >= 0 else { continue }
            Task.detached { [weak self] in
                await self?.handleConnection(fd: clientFd)
            }
        }
    }

    private func handleConnection(fd: Int32) async {
        defer { Darwin.close(fd) }

        // Read newline-delimited JSON-RPC messages
        var buffer = Data()
        let chunk = 4096
        while true {
            var tmp = [UInt8](repeating: 0, count: chunk)
            let n = Darwin.read(fd, &tmp, chunk)
            if n <= 0 { break }
            buffer.append(contentsOf: tmp.prefix(n))

            // Process complete lines
            while let range = buffer.range(of: Data([0x0A])) { // newline
                let lineData = buffer.subdata(in: buffer.startIndex..<range.lowerBound)
                buffer.removeSubrange(buffer.startIndex...range.lowerBound)

                if let response = await processLine(lineData) {
                    var responseData = response
                    responseData.append(0x0A) // newline terminator
                    _ = responseData.withUnsafeBytes { ptr in
                        Darwin.write(fd, ptr.baseAddress, responseData.count)
                    }
                }
            }
        }
    }

    private func processLine(_ data: Data) async -> Data? {
        guard let request = try? JSONDecoder().decode(JSONRPCRequest.self, from: data) else {
            return nil
        }

        let result = await dispatch(method: request.method, params: request.params.value)

        var response: [String: Any] = [
            "jsonrpc": "2.0",
            "id": request.id,
        ]
        switch result {
        case .success(let value):
            response["result"] = value
        case .failure(let error):
            response["error"] = ["code": -32000, "message": error.localizedDescription]
        }

        return try? JSONSerialization.data(withJSONObject: response)
    }

    private func dispatch(method: String, params: Any) async -> Result<Any, Error> {
        let p = params as? [String: Any] ?? [:]
        switch method {
        case "desktop.ping":
            return .success("pong")
        case "desktop.screenshot":
            let region = parseRegion(from: p["region"])
            return await Perception.shared.screenshot(region: region)
        case "desktop.ocr":
            let imageB64 = p["image_base64"] as? String
            return await Perception.shared.ocr(imageBase64: imageB64)
        case "desktop.ax_tree":
            let bundleId = p["app_bundle_id"] as? String
            return await Perception.shared.axTree(appBundleId: bundleId)
        case "desktop.click":
            let x = p["x"] as? Double ?? 0
            let y = p["y"] as? Double ?? 0
            let btn = p["button"] as? String ?? "left"
            return await Action.shared.click(x: x, y: y, button: btn)
        case "desktop.type_text":
            let text = p["text"] as? String ?? ""
            return await Action.shared.typeText(text)
        case "desktop.key_combo":
            let keys = p["keys"] as? [String] ?? []
            return await Action.shared.keyCombo(keys: keys)
        case "desktop.launch_app":
            let bundleId = p["bundle_id"] as? String ?? ""
            return await Action.shared.launchApp(bundleId: bundleId)
        case "desktop.window_list":
            return await Action.shared.windowList()
        case "desktop.focus_window":
            let windowId = p["window_id"] as? UInt32 ?? 0
            return await Action.shared.focusWindow(id: windowId)
        case "desktop.canvas_show":
            let html = p["html"] as? String ?? ""
            let pos = parsePosition(from: p["position"])
            return await Canvas.shared.show(html: html, position: pos)
        case "desktop.canvas_hide":
            return await Canvas.shared.hide()
        case "desktop.canvas_update":
            let patch = p["patch"] ?? []
            return await Canvas.shared.update(patch: patch)
        default:
            let err = NSError(domain: "DesktopBridge", code: -32601,
                              userInfo: [NSLocalizedDescriptionKey: "Method not found: \(method)"])
            return .failure(err)
        }
    }

    private func parseRegion(from value: Any?) -> ScreenRegion? {
        guard let dict = value as? [String: Any],
              let x = dict["x"] as? Double,
              let y = dict["y"] as? Double,
              let w = dict["width"] as? Double,
              let h = dict["height"] as? Double
        else { return nil }
        return ScreenRegion(x: x, y: y, width: w, height: h)
    }

    private func parsePosition(from value: Any?) -> CanvasPosition {
        guard let dict = value as? [String: Any],
              let x = dict["x"] as? Double,
              let y = dict["y"] as? Double,
              let w = dict["width"] as? Double,
              let h = dict["height"] as? Double
        else { return CanvasPosition(x: 100, y: 100, width: 800, height: 600) }
        return CanvasPosition(x: x, y: y, width: w, height: h)
    }
}
```

**Step 3: Start the server from AppDelegate**

In `apps/macos/Aleph/Sources/AppDelegate.swift`, add to `applicationDidFinishLaunching`:
```swift
Task { await DesktopBridgeServer.shared.start() }
```

And in `applicationWillTerminate`:
```swift
await DesktopBridgeServer.shared.stop()
```

**Step 4: Generate Xcode project**

```bash
cd /Volumes/TBU4/Workspace/Aleph/apps/macos && xcodegen generate
```

**Step 5: Build the macOS App to verify it compiles**

```bash
xcodebuild -project /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph.xcodeproj \
  -scheme Aleph -configuration Debug -destination 'platform=macOS' \
  build 2>&1 | tail -20
```

Expected: `BUILD SUCCEEDED`

**Step 6: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/ apps/macos/Aleph/Sources/AppDelegate.swift
git commit -m "feat(desktop): add Swift UDS server skeleton with JSON-RPC dispatcher"
```

---

### Task 6: End-to-end ping test

**Goal:** Verify Rust can send `desktop.ping` and Swift responds `pong`.

**Step 1: Add a test binary or integration test**

Create a simple test in `core/src/desktop/client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires macOS App running"]
    async fn test_ping() {
        let client = DesktopBridgeClient::new();
        if !client.is_available() {
            eprintln!("Skipping: macOS App not running");
            return;
        }
        let result = client.send(DesktopRequest::Ping {}).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!("pong"));
    }
}
```

**Step 2: Run the App, then run the test**

```bash
# In one terminal: open the app
open /path/to/Aleph.app

# In another terminal:
cargo test -p alephcore -- desktop::client::tests::test_ping --ignored --nocapture
```

Expected: `pong` returned.

**Step 3: Commit**

```bash
git add core/src/desktop/client.rs
git commit -m "test(desktop): add integration test for ping/pong"
```

---

## Phase 2: Perception Capabilities

### Task 7: Screenshot (Swift)

**Files:**
- Create: `apps/macos/Aleph/Sources/DesktopBridge/Perception.swift`

**Step 1: Create Perception.swift**

```swift
// apps/macos/Aleph/Sources/DesktopBridge/Perception.swift
import Foundation
import ScreenCaptureKit
import CoreGraphics
import Vision

/// Provides perception capabilities: screenshot, OCR, accessibility tree.
@MainActor
class Perception {
    static let shared = Perception()

    // MARK: - Screenshot

    func screenshot(region: ScreenRegion?) async -> Result<Any, Error> {
        // ScreenCaptureKit (macOS 13+)
        if #available(macOS 13.0, *) {
            return await screenshotSCK(region: region)
        } else {
            return screenshotCGWindow(region: region)
        }
    }

    @available(macOS 13.0, *)
    private func screenshotSCK(region: ScreenRegion?) async -> Result<Any, Error> {
        do {
            let displays = try await SCShareableContent.current.displays
            guard let display = displays.first else {
                throw NSError(domain: "Perception", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "No display found"])
            }

            let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
            let config = SCStreamConfiguration()
            if let r = region {
                config.sourceRect = CGRect(x: r.x, y: r.y, width: r.width, height: r.height)
            }
            config.pixelFormat = kCVPixelFormatType_32BGRA

            let image = try await SCScreenshotManager.captureImage(contentFilter: filter,
                                                                    configuration: config)
            guard let data = imageToBase64PNG(image) else {
                throw NSError(domain: "Perception", code: 2,
                              userInfo: [NSLocalizedDescriptionKey: "Failed to encode screenshot"])
            }

            return .success([
                "image_base64": data,
                "width": Int(image.width),
                "height": Int(image.height),
                "format": "png",
            ])
        } catch {
            return .failure(error)
        }
    }

    private func screenshotCGWindow(region: ScreenRegion?) -> Result<Any, Error> {
        let bounds: CGRect
        if let r = region {
            bounds = CGRect(x: r.x, y: r.y, width: r.width, height: r.height)
        } else {
            bounds = CGRect.infinite
        }

        guard let image = CGWindowListCreateImage(bounds, .optionAll, kCGNullWindowID, .bestResolution) else {
            let err = NSError(domain: "Perception", code: 3,
                              userInfo: [NSLocalizedDescriptionKey: "CGWindowListCreateImage failed"])
            return .failure(err)
        }

        guard let data = imageToBase64PNG(image) else {
            let err = NSError(domain: "Perception", code: 4,
                              userInfo: [NSLocalizedDescriptionKey: "Failed to encode image"])
            return .failure(err)
        }

        return .success([
            "image_base64": data,
            "width": image.width,
            "height": image.height,
            "format": "png",
        ])
    }

    private func imageToBase64PNG(_ image: CGImage) -> String? {
        let nsImage = NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
        guard let tiff = nsImage.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let pngData = bitmap.representation(using: .png, properties: [:])
        else { return nil }
        return pngData.base64EncodedString()
    }

    // MARK: - OCR

    func ocr(imageBase64: String?) async -> Result<Any, Error> {
        let imageData: Data?
        if let b64 = imageBase64 {
            imageData = Data(base64Encoded: b64)
        } else {
            // Capture current screen first
            let result = await screenshot(region: nil)
            switch result {
            case .success(let dict):
                let d = dict as? [String: Any]
                let b64 = d?["image_base64"] as? String
                imageData = b64.flatMap { Data(base64Encoded: $0) }
            case .failure(let e):
                return .failure(e)
            }
        }

        guard let data = imageData, let nsImage = NSImage(data: data),
              let cgImage = nsImage.cgImage(forProposedRect: nil, context: nil, hints: nil)
        else {
            let err = NSError(domain: "Perception", code: 5,
                              userInfo: [NSLocalizedDescriptionKey: "Invalid image data"])
            return .failure(err)
        }

        return await recognizeText(in: cgImage)
    }

    private func recognizeText(in image: CGImage) async -> Result<Any, Error> {
        return await withCheckedContinuation { continuation in
            let request = VNRecognizeTextRequest { request, error in
                if let error = error {
                    continuation.resume(returning: .failure(error))
                    return
                }
                let observations = request.results as? [VNRecognizedTextObservation] ?? []
                let lines = observations.compactMap { obs -> [String: Any]? in
                    guard let top = obs.topCandidates(1).first else { return nil }
                    return [
                        "text": top.string,
                        "confidence": top.confidence,
                        "bounds": [
                            "x": obs.boundingBox.origin.x,
                            "y": obs.boundingBox.origin.y,
                            "width": obs.boundingBox.width,
                            "height": obs.boundingBox.height,
                        ],
                    ]
                }
                let fullText = lines.compactMap { $0["text"] as? String }.joined(separator: "\n")
                continuation.resume(returning: .success([
                    "text": fullText,
                    "lines": lines,
                ]))
            }
            request.recognitionLevel = .accurate
            request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en-US"]
            request.usesLanguageCorrection = true

            let handler = VNImageRequestHandler(cgImage: image, options: [:])
            do {
                try handler.perform([request])
            } catch {
                continuation.resume(returning: .failure(error))
            }
        }
    }

    // MARK: - Accessibility Tree

    func axTree(appBundleId: String?) async -> Result<Any, Error> {
        let app: AXUIElement
        if let bundleId = appBundleId {
            guard let runningApp = NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).first else {
                let err = NSError(domain: "Perception", code: 6,
                                  userInfo: [NSLocalizedDescriptionKey: "App not running: \(bundleId)"])
                return .failure(err)
            }
            app = AXUIElementCreateApplication(runningApp.processIdentifier)
        } else {
            // Frontmost app
            guard let frontmost = NSWorkspace.shared.frontmostApplication else {
                let err = NSError(domain: "Perception", code: 7,
                                  userInfo: [NSLocalizedDescriptionKey: "No frontmost app"])
                return .failure(err)
            }
            app = AXUIElementCreateApplication(frontmost.processIdentifier)
        }

        let tree = axElementToDict(app, depth: 0, maxDepth: 5)
        return .success(tree)
    }

    private func axElementToDict(_ element: AXUIElement, depth: Int, maxDepth: Int) -> [String: Any] {
        guard depth < maxDepth else { return ["truncated": true] }

        var result: [String: Any] = [:]

        // Role
        var roleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &roleValue)
        result["role"] = roleValue as? String ?? "unknown"

        // Title / Label
        var titleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXTitleAttribute as CFString, &titleValue)
        if let title = titleValue as? String, !title.isEmpty {
            result["title"] = title
        }

        // Value
        var valueValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXValueAttribute as CFString, &valueValue)
        if let v = valueValue as? String, !v.isEmpty {
            result["value"] = v
        }

        // Position and size
        var positionValue: AnyObject?
        var sizeValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue)
        AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue)
        if let pos = positionValue, let sz = sizeValue {
            var point = CGPoint.zero
            var size = CGSize.zero
            AXValueGetValue(pos as! AXValue, .cgPoint, &point)
            AXValueGetValue(sz as! AXValue, .cgSize, &size)
            result["frame"] = ["x": point.x, "y": point.y, "width": size.width, "height": size.height]
        }

        // Children
        var childrenValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &childrenValue)
        if let children = childrenValue as? [AXUIElement] {
            result["children"] = children.map { axElementToDict($0, depth: depth + 1, maxDepth: maxDepth) }
        }

        return result
    }
}
```

**Step 2: Add Info.plist permissions**

In `apps/macos/project.yml`, ensure these entries exist under the app's `info` key:
```yaml
NSScreenCaptureUsageDescription: "Aleph needs screen access to capture what's on your screen."
```

**Step 3: Build and test manually**

```bash
xcodebuild -project /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph.xcodeproj \
  -scheme Aleph -configuration Debug -destination 'platform=macOS' \
  build 2>&1 | tail -10
```

Then run the app, and test with Rust:
```bash
cargo test -p alephcore -- test_ping --ignored --nocapture
```

**Step 4: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/Perception.swift apps/macos/project.yml
git commit -m "feat(desktop): implement screenshot and OCR in Swift (Vision.framework)"
```

---

### Task 8: Accessibility tree permissions

**Files:**
- Modify: `apps/macos/project.yml`

**Step 1: Add Accessibility entitlement**

In `project.yml`, add to entitlements or Info.plist:
```yaml
NSAccessibilityUsageDescription: "Aleph needs accessibility to read UI elements in other apps."
```

Also in the entitlements file (if present):
```xml
<key>com.apple.security.automation.apple-events</key>
<true/>
```

**Step 2: Verify AX permission at runtime**

Add to `DesktopBridgeServer.start()`:
```swift
let trusted = AXIsProcessTrustedWithOptions(
    [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
)
if !trusted {
    print("[DesktopBridge] Warning: Accessibility permission not granted")
}
```

**Step 3: Commit**

```bash
git add apps/macos/project.yml apps/macos/Aleph/Sources/DesktopBridge/DesktopBridgeServer.swift
git commit -m "feat(desktop): add accessibility permission check and prompt"
```

---

## Phase 3: Action Capabilities

### Task 9: Mouse and keyboard simulation (Swift)

**Files:**
- Create: `apps/macos/Aleph/Sources/DesktopBridge/Action.swift`

**Step 1: Create Action.swift**

```swift
// apps/macos/Aleph/Sources/DesktopBridge/Action.swift
import Foundation
import AppKit
import CoreGraphics

/// Provides action capabilities: mouse, keyboard, app launch, windows.
@MainActor
class Action {
    static let shared = Action()

    // MARK: - Mouse

    func click(x: Double, y: Double, button: String) async -> Result<Any, Error> {
        let point = CGPoint(x: x, y: y)
        let (downType, upType, cgButton): (CGEventType, CGEventType, CGMouseButton)
        switch button {
        case "right":
            downType = .rightMouseDown; upType = .rightMouseUp; cgButton = .right
        case "middle":
            downType = .otherMouseDown; upType = .otherMouseUp; cgButton = .center
        default:
            downType = .leftMouseDown; upType = .leftMouseUp; cgButton = .left
        }

        guard let source = CGEventSource(stateID: .hidSystemState),
              let down = CGEvent(mouseEventSource: source, mouseType: downType,
                                 mouseCursorPosition: point, mouseButton: cgButton),
              let up = CGEvent(mouseEventSource: source, mouseType: upType,
                               mouseCursorPosition: point, mouseButton: cgButton)
        else {
            return .failure(NSError(domain: "Action", code: 1,
                                    userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse events"]))
        }

        down.post(tap: .cghidEventTap)
        try? await Task.sleep(nanoseconds: 50_000_000) // 50ms
        up.post(tap: .cghidEventTap)

        return .success(["clicked": true, "x": x, "y": y, "button": button])
    }

    // MARK: - Keyboard

    func typeText(_ text: String) async -> Result<Any, Error> {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(NSError(domain: "Action", code: 2,
                                    userInfo: [NSLocalizedDescriptionKey: "Failed to create event source"]))
        }

        for char in text.unicodeScalars {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true) else { continue }
            var scalar = char
            event.keyboardSetUnicodeString(stringLength: 1, unicodeString: &scalar.value)
            event.post(tap: .cghidEventTap)

            let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false)
            up?.post(tap: .cghidEventTap)

            try? await Task.sleep(nanoseconds: 10_000_000) // 10ms between chars
        }

        return .success(["typed": text.count])
    }

    func keyCombo(keys: [String]) async -> Result<Any, Error> {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(NSError(domain: "Action", code: 3,
                                    userInfo: [NSLocalizedDescriptionKey: "Failed to create event source"]))
        }

        var flags: CGEventFlags = []
        var mainKey: CGKeyCode = 0

        for key in keys {
            switch key.lowercased() {
            case "cmd", "command": flags.insert(.maskCommand)
            case "shift":          flags.insert(.maskShift)
            case "opt", "alt":    flags.insert(.maskAlternate)
            case "ctrl", "control": flags.insert(.maskControl)
            default:
                // Map common key names to virtual keycodes
                mainKey = keyNameToCode(key)
            }
        }

        guard let down = CGEvent(keyboardEventSource: source, virtualKey: mainKey, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: mainKey, keyDown: false)
        else {
            return .failure(NSError(domain: "Action", code: 4,
                                    userInfo: [NSLocalizedDescriptionKey: "Failed to create key events"]))
        }

        down.flags = flags
        up.flags = flags
        down.post(tap: .cghidEventTap)
        try? await Task.sleep(nanoseconds: 50_000_000)
        up.post(tap: .cghidEventTap)

        return .success(["keys": keys])
    }

    private func keyNameToCode(_ name: String) -> CGKeyCode {
        // Common key name → virtual keycode mappings (US keyboard)
        let map: [String: CGKeyCode] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
            "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16,
            "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
            "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30,
            "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
            "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45,
            "m": 46, ".": 47, "tab": 48, "space": 49, "`": 50, "delete": 51,
            "return": 36, "enter": 76, "escape": 53, "esc": 53,
            "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
            "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
            "left": 123, "right": 124, "down": 125, "up": 126,
            "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
        ]
        return map[name.lowercased()] ?? 0
    }

    // MARK: - App and Window Management

    func launchApp(bundleId: String) async -> Result<Any, Error> {
        let config = NSWorkspace.OpenConfiguration()
        do {
            let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleId)
            guard let appURL = url else {
                return .failure(NSError(domain: "Action", code: 5,
                                        userInfo: [NSLocalizedDescriptionKey: "App not found: \(bundleId)"]))
            }
            let app = try await NSWorkspace.shared.openApplication(at: appURL, configuration: config)
            return .success(["launched": bundleId, "pid": app.processIdentifier])
        } catch {
            return .failure(error)
        }
    }

    func windowList() async -> Result<Any, Error> {
        guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements],
                                                       kCGNullWindowID) as? [[String: Any]]
        else {
            return .failure(NSError(domain: "Action", code: 6,
                                    userInfo: [NSLocalizedDescriptionKey: "Failed to list windows"]))
        }

        let list = windows.compactMap { info -> [String: Any]? in
            guard let id = info[kCGWindowNumber as String] as? Int else { return nil }
            let name = info[kCGWindowName as String] as? String ?? ""
            let owner = info[kCGWindowOwnerName as String] as? String ?? ""
            let pid = info[kCGWindowOwnerPID as String] as? Int ?? 0
            let bounds = info[kCGWindowBounds as String] as? [String: Any] ?? [:]
            return ["id": id, "title": name, "owner": owner, "pid": pid, "bounds": bounds]
        }

        return .success(["windows": list])
    }

    func focusWindow(id: UInt32) async -> Result<Any, Error> {
        // Find the running app that owns this window
        guard let windows = CGWindowListCopyWindowInfo([.optionAll], CGWindowID(id)) as? [[String: Any]],
              let window = windows.first,
              let pid = window[kCGWindowOwnerPID as String] as? Int32
        else {
            return .failure(NSError(domain: "Action", code: 7,
                                    userInfo: [NSLocalizedDescriptionKey: "Window \(id) not found"]))
        }

        let app = NSRunningApplication(processIdentifier: pid)
        app?.activate(options: .activateIgnoringOtherApps)

        return .success(["focused": id])
    }
}
```

**Step 2: Build and verify**

```bash
xcodebuild -project /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph.xcodeproj \
  -scheme Aleph -configuration Debug -destination 'platform=macOS' \
  build 2>&1 | tail -10
```

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/Action.swift
git commit -m "feat(desktop): implement mouse/keyboard/window actions in Swift"
```

---

## Phase 4: Canvas Visualization

### Task 10: Canvas WKWebView overlay (Swift)

**Files:**
- Create: `apps/macos/Aleph/Sources/DesktopBridge/Canvas.swift`

**Step 1: Create Canvas.swift**

```swift
// apps/macos/Aleph/Sources/DesktopBridge/Canvas.swift
import Foundation
import AppKit
import WebKit

/// Provides canvas rendering: WKWebView overlay window with A2UI protocol support.
@MainActor
class Canvas: NSObject {
    static let shared = Canvas()

    private var panel: NSPanel?
    private var webView: WKWebView?

    func show(html: String, position: CanvasPosition) async -> Result<Any, Error> {
        if panel == nil {
            createPanel()
        }

        guard let panel = panel, let webView = webView else {
            return .failure(NSError(domain: "Canvas", code: 1,
                                    userInfo: [NSLocalizedDescriptionKey: "Failed to create canvas panel"]))
        }

        // Position and size
        panel.setFrame(NSRect(x: position.x, y: position.y,
                              width: position.width, height: position.height),
                       display: true)

        // Load HTML content
        webView.loadHTMLString(html, baseURL: nil)
        panel.orderFront(nil)

        return .success(["visible": true, "position": [
            "x": position.x, "y": position.y,
            "width": position.width, "height": position.height,
        ]])
    }

    func hide() async -> Result<Any, Error> {
        panel?.orderOut(nil)
        return .success(["visible": false])
    }

    func update(patch: Any) async -> Result<Any, Error> {
        guard let webView = webView else {
            return .failure(NSError(domain: "Canvas", code: 2,
                                    userInfo: [NSLocalizedDescriptionKey: "Canvas not shown"]))
        }

        // Apply A2UI patch via JavaScript
        guard let patchData = try? JSONSerialization.data(withJSONObject: patch),
              let patchJson = String(data: patchData, encoding: .utf8)
        else {
            return .failure(NSError(domain: "Canvas", code: 3,
                                    userInfo: [NSLocalizedDescriptionKey: "Invalid patch data"]))
        }

        let script = "if (window.alephApplyPatch) { window.alephApplyPatch(\(patchJson)); }"
        await webView.evaluateJavaScript(script)

        return .success(["patched": true])
    }

    private func createPanel() {
        let panel = NSPanel(
            contentRect: NSRect(x: 100, y: 100, width: 800, height: 600),
            styleMask: [.titled, .closable, .resizable, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.title = "Aleph Canvas"
        panel.level = .floating
        panel.isReleasedWhenClosed = false
        panel.backgroundColor = .clear
        panel.isOpaque = false

        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let webView = WKWebView(frame: panel.contentRect(forFrameRect: panel.frame),
                                configuration: config)
        webView.autoresizingMask = [.width, .height]
        webView.navigationDelegate = self

        panel.contentView = webView

        self.panel = panel
        self.webView = webView
    }
}

extension Canvas: WKNavigationDelegate {
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // Inject A2UI helper
        let script = """
        window.alephApplyPatch = function(patch) {
            // Simple A2UI v0.8 surface update handler
            if (Array.isArray(patch)) {
                patch.forEach(function(op) {
                    if (op.type === 'surfaceUpdate' && op.content) {
                        document.body.innerHTML = op.content;
                    }
                });
            }
        };
        """
        webView.evaluateJavaScript(script, completionHandler: nil)
    }
}
```

**Step 2: Build**

```bash
xcodebuild -project /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph.xcodeproj \
  -scheme Aleph -configuration Debug -destination 'platform=macOS' \
  build 2>&1 | tail -10
```

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/Canvas.swift
git commit -m "feat(desktop): implement WKWebView canvas overlay with A2UI support"
```

---

## Phase 5: Register with Agent and End-to-End Test

### Task 11: Enable DesktopTool in agent configuration

**Files:**
- Explore: how agents load their tool sets (look for config or initialization code)
- Modify: the agent setup to include `with_desktop()`

**Step 1: Find agent tool setup**

```bash
grep -r "with_bash\|with_search\|AlephToolServer" /Volumes/TBU4/Workspace/Aleph/core/src/ --include="*.rs" -l
```

**Step 2: Add desktop tool to agent setup**

In whichever file constructs the tool server for the main agent, add:
```rust
.with_desktop()
```

**Step 3: End-to-end test via CLI**

Run the server:
```bash
cargo run --bin aleph-server 2>&1 &
```

Send a message that uses the desktop tool:
```bash
# Use the Aleph CLI or WebSocket to ask:
# "Take a screenshot of my screen and describe what you see."
```

Expected: Agent calls DesktopTool with `{"action":"screenshot"}`, gets base64 PNG, describes it.

**Step 4: Final commit**

```bash
git add -u
git commit -m "feat(desktop): enable DesktopTool in agent — Phase 1-4 complete"
```

---

## Summary

| Task | Deliverable | Language |
|------|-------------|----------|
| 1 | Desktop types + error module | Rust |
| 2 | UDS client with JSON-RPC 2.0 | Rust |
| 3 | DesktopTool builtin | Rust |
| 4 | Wire into executor registry | Rust |
| 5 | Swift UDS server skeleton | Swift |
| 6 | Ping/pong integration test | Rust + Swift |
| 7 | Screenshot + OCR | Swift |
| 8 | Accessibility permissions | Swift |
| 9 | Mouse + keyboard + windows | Swift |
| 10 | Canvas WKWebView overlay | Swift |
| 11 | Agent integration + E2E test | Rust + Swift |

## Quick Reference

```bash
# Build Rust Core
cargo build -p alephcore

# Build macOS App
cd apps/macos && xcodegen generate
xcodebuild -scheme Aleph -configuration Debug -destination 'platform=macOS' build

# Run tests
cargo test -p alephcore -- desktop

# Check socket is running
ls -la ~/.aleph/desktop.sock
```
