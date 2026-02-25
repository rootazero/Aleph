# Semantic Targeting & Action Primitives — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add ref-based semantic UI targeting and missing action primitives (scroll, drag, hover, double-click, paste) to the Desktop Bridge.

**Architecture:** Bridge-side ref management over AX Tree. Core defines types, Bridge implements ref generation/resolution/actions. All actions support dual-track targeting: `ref` OR `(x, y)`. JSON-RPC 2.0 over UDS unchanged.

**Tech Stack:** Rust (core types, tool args, JSON-RPC serialization) + Swift (AX traversal, ref store, CGEvent actions, DesktopBridgeServer routing)

**Design doc:** `docs/plans/2026-02-25-semantic-targeting-and-action-primitives-design.md`

---

## Task 1: Add Core Types for Snapshot & New Actions (Rust)

**Files:**
- Modify: `core/src/desktop/types.rs`
- Modify: `core/src/desktop/mod.rs`

**Step 1: Add snapshot and ref types to `types.rs`**

Add after the existing `CanvasPosition` definition (after line 28):

```rust
/// Element reference ID (e.g. "e1", "e12").
pub type RefId = String;

/// A resolved UI element from a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolvedElement {
    pub ref_id: RefId,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub frame: ScreenRegion,
}

/// Statistics about a UI snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotStats {
    pub total_elements: u32,
    pub interactive: u32,
    pub max_depth: u32,
}
```

**Step 2: Expand `DesktopRequest` enum**

Replace the existing `DesktopRequest` enum (lines 47-66) with:

```rust
#[derive(Debug, Clone)]
pub enum DesktopRequest {
    // Perception (existing)
    Screenshot { region: Option<ScreenRegion> },
    Ocr { image_base64: Option<String> },
    AxTree { app_bundle_id: Option<String> },

    // Perception (new)
    Snapshot {
        app_bundle_id: Option<String>,
        max_depth: Option<u32>,
        include_non_interactive: Option<bool>,
    },

    // Action (existing — upgraded with ref support)
    Click {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        button: MouseButton,
    },
    TypeText {
        ref_id: Option<String>,
        text: String,
    },
    KeyCombo { keys: Vec<String> },
    LaunchApp { bundle_id: String },
    WindowList,
    FocusWindow { window_id: u32 },

    // Action (new)
    Scroll {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        delta_x: f64,
        delta_y: f64,
    },
    DoubleClick {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
        button: MouseButton,
    },
    Drag {
        start_ref: Option<String>,
        start_x: Option<f64>,
        start_y: Option<f64>,
        end_ref: Option<String>,
        end_x: Option<f64>,
        end_y: Option<f64>,
        duration_ms: Option<u64>,
    },
    Hover {
        ref_id: Option<String>,
        x: Option<f64>,
        y: Option<f64>,
    },
    Paste { text: String },

    // Canvas (unchanged)
    CanvasShow { html: String, position: CanvasPosition },
    CanvasHide,
    CanvasUpdate { patch: serde_json::Value },

    // Internal
    Ping,
}
```

**Step 3: Update `mod.rs` re-exports**

Replace line 7-9 of `core/src/desktop/mod.rs`:

```rust
pub use types::{
    CanvasPosition, DesktopRequest, DesktopResponse, DesktopRpcError, MouseButton, RefId,
    ResolvedElement, ScreenRegion, SnapshotStats,
};
```

**Step 4: Run `cargo check` to verify compilation**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | head -30`

Expected: Compilation errors in `client.rs` (existing `Click` variant changed). That's expected — we fix it in the next step.

**Step 5: Update `request_to_jsonrpc` in `client.rs`**

Replace the `Click` match arm (line 148-154) with:

```rust
DesktopRequest::Click { ref_id, x, y, button } => {
    let btn = match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    };
    ("desktop.click", json!({ "ref": ref_id, "x": x, "y": y, "button": btn }))
}
```

Replace the `TypeText` match arm (line 156):

```rust
DesktopRequest::TypeText { ref_id, text } => {
    ("desktop.type_text", json!({ "ref": ref_id, "text": text }))
}
```

Add new match arms before the `CanvasShow` arm:

```rust
DesktopRequest::Snapshot { app_bundle_id, max_depth, include_non_interactive } => {
    ("desktop.snapshot", json!({
        "app_bundle_id": app_bundle_id,
        "max_depth": max_depth,
        "include_non_interactive": include_non_interactive,
    }))
}
DesktopRequest::Scroll { ref_id, x, y, delta_x, delta_y } => {
    ("desktop.scroll", json!({
        "ref": ref_id, "x": x, "y": y,
        "delta_x": delta_x, "delta_y": delta_y,
    }))
}
DesktopRequest::DoubleClick { ref_id, x, y, button } => {
    let btn = match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    };
    ("desktop.double_click", json!({ "ref": ref_id, "x": x, "y": y, "button": btn }))
}
DesktopRequest::Drag { start_ref, start_x, start_y, end_ref, end_x, end_y, duration_ms } => {
    ("desktop.drag", json!({
        "start_ref": start_ref, "start_x": start_x, "start_y": start_y,
        "end_ref": end_ref, "end_x": end_x, "end_y": end_y,
        "duration_ms": duration_ms,
    }))
}
DesktopRequest::Hover { ref_id, x, y } => {
    ("desktop.hover", json!({ "ref": ref_id, "x": x, "y": y }))
}
DesktopRequest::Paste { text } => {
    ("desktop.paste", json!({ "text": text }))
}
```

**Step 6: Fix existing tests in `client.rs`**

Update `test_request_to_jsonrpc_click` (lines 214-222):

```rust
#[test]
fn test_request_to_jsonrpc_click() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Click {
        ref_id: None,
        x: Some(100.0),
        y: Some(200.0),
        button: MouseButton::Left,
    });
    assert_eq!(method, "desktop.click");
    assert_eq!(params["button"], "left");
}
```

**Step 7: Add tests for new request types**

Add at end of `tests` module in `client.rs`:

```rust
#[test]
fn test_request_to_jsonrpc_snapshot() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Snapshot {
        app_bundle_id: None,
        max_depth: Some(5),
        include_non_interactive: Some(false),
    });
    assert_eq!(method, "desktop.snapshot");
    assert_eq!(params["max_depth"], 5);
}

#[test]
fn test_request_to_jsonrpc_scroll() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Scroll {
        ref_id: Some("e3".into()),
        x: None,
        y: None,
        delta_x: 0.0,
        delta_y: -200.0,
    });
    assert_eq!(method, "desktop.scroll");
    assert_eq!(params["ref"], "e3");
    assert_eq!(params["delta_y"], -200.0);
}

#[test]
fn test_request_to_jsonrpc_double_click() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::DoubleClick {
        ref_id: Some("e1".into()),
        x: None,
        y: None,
        button: MouseButton::Left,
    });
    assert_eq!(method, "desktop.double_click");
    assert_eq!(params["ref"], "e1");
}

#[test]
fn test_request_to_jsonrpc_drag() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Drag {
        start_ref: Some("e1".into()),
        start_x: None,
        start_y: None,
        end_ref: Some("e5".into()),
        end_x: None,
        end_y: None,
        duration_ms: Some(500),
    });
    assert_eq!(method, "desktop.drag");
    assert_eq!(params["start_ref"], "e1");
    assert_eq!(params["end_ref"], "e5");
}

#[test]
fn test_request_to_jsonrpc_hover() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Hover {
        ref_id: None,
        x: Some(300.0),
        y: Some(400.0),
    });
    assert_eq!(method, "desktop.hover");
    assert_eq!(params["x"], 300.0);
}

#[test]
fn test_request_to_jsonrpc_paste() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Paste {
        text: "hello".into(),
    });
    assert_eq!(method, "desktop.paste");
    assert_eq!(params["text"], "hello");
}

#[test]
fn test_request_to_jsonrpc_click_with_ref() {
    let (method, params) = request_to_jsonrpc(&DesktopRequest::Click {
        ref_id: Some("e7".into()),
        x: None,
        y: None,
        button: MouseButton::Left,
    });
    assert_eq!(method, "desktop.click");
    assert_eq!(params["ref"], "e7");
    assert!(params["x"].is_null());
}
```

**Step 8: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore desktop -- --nocapture 2>&1 | tail -20`

Expected: All tests PASS.

**Step 9: Commit**

```bash
git add core/src/desktop/types.rs core/src/desktop/mod.rs core/src/desktop/client.rs
git commit -m "desktop: add core types for snapshot, ref system, and new action primitives"
```

---

## Task 2: Update Desktop Tool Args & Build Request (Rust)

**Files:**
- Modify: `core/src/builtin_tools/desktop.rs`

**Step 1: Add new fields to `DesktopArgs`**

Add these fields after the existing `patch` field (after line 76):

```rust
    /// Element ref from snapshot (alternative to x/y coordinates).
    /// Use snapshot action first to get refs.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ref")]
    pub ref_id: Option<String>,

    /// Start element ref for drag (alternative to start_x/start_y).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ref: Option<String>,

    /// Start X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_x: Option<f64>,

    /// Start Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_y: Option<f64>,

    /// End element ref for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_ref: Option<String>,

    /// End X for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_x: Option<f64>,

    /// End Y for drag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_y: Option<f64>,

    /// Horizontal scroll amount (pixels, negative=left).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,

    /// Vertical scroll amount (pixels, negative=up).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,

    /// Drag duration in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,

    /// Max AX tree depth for snapshot (default: 5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,

    /// Include non-interactive elements in snapshot refs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_non_interactive: Option<bool>,
```

**Step 2: Update tool DESCRIPTION**

Replace the `DESCRIPTION` const (lines 112-143) with:

```rust
    const DESCRIPTION: &'static str = r#"Control the desktop: screenshot, OCR, UI snapshot with element refs, keyboard/mouse, canvas overlays.

Requires the Aleph Desktop Bridge (starts automatically with the server).

Perception:
- snapshot: Capture UI structure with element refs. Returns tree (text), refs map, interactive list. Use BEFORE click/scroll/drag to target elements by ref.
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen (or provided image_base64). Returns {text, lines[]}
- ax_tree: Raw accessibility tree (for debugging, prefer snapshot)

Actions (support ref OR x/y targeting):
- click: Click element. ref="e3" (from snapshot) or x/y coordinates. Optional button.
- double_click: Double-click element. Same targeting as click.
- scroll: Scroll at element. ref or x/y + delta_x/delta_y (pixels, negative=up/left).
- drag: Drag between elements. start_ref/end_ref or start_x,y/end_x,y + duration_ms.
- hover: Move mouse to element without clicking. ref or x/y.
- type_text: Type text string. Optional ref to focus element first.
- key_combo: Press key combo, e.g. keys=["cmd","c"]
- paste: Write text to clipboard and paste (Cmd+V).
- launch_app: Launch by bundle_id
- window_list: List open windows
- focus_window: Bring window to front

Canvas:
- canvas_show/canvas_hide/canvas_update: HTML overlay with A2UI patches.

Examples:
{"action":"snapshot"}
{"action":"click","ref":"e3"}
{"action":"click","x":500,"y":300}
{"action":"scroll","ref":"e7","delta_y":-300}
{"action":"double_click","ref":"e1"}
{"action":"drag","start_ref":"e5","end_ref":"e12"}
{"action":"hover","ref":"e3"}
{"action":"type_text","ref":"e1","text":"Hello"}
{"action":"paste","text":"clipboard content"}"#;
```

**Step 3: Update `build_request()` function**

Replace the `"click"` match arm (lines 201-209):

```rust
"click" => {
    let ref_id = args.ref_id.clone();
    let x = args.x;
    let y = args.y;
    if ref_id.is_none() && (x.is_none() || y.is_none()) {
        return Err("click requires 'ref' or both 'x' and 'y' coordinates".to_string());
    }
    DesktopRequest::Click {
        ref_id,
        x,
        y,
        button: args.button.clone().unwrap_or(MouseButton::Left),
    }
}
```

Replace the `"type_text"` arm (lines 210-212):

```rust
"type_text" => DesktopRequest::TypeText {
    ref_id: args.ref_id.clone(),
    text: args.text.clone().unwrap_or_default(),
},
```

Add new match arms before the `"canvas_show"` arm:

```rust
"snapshot" => DesktopRequest::Snapshot {
    app_bundle_id: args.app_bundle_id.clone(),
    max_depth: args.max_depth,
    include_non_interactive: args.include_non_interactive,
},
"scroll" => {
    let ref_id = args.ref_id.clone();
    let x = args.x;
    let y = args.y;
    if ref_id.is_none() && (x.is_none() || y.is_none()) {
        return Err("scroll requires 'ref' or both 'x' and 'y' coordinates".to_string());
    }
    DesktopRequest::Scroll {
        ref_id,
        x,
        y,
        delta_x: args.delta_x.unwrap_or(0.0),
        delta_y: args.delta_y.unwrap_or(0.0),
    }
}
"double_click" => {
    let ref_id = args.ref_id.clone();
    let x = args.x;
    let y = args.y;
    if ref_id.is_none() && (x.is_none() || y.is_none()) {
        return Err("double_click requires 'ref' or both 'x' and 'y' coordinates".to_string());
    }
    DesktopRequest::DoubleClick {
        ref_id,
        x,
        y,
        button: args.button.clone().unwrap_or(MouseButton::Left),
    }
}
"drag" => {
    let has_start = args.start_ref.is_some() || (args.start_x.is_some() && args.start_y.is_some());
    let has_end = args.end_ref.is_some() || (args.end_x.is_some() && args.end_y.is_some());
    if !has_start || !has_end {
        return Err("drag requires start (start_ref or start_x+start_y) and end (end_ref or end_x+end_y)".to_string());
    }
    DesktopRequest::Drag {
        start_ref: args.start_ref.clone(),
        start_x: args.start_x,
        start_y: args.start_y,
        end_ref: args.end_ref.clone(),
        end_x: args.end_x,
        end_y: args.end_y,
        duration_ms: args.duration_ms,
    }
}
"hover" => {
    let ref_id = args.ref_id.clone();
    let x = args.x;
    let y = args.y;
    if ref_id.is_none() && (x.is_none() || y.is_none()) {
        return Err("hover requires 'ref' or both 'x' and 'y' coordinates".to_string());
    }
    DesktopRequest::Hover { ref_id, x, y }
}
"paste" => DesktopRequest::Paste {
    text: args.text.clone().unwrap_or_default(),
},
```

Update the error message in the `other` fallback arm:

```rust
other => {
    return Err(format!(
        "Unknown desktop action: '{}'. Valid: snapshot, screenshot, ocr, ax_tree, \
         click, double_click, scroll, drag, hover, type_text, key_combo, paste, \
         launch_app, window_list, focus_window, \
         canvas_show, canvas_hide, canvas_update",
        other
    ));
}
```

**Step 4: Fix existing tests and add new ones**

Update `make_args` to include new fields:

```rust
fn make_args(action: &str) -> DesktopArgs {
    DesktopArgs {
        action: action.into(),
        region: None,
        image_base64: None,
        app_bundle_id: None,
        x: None,
        y: None,
        button: None,
        text: None,
        keys: None,
        bundle_id: None,
        window_id: None,
        html: None,
        position: None,
        patch: None,
        ref_id: None,
        start_ref: None,
        start_x: None,
        start_y: None,
        end_ref: None,
        end_x: None,
        end_y: None,
        delta_x: None,
        delta_y: None,
        duration_ms: None,
        max_depth: None,
        include_non_interactive: None,
    }
}
```

Update `test_build_request_click` to use new field structure:

```rust
#[test]
fn test_build_request_click() {
    let mut args = make_args("click");
    args.x = Some(100.0);
    args.y = Some(200.0);
    args.button = Some(MouseButton::Right);
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Click { ref_id: None, button: MouseButton::Right, .. }));
}
```

Add new tests:

```rust
#[test]
fn test_build_request_snapshot() {
    let mut args = make_args("snapshot");
    args.max_depth = Some(3);
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Snapshot { max_depth: Some(3), .. }));
}

#[test]
fn test_build_request_click_with_ref() {
    let mut args = make_args("click");
    args.ref_id = Some("e3".into());
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Click { ref_id: Some(_), .. }));
}

#[test]
fn test_build_request_click_no_target() {
    let args = make_args("click");
    assert!(build_request(&args).is_err());
}

#[test]
fn test_build_request_scroll() {
    let mut args = make_args("scroll");
    args.ref_id = Some("e7".into());
    args.delta_y = Some(-300.0);
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Scroll { delta_y, .. } if delta_y == -300.0));
}

#[test]
fn test_build_request_double_click() {
    let mut args = make_args("double_click");
    args.ref_id = Some("e1".into());
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::DoubleClick { .. }));
}

#[test]
fn test_build_request_drag() {
    let mut args = make_args("drag");
    args.start_ref = Some("e1".into());
    args.end_ref = Some("e5".into());
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Drag { .. }));
}

#[test]
fn test_build_request_drag_missing_end() {
    let mut args = make_args("drag");
    args.start_ref = Some("e1".into());
    assert!(build_request(&args).is_err());
}

#[test]
fn test_build_request_hover() {
    let mut args = make_args("hover");
    args.x = Some(100.0);
    args.y = Some(200.0);
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Hover { .. }));
}

#[test]
fn test_build_request_paste() {
    let mut args = make_args("paste");
    args.text = Some("hello".into());
    let req = build_request(&args).unwrap();
    assert!(matches!(req, DesktopRequest::Paste { text } if text == "hello"));
}
```

**Step 5: Run tests**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore desktop -- --nocapture 2>&1 | tail -20`

Expected: All tests PASS.

**Step 6: Commit**

```bash
git add core/src/builtin_tools/desktop.rs
git commit -m "desktop: update tool args and build_request for snapshot, ref targeting, and new actions"
```

---

## Task 3: Implement RefStore in Swift

**Files:**
- Create: `apps/macos/Aleph/Sources/DesktopBridge/RefStore.swift`

**Step 1: Create RefStore.swift**

```swift
// RefStore.swift
// Stores ref-to-element mappings from UI snapshots for the Desktop Bridge.

import Foundation

/// A resolved UI element from a snapshot.
struct ResolvedElement {
    let refId: String
    let role: String
    let label: String?
    let frame: CGRect
}

/// Error types for ref resolution.
enum RefError: LocalizedError {
    case notFound(String)
    case noSnapshot

    var errorDescription: String? {
        switch self {
        case .notFound(let refId):
            return "ref '\(refId)' not found in current snapshot. Run snapshot to refresh refs."
        case .noSnapshot:
            return "No snapshot available. Run snapshot first."
        }
    }
}

/// Stores the current ref map from the most recent snapshot.
/// Thread-safe: all access is serialized through a lock.
final class RefStore: @unchecked Sendable {
    static let shared = RefStore()

    private let lock = NSLock()
    private var refs: [String: ResolvedElement] = [:]
    private var snapshotTimestamp: Date?

    /// Replace refs with a new set from a fresh snapshot.
    func update(newRefs: [String: ResolvedElement]) {
        lock.lock()
        defer { lock.unlock() }
        refs = newRefs
        snapshotTimestamp = Date()
    }

    /// Resolve a ref ID to a center point for action targeting.
    func resolve(_ refId: String) -> Result<CGPoint, RefError> {
        lock.lock()
        defer { lock.unlock() }

        guard snapshotTimestamp != nil else {
            return .failure(.noSnapshot)
        }

        guard let element = refs[refId] else {
            return .failure(.notFound(refId))
        }

        let center = CGPoint(
            x: element.frame.origin.x + element.frame.size.width / 2,
            y: element.frame.origin.y + element.frame.size.height / 2
        )
        return .success(center)
    }

    /// Clear stored refs.
    func clear() {
        lock.lock()
        defer { lock.unlock() }
        refs.removeAll()
        snapshotTimestamp = nil
    }
}
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aleph/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph/Sources/DesktopBridge/RefStore.swift`

Expected: No syntax errors.

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/RefStore.swift
git commit -m "desktop: add RefStore for snapshot ref management (Swift)"
```

---

## Task 4: Implement Snapshot in Perception.swift

**Files:**
- Modify: `apps/macos/Aleph/Sources/DesktopBridge/Perception.swift`

**Step 1: Add interactive roles constant and snapshot method**

Add at the bottom of the `Perception` class, before the closing `}`:

```swift
    // MARK: - UI Snapshot (ref-based)

    /// Roles considered interactive (ref-targetable).
    private static let interactiveRoles: Set<String> = [
        "AXButton", "AXTextField", "AXTextArea", "AXCheckBox", "AXSlider",
        "AXPopUpButton", "AXMenuItem", "AXLink", "AXTab", "AXRadioButton",
        "AXComboBox", "AXScrollArea", "AXTable", "AXList", "AXIncrementor",
        "AXDisclosureTriangle", "AXColorWell", "AXMenuButton",
    ]

    /// Capture a structured UI snapshot with ref IDs for interactive elements.
    func snapshot(appBundleId: String?, maxDepth: Int, includeNonInteractive: Bool) async -> Result<Any, Error> {
        let axApp: AXUIElement
        let resolvedBundleId: String
        let appName: String

        if let bundleId = appBundleId {
            guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).first else {
                return .failure(NSError(domain: "Perception", code: 6,
                                       userInfo: [NSLocalizedDescriptionKey: "App not running: \(bundleId)"]))
            }
            axApp = AXUIElementCreateApplication(app.processIdentifier)
            resolvedBundleId = bundleId
            appName = app.localizedName ?? bundleId
        } else {
            guard let frontmost = NSWorkspace.shared.frontmostApplication else {
                return .failure(NSError(domain: "Perception", code: 7,
                                       userInfo: [NSLocalizedDescriptionKey: "No frontmost application"]))
            }
            axApp = AXUIElementCreateApplication(frontmost.processIdentifier)
            resolvedBundleId = frontmost.bundleIdentifier ?? "unknown"
            appName = frontmost.localizedName ?? "Unknown"
        }

        var refCounter = 0
        var refs: [String: ResolvedElement] = [:]
        var interactiveRefs: [String] = []
        var totalElements = 0

        // Build tree text and refs simultaneously
        let tree = buildSnapshotTree(
            element: axApp,
            depth: 0,
            maxDepth: maxDepth,
            indent: "",
            refCounter: &refCounter,
            refs: &refs,
            interactiveRefs: &interactiveRefs,
            totalElements: &totalElements,
            includeNonInteractive: includeNonInteractive
        )

        // Update the shared RefStore
        RefStore.shared.update(newRefs: refs)

        // Build refs dict for JSON response
        let refsDict: [String: Any] = refs.mapValues { element in
            var entry: [String: Any] = [
                "role": element.role,
                "frame": [
                    "x": element.frame.origin.x,
                    "y": element.frame.origin.y,
                    "w": element.frame.size.width,
                    "h": element.frame.size.height,
                ] as [String: Any],
            ]
            if let label = element.label {
                entry["label"] = label
            }
            return entry
        }

        return .success([
            "app_bundle_id": resolvedBundleId,
            "app_name": appName,
            "tree": tree,
            "refs": refsDict,
            "interactive": interactiveRefs,
            "stats": [
                "total_elements": totalElements,
                "interactive": interactiveRefs.count,
                "max_depth": maxDepth,
            ] as [String: Any],
        ] as [String: Any])
    }

    private func buildSnapshotTree(
        element: AXUIElement,
        depth: Int,
        maxDepth: Int,
        indent: String,
        refCounter: inout Int,
        refs: inout [String: ResolvedElement],
        interactiveRefs: inout [String],
        totalElements: inout Int,
        includeNonInteractive: Bool
    ) -> String {
        guard depth < maxDepth else { return "" }

        totalElements += 1

        // Extract role
        var roleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &roleValue)
        let role = (roleValue as? String) ?? "AXUnknown"

        // Extract label (title or description)
        var titleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXTitleAttribute as CFString, &titleValue)
        let title = titleValue as? String

        var descValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXDescriptionAttribute as CFString, &descValue)
        let desc = descValue as? String

        let label = title.flatMap({ $0.isEmpty ? nil : $0 }) ?? desc.flatMap({ $0.isEmpty ? nil : $0 })

        // Extract frame
        var positionValue: AnyObject?
        var sizeValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue)
        AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue)

        var frame: CGRect?
        if let rawPos = positionValue, let rawSz = sizeValue,
           CFGetTypeID(rawPos as CFTypeRef) == AXValueGetTypeID(),
           CFGetTypeID(rawSz as CFTypeRef) == AXValueGetTypeID() {
            // swiftlint:disable:next force_cast
            let pos = rawPos as! AXValue
            // swiftlint:disable:next force_cast
            let sz = rawSz as! AXValue
            var point = CGPoint.zero
            var size = CGSize.zero
            AXValueGetValue(pos, .cgPoint, &point)
            AXValueGetValue(sz, .cgSize, &size)
            frame = CGRect(origin: point, size: size)
        }

        let isInteractive = Self.interactiveRoles.contains(role)

        // Build line
        var line = indent
        var refId: String?

        if isInteractive, let f = frame {
            refCounter += 1
            let rid = "e\(refCounter)"
            refId = rid

            let resolved = ResolvedElement(
                refId: rid,
                role: role,
                label: label,
                frame: f
            )
            refs[rid] = resolved
            interactiveRefs.append(rid)

            let labelStr = label.map { " '\($0)'" } ?? ""
            let frameStr = "(\(Int(f.origin.x)),\(Int(f.origin.y)) \(Int(f.size.width))x\(Int(f.size.height)))"
            line += "[\(rid)] \(role)\(labelStr) \(frameStr)"
        } else if includeNonInteractive, let f = frame {
            // Non-interactive with frame — include in refs if requested
            refCounter += 1
            let rid = "e\(refCounter)"
            refId = rid

            let resolved = ResolvedElement(
                refId: rid,
                role: role,
                label: label,
                frame: f
            )
            refs[rid] = resolved

            let labelStr = label.map { " '\($0)'" } ?? ""
            let frameStr = "(\(Int(f.origin.x)),\(Int(f.origin.y)) \(Int(f.size.width))x\(Int(f.size.height)))"
            line += "[\(rid)] \(role)\(labelStr) \(frameStr)"
        } else {
            let labelStr = label.map { " '\($0)'" } ?? ""
            line += "\(role)\(labelStr)"
        }

        // Recurse into children
        var childrenValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &childrenValue)
        var childLines = ""
        if let children = childrenValue as? [AXUIElement] {
            for child in children {
                let childTree = buildSnapshotTree(
                    element: child,
                    depth: depth + 1,
                    maxDepth: maxDepth,
                    indent: indent + "  ",
                    refCounter: &refCounter,
                    refs: &refs,
                    interactiveRefs: &interactiveRefs,
                    totalElements: &totalElements,
                    includeNonInteractive: includeNonInteractive
                )
                if !childTree.isEmpty {
                    childLines += "\n" + childTree
                }
            }
        }

        return line + childLines
    }
```

**Step 2: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aleph/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph/Sources/DesktopBridge/Perception.swift`

Expected: No syntax errors.

**Step 3: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/Perception.swift
git commit -m "desktop: implement UI snapshot with ref generation in Perception.swift"
```

---

## Task 5: Implement New Action Primitives in Swift

**Files:**
- Modify: `apps/macos/Aleph/Sources/DesktopBridge/Action.swift`

**Step 1: Add ref-resolution helper to Action class**

Add at the top of the `Action` class, after `static let shared = Action()`:

```swift
    // MARK: - Ref Resolution

    /// Resolve a target from either a ref ID or explicit coordinates.
    private func resolveTarget(refId: String?, x: Double?, y: Double?) -> Result<CGPoint, Error> {
        if let refId = refId {
            switch RefStore.shared.resolve(refId) {
            case .success(let point):
                return .success(point)
            case .failure(let err):
                return .failure(err)
            }
        }
        if let x = x, let y = y {
            return .success(CGPoint(x: x, y: y))
        }
        return .failure(NSError(domain: "Action", code: 10,
                                userInfo: [NSLocalizedDescriptionKey: "Either ref or (x, y) coordinates required"]))
    }
```

**Step 2: Add ref-aware click method**

Add after the existing `click` method:

```swift
    /// Click with ref-based or coordinate-based targeting.
    func clickTarget(refId: String?, x: Double?, y: Double?, button: String) async -> Result<Any, Error> {
        switch resolveTarget(refId: refId, x: x, y: y) {
        case .success(let point):
            return await click(x: point.x, y: point.y, button: button)
        case .failure(let err):
            return .failure(err)
        }
    }
```

**Step 3: Add double-click method**

Add after `clickTarget`:

```swift
    // MARK: - Double Click

    func doubleClick(refId: String?, x: Double?, y: Double?, button: String) async -> Result<Any, Error> {
        switch resolveTarget(refId: refId, x: x, y: y) {
        case .failure(let err):
            return .failure(err)
        case .success(let point):
            let (downType, upType, cgButton): (CGEventType, CGEventType, CGMouseButton)
            switch button.lowercased() {
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

            // First click
            down.setIntegerValueField(.mouseEventClickState, value: 1)
            up.setIntegerValueField(.mouseEventClickState, value: 1)
            down.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 50_000_000)
            up.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 50_000_000)

            // Second click
            guard let down2 = CGEvent(mouseEventSource: source, mouseType: downType,
                                      mouseCursorPosition: point, mouseButton: cgButton),
                  let up2 = CGEvent(mouseEventSource: source, mouseType: upType,
                                    mouseCursorPosition: point, mouseButton: cgButton)
            else {
                return .failure(NSError(domain: "Action", code: 1,
                                       userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse events"]))
            }
            down2.setIntegerValueField(.mouseEventClickState, value: 2)
            up2.setIntegerValueField(.mouseEventClickState, value: 2)
            down2.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 50_000_000)
            up2.post(tap: .cghidEventTap)

            return .success(["double_clicked": true, "x": point.x, "y": point.y, "button": button] as [String: Any])
        }
    }
```

**Step 4: Add scroll method**

```swift
    // MARK: - Scroll

    func scroll(refId: String?, x: Double?, y: Double?, deltaX: Double, deltaY: Double) async -> Result<Any, Error> {
        // If ref or coords given, move mouse there first
        if refId != nil || (x != nil && y != nil) {
            switch resolveTarget(refId: refId, x: x, y: y) {
            case .success(let point):
                // Move mouse to target position
                guard let source = CGEventSource(stateID: .hidSystemState),
                      let moveEvent = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                                              mouseCursorPosition: point, mouseButton: .left)
                else {
                    return .failure(NSError(domain: "Action", code: 8,
                                           userInfo: [NSLocalizedDescriptionKey: "Failed to create move event"]))
                }
                moveEvent.post(tap: .cghidEventTap)
                try? await Task.sleep(nanoseconds: 10_000_000) // 10ms settle
            case .failure(let err):
                return .failure(err)
            }
        }

        guard let source = CGEventSource(stateID: .hidSystemState),
              let scrollEvent = CGEvent(scrollWheelEvent2Source: source,
                                        units: .pixel,
                                        wheelCount: 2,
                                        wheel1: Int32(deltaY),
                                        wheel2: Int32(deltaX))
        else {
            return .failure(NSError(domain: "Action", code: 9,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create scroll event"]))
        }
        scrollEvent.post(tap: .cghidEventTap)

        return .success(["scrolled": true, "delta_x": deltaX, "delta_y": deltaY] as [String: Any])
    }
```

**Step 5: Add hover method**

```swift
    // MARK: - Hover

    func hover(refId: String?, x: Double?, y: Double?) async -> Result<Any, Error> {
        switch resolveTarget(refId: refId, x: x, y: y) {
        case .failure(let err):
            return .failure(err)
        case .success(let point):
            guard let source = CGEventSource(stateID: .hidSystemState),
                  let moveEvent = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                                          mouseCursorPosition: point, mouseButton: .left)
            else {
                return .failure(NSError(domain: "Action", code: 11,
                                       userInfo: [NSLocalizedDescriptionKey: "Failed to create move event"]))
            }
            moveEvent.post(tap: .cghidEventTap)
            return .success(["hovered": true, "x": point.x, "y": point.y] as [String: Any])
        }
    }
```

**Step 6: Add drag method**

```swift
    // MARK: - Drag

    func drag(startRefId: String?, startX: Double?, startY: Double?,
              endRefId: String?, endX: Double?, endY: Double?,
              durationMs: UInt64?) async -> Result<Any, Error> {
        let startPoint: CGPoint
        switch resolveTarget(refId: startRefId, x: startX, y: startY) {
        case .success(let p): startPoint = p
        case .failure(let err): return .failure(err)
        }

        let endPoint: CGPoint
        switch resolveTarget(refId: endRefId, x: endX, y: endY) {
        case .success(let p): endPoint = p
        case .failure(let err): return .failure(err)
        }

        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(NSError(domain: "Action", code: 12,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create event source"]))
        }

        let duration = durationMs ?? 300
        let steps = max(Int(duration / 16), 5) // ~60fps, minimum 5 steps
        let sleepPerStep = UInt64(duration) * 1_000_000 / UInt64(steps)

        // Mouse down at start
        guard let down = CGEvent(mouseEventSource: source, mouseType: .leftMouseDown,
                                 mouseCursorPosition: startPoint, mouseButton: .left)
        else {
            return .failure(NSError(domain: "Action", code: 12,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse down"]))
        }
        down.post(tap: .cghidEventTap)
        try? await Task.sleep(nanoseconds: 20_000_000) // 20ms

        // Interpolate drag path
        for i in 1...steps {
            let t = Double(i) / Double(steps)
            let x = startPoint.x + (endPoint.x - startPoint.x) * t
            let y = startPoint.y + (endPoint.y - startPoint.y) * t
            let pt = CGPoint(x: x, y: y)

            guard let dragEvent = CGEvent(mouseEventSource: source, mouseType: .leftMouseDragged,
                                          mouseCursorPosition: pt, mouseButton: .left)
            else { continue }
            dragEvent.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: sleepPerStep)
        }

        // Mouse up at end
        guard let up = CGEvent(mouseEventSource: source, mouseType: .leftMouseUp,
                               mouseCursorPosition: endPoint, mouseButton: .left)
        else {
            return .failure(NSError(domain: "Action", code: 12,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse up"]))
        }
        up.post(tap: .cghidEventTap)

        return .success([
            "dragged": true,
            "start": ["x": startPoint.x, "y": startPoint.y],
            "end": ["x": endPoint.x, "y": endPoint.y],
        ] as [String: Any])
    }
```

**Step 7: Add paste method**

```swift
    // MARK: - Paste

    func paste(text: String) async -> Result<Any, Error> {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)

        // Trigger Cmd+V
        return await keyCombo(keys: ["cmd", "v"])
    }
```

**Step 8: Add ref-aware type_text method**

```swift
    /// Type text with optional ref-based focus.
    func typeTextTarget(refId: String?, text: String) async -> Result<Any, Error> {
        if let refId = refId {
            // Click the element first to focus it
            let clickResult = await clickTarget(refId: refId, x: nil, y: nil, button: "left")
            if case .failure(let err) = clickResult {
                return .failure(err)
            }
            try? await Task.sleep(nanoseconds: 100_000_000) // 100ms to focus
        }
        return await typeText(text)
    }
```

**Step 9: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aleph/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph/Sources/DesktopBridge/Action.swift`

Expected: No syntax errors.

**Step 10: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/Action.swift
git commit -m "desktop: implement scroll, double-click, drag, hover, paste, and ref-aware targeting"
```

---

## Task 6: Wire Snapshot & New Actions in DesktopBridgeServer.swift

**Files:**
- Modify: `apps/macos/Aleph/Sources/DesktopBridge/DesktopBridgeServer.swift`

**Step 1: Add `desktop.snapshot` case to dispatch method**

Add after the `"desktop.ax_tree"` case (after line 219):

```swift
        case "desktop.snapshot":
            let bundleId = params["app_bundle_id"] as? String
            let maxDepth = params["max_depth"] as? Int ?? 5
            let includeNonInteractive = params["include_non_interactive"] as? Bool ?? false
            return runAsync {
                await Perception.shared.snapshot(
                    appBundleId: bundleId,
                    maxDepth: maxDepth,
                    includeNonInteractive: includeNonInteractive
                )
            }
```

**Step 2: Update `desktop.click` to support ref**

Replace the existing `"desktop.click"` case (lines 222-226):

```swift
        case "desktop.click":
            let refId = params["ref"] as? String
            let x = params["x"] as? Double
            let y = params["y"] as? Double
            let button = params["button"] as? String ?? "left"
            return runAsync { await Action.shared.clickTarget(refId: refId, x: x, y: y, button: button) }
```

**Step 3: Update `desktop.type_text` to support ref**

Replace the existing `"desktop.type_text"` case (lines 228-230):

```swift
        case "desktop.type_text":
            let refId = params["ref"] as? String
            let text = params["text"] as? String ?? ""
            return runAsync { await Action.shared.typeTextTarget(refId: refId, text: text) }
```

**Step 4: Add new action cases**

Add after the `"desktop.focus_window"` case (after line 245):

```swift
        case "desktop.scroll":
            let refId = params["ref"] as? String
            let x = params["x"] as? Double
            let y = params["y"] as? Double
            let deltaX = params["delta_x"] as? Double ?? 0
            let deltaY = params["delta_y"] as? Double ?? 0
            return runAsync { await Action.shared.scroll(refId: refId, x: x, y: y, deltaX: deltaX, deltaY: deltaY) }

        case "desktop.double_click":
            let refId = params["ref"] as? String
            let x = params["x"] as? Double
            let y = params["y"] as? Double
            let button = params["button"] as? String ?? "left"
            return runAsync { await Action.shared.doubleClick(refId: refId, x: x, y: y, button: button) }

        case "desktop.drag":
            let startRef = params["start_ref"] as? String
            let startX = params["start_x"] as? Double
            let startY = params["start_y"] as? Double
            let endRef = params["end_ref"] as? String
            let endX = params["end_x"] as? Double
            let endY = params["end_y"] as? Double
            let durationMs = params["duration_ms"] as? UInt64
            return runAsync {
                await Action.shared.drag(
                    startRefId: startRef, startX: startX, startY: startY,
                    endRefId: endRef, endX: endX, endY: endY,
                    durationMs: durationMs
                )
            }

        case "desktop.hover":
            let refId = params["ref"] as? String
            let x = params["x"] as? Double
            let y = params["y"] as? Double
            return runAsync { await Action.shared.hover(refId: refId, x: x, y: y) }

        case "desktop.paste":
            let text = params["text"] as? String ?? ""
            return runAsync { await Action.shared.paste(text: text) }
```

**Step 5: Verify Swift syntax**

Run: `~/.uv/python3/bin/python /Volumes/TBU4/Workspace/Aleph/Scripts/verify_swift_syntax.py /Volumes/TBU4/Workspace/Aleph/apps/macos/Aleph/Sources/DesktopBridge/DesktopBridgeServer.swift`

Expected: No syntax errors.

**Step 6: Commit**

```bash
git add apps/macos/Aleph/Sources/DesktopBridge/DesktopBridgeServer.swift
git commit -m "desktop: wire snapshot and new actions in DesktopBridgeServer dispatch"
```

---

## Task 7: Rust Compilation & Full Test Suite

**Files:**
- All modified Rust files

**Step 1: Run full cargo check**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo check -p alephcore 2>&1 | tail -20`

Expected: No errors. Fix any compilation issues.

**Step 2: Run full test suite**

Run: `cd /Volumes/TBU4/Workspace/Aleph && cargo test -p alephcore 2>&1 | tail -30`

Expected: All tests pass. Some tests may be ignored (require macOS App running).

**Step 3: Run Swift syntax verification on all modified files**

Run:
```bash
for f in apps/macos/Aleph/Sources/DesktopBridge/*.swift; do
  echo "=== $f ==="
  ~/.uv/python3/bin/python Scripts/verify_swift_syntax.py "$f"
done
```

Expected: No syntax errors in any file.

**Step 4: Commit any fixes**

If any fixes were needed:
```bash
git add -A
git commit -m "desktop: fix compilation and test issues"
```

---

## Task 8: Regenerate Xcode Project

**Files:**
- Xcode project files

**Step 1: Regenerate with xcodegen**

Run: `cd /Volumes/TBU4/Workspace/Aleph/apps/macos && xcodegen generate`

Expected: `Generated project` message. The new `RefStore.swift` file should be included automatically.

**Step 2: Verify build compiles**

Run: `cd /Volumes/TBU4/Workspace/Aleph/apps/macos && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build 2>&1 | tail -20`

Expected: `BUILD SUCCEEDED`. If it fails, fix Swift compilation errors.

**Step 3: Commit if xcodegen changed project file**

```bash
git add apps/macos/Aleph.xcodeproj
git commit -m "desktop: regenerate Xcode project with RefStore.swift"
```
