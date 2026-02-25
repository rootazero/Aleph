# Semantic Targeting System & Action Primitives

> **Status**: Approved
> **Date**: 2026-02-25
> **Scope**: Desktop Bridge enhancement — ref-based UI targeting + missing action primitives
> **Approach**: Perception-Action Fusion (方案 B)

---

## Motivation

### Problem

Aleph's current Desktop Bridge uses **pixel coordinates** `(x, y)` for all actions (click, type). This forces the AI agent to:

1. Take a screenshot → OCR/analyze → estimate pixel coordinates → hope they're correct
2. Deal with brittle targeting that breaks on window resize, scroll, or DPI change
3. Have no structured understanding of what UI elements exist on screen

### OpenClaw Comparison

OpenClaw uses **ref-based targeting** via Playwright DOM snapshots: the AI says `click(ref="e12")` and the system resolves `e12` to coordinates. This is dramatically more robust for LLM-driven automation.

However, OpenClaw's approach only covers **browser content** (Playwright DOM). Aleph's advantage is access to **macOS Accessibility (AXUIElement)**, which covers every native application.

### Goal

Build a semantic targeting system that gives Aleph **better-than-OpenClaw** desktop automation:
- **Broader scope**: Any native app, not just browsers
- **Same ergonomics**: `click(ref="e3")` instead of `click(x=500, y=300)`
- **Complete actions**: Add scroll, drag, hover, double-click, paste

---

## Architecture

### Design Principles

1. **R1 Compliance**: Core defines types, Bridge implements. Core never resolves refs.
2. **Dual-track targeting**: All actions accept `ref` OR `(x, y)` — backward compatible.
3. **Bridge-side ref management**: Ref map generated and stored in Bridge memory, refreshed per snapshot.
4. **AI-friendly output**: Structured text tree that LLMs can read directly.

### System Flow

```
                      ┌────────────────────┐
AI Agent              │  desktop.snapshot   │  ← New action
                      │  returns:           │
                      │    tree (text)       │  ← AI-readable structure
                      │    refs: {e1..eN}    │  ← ref → bounds mapping
                      │    interactive: [..] │  ← actionable elements
                      └─────────┬──────────┘
                                │
              ┌─────────────────┼─────────────────┐
              ▼                 ▼                  ▼
    desktop.click        desktop.scroll      desktop.drag
    ref="e12"            ref="e7"             start_ref="e3"
    OR x=500,y=300       delta_y=-200         end_ref="e15"
```

### Responsibility Split

| Responsibility | Location | Reason |
|---------------|----------|--------|
| AX Tree traversal | Bridge (Swift/Tauri) | Platform API |
| Ref ID generation | Bridge | Depends on AX traversal result |
| Ref map storage | Bridge (in-memory) | Refreshed per snapshot call |
| Ref → coordinate resolution | Bridge | Coordinates are platform-specific |
| Tree text formatting | Bridge | Needs AX attribute knowledge |
| Action execution | Bridge | CGEvent / platform API |
| Type definitions | Core (Rust) | Cross-platform consistency |
| Tool description & args | Core (Rust) | Agent-facing interface |

---

## Section 1: Semantic Targeting (Ref System)

### Ref Generation Rules

1. **Source**: AX Tree traversal — extract all elements with `role` + `frame`
2. **ID format**: `e{N}` where N starts at 1, incrementing (reset per snapshot)
3. **Interactive roles** (prioritized): `button`, `textField`, `checkbox`, `slider`, `popUpButton`, `menuItem`, `link`, `tab`, `radioButton`, `comboBox`, `scrollArea`, `table`
4. **Deduplication**: Same role + same label → append `:nth=N` (e.g., `e5` and `e6` both "OK" buttons → labels show `nth=1`, `nth=2`)
5. **Storage**: Bridge holds `HashMap<RefId, ResolvedElement>` in memory, replaced on each `snapshot` call

### New RPC Method: `desktop.snapshot`

**Request**:
```json
{
  "method": "desktop.snapshot",
  "params": {
    "app_bundle_id": null,
    "max_depth": 5,
    "include_non_interactive": false
  }
}
```

**Response**:
```json
{
  "result": {
    "app_bundle_id": "com.apple.Safari",
    "app_name": "Safari",
    "tree": "AXApplication 'Safari'\n  AXWindow 'GitHub - Pull Request'\n    AXWebArea\n      [e1] AXTextField 'Search' (200,80 400x32)\n      [e3] AXButton 'New pull request' (900,80 120x30)\n      [e7] AXLink 'Files changed' (300,150 100x20)",
    "refs": {
      "e1": { "role": "textField", "label": "Search", "frame": {"x":200,"y":80,"w":400,"h":32} },
      "e3": { "role": "button", "label": "New pull request", "frame": {"x":900,"y":80,"w":120,"h":30} },
      "e7": { "role": "link", "label": "Files changed", "frame": {"x":300,"y":150,"w":100,"h":20} }
    },
    "interactive": ["e1", "e3", "e7"],
    "stats": { "total_elements": 127, "interactive": 18, "max_depth": 5 }
  }
}
```

### Tree Text Format

Indented text with ref annotations on interactive elements:

```
AXApplication 'Safari'
  AXWindow 'GitHub - Pull Request'
    AXWebArea
      [e1] AXTextField 'Search' (200,80 400x32)
      AXGroup
        [e2] AXButton 'Code' (100,120 60x24)
        [e3] AXButton 'Issues' (170,120 60x24)
        [e4] AXButton 'Pull requests' (240,120 90x24)
      [e5] AXStaticText 'Add semantic targeting...'
      AXScrollArea
        [e6] AXLink 'src/desktop/types.rs' (50,200 200x18)
        [e7] AXLink 'src/desktop/client.rs' (50,220 200x18)
```

Non-interactive elements appear without `[eN]` prefix. The `include_non_interactive` param controls whether structural elements (AXGroup, AXScrollArea) appear in the `refs` map.

### Ref Resolution (in Bridge)

```swift
class RefStore {
    private var refs: [String: ResolvedElement] = [:]
    private var snapshotTimestamp: Date?

    func resolve(_ refId: String) -> Result<CGPoint, RefError> {
        guard let element = refs[refId] else {
            return .failure(.notFound(refId))
        }
        // Compute center point of frame
        let center = CGPoint(
            x: element.frame.x + element.frame.width / 2,
            y: element.frame.y + element.frame.height / 2
        )
        return .success(center)
    }

    func update(newRefs: [String: ResolvedElement]) {
        refs = newRefs
        snapshotTimestamp = Date()
    }
}
```

---

## Section 2: Action Primitives

### New Actions

| Action | Wire Method | Parameters | CGEvent Implementation |
|--------|------------|------------|----------------------|
| **scroll** | `desktop.scroll` | `ref`/`(x,y)` + `delta_x`, `delta_y` | `scrollWheelEvent2Source` with `.pixel` units |
| **double_click** | `desktop.double_click` | `ref`/`(x,y)` + `button` | `mouseEventClickState = 2` |
| **drag** | `desktop.drag` | `start_ref`/`(x,y)` + `end_ref`/`(x,y)` + `duration_ms` | mouseDown → mouseDragged(interpolated) → mouseUp |
| **hover** | `desktop.hover` | `ref`/`(x,y)` | `CGEvent(.mouseMoved)` |
| **paste** | `desktop.paste` | `text` | `NSPasteboard.setString` + keyCombo(cmd+v) |

### Upgraded Existing Actions

| Action | Change |
|--------|--------|
| **click** | Add optional `ref` parameter (alternative to `x,y`) |
| **type_text** | Add optional `ref` parameter (focus element before typing) |

### Dual-Track Targeting Logic

Applied consistently across all actions that take a position:

```
fn resolve_target(ref_id: Option<String>, x: Option<f64>, y: Option<f64>) -> Result<CGPoint> {
    if let ref_id = ref_id {
        return ref_store.resolve(ref_id)
    }
    if let (x, y) = (x, y) {
        return Ok(CGPoint(x, y))
    }
    Err("either ref or (x,y) coordinates required")
}
```

### Rust Types (core/src/desktop/types.rs)

```rust
pub enum DesktopRequest {
    // Perception (existing)
    Screenshot { region: Option<ScreenRegion> },
    Ocr { image_base64: Option<String> },
    AxTree { app_bundle_id: Option<String> },

    // Perception (new)
    Snapshot { app_bundle_id: Option<String>, max_depth: Option<u32>,
              include_non_interactive: Option<bool> },

    // Action (existing, upgraded with ref support)
    Click { ref_id: Option<String>, x: Option<f64>, y: Option<f64>, button: MouseButton },
    TypeText { ref_id: Option<String>, text: String },
    KeyCombo { keys: Vec<String> },
    LaunchApp { bundle_id: String },
    WindowList,
    FocusWindow { window_id: u32 },

    // Action (new)
    Scroll { ref_id: Option<String>, x: Option<f64>, y: Option<f64>,
             delta_x: f64, delta_y: f64 },
    DoubleClick { ref_id: Option<String>, x: Option<f64>, y: Option<f64>,
                  button: MouseButton },
    Drag { start_ref: Option<String>, start_x: Option<f64>, start_y: Option<f64>,
           end_ref: Option<String>, end_x: Option<f64>, end_y: Option<f64>,
           duration_ms: Option<u64> },
    Hover { ref_id: Option<String>, x: Option<f64>, y: Option<f64> },
    Paste { text: String },

    // Canvas (unchanged)
    CanvasShow { html: String, position: CanvasPosition },
    CanvasHide,
    CanvasUpdate { patch: serde_json::Value },

    // Internal
    Ping,
}
```

---

## Section 3: Cross-Platform Trait Abstraction

### Core Types (platform-agnostic)

```rust
// core/src/desktop/traits.rs

/// Element reference returned by snapshot, used by actions.
pub type RefId = String;

/// A resolved element with its screen coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedElement {
    pub ref_id: RefId,
    pub role: String,
    pub label: Option<String>,
    pub frame: ScreenRegion,
}

/// UI snapshot result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UISnapshot {
    pub app_bundle_id: Option<String>,
    pub app_name: String,
    pub tree: String,
    pub refs: HashMap<RefId, ResolvedElement>,
    pub interactive: Vec<RefId>,
    pub stats: SnapshotStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotStats {
    pub total_elements: u32,
    pub interactive: u32,
    pub max_depth: u32,
}

/// Action target: either a ref or absolute coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionTarget {
    Ref { ref_id: RefId },
    Coords { x: f64, y: f64 },
}
```

### Platform Implementation Points

| Platform | AX Source | Action Engine | Future Work |
|----------|-----------|---------------|-------------|
| **macOS** | `AXUIElement` (Carbon) | `CGEvent` | This design |
| **Linux** | `AT-SPI` (D-Bus) | `xdotool`/`ydotool` | Phase 7 |
| **Windows** | `UI Automation` (COM) | `SendInput` | Phase 7 |

All platforms share the same JSON-RPC protocol and `UISnapshot` response format. Only the underlying AX data source and event generation differ.

---

## Section 4: AI Agent Experience

### Typical Workflow

```
Step 1: AI captures UI snapshot
  {"action": "snapshot"}
  → Returns tree + refs

Step 2: AI reads tree, identifies target
  tree: "AXApplication 'Safari'
    AXWindow 'GitHub'
      [e1] AXTextField 'Search' (200,80 400x32)
      [e3] AXButton 'New pull request' (900,80 120x30)"

  AI reasoning: "User wants to create a PR, I need to click 'New pull request'"

Step 3: AI executes action
  {"action": "click", "ref": "e3"}
  → Bridge resolves e3 → (960, 95) → executes click
```

### Updated Tool Description

```
desktop: Control the desktop — screenshot, OCR, UI automation, keyboard/mouse, canvas.

Perception:
- snapshot: Capture UI structure with element refs. Returns tree (text), refs map,
  interactive list. Use BEFORE actions to identify targets by ref.
- screenshot: Capture screen as base64 PNG. Optional region.
- ocr: Extract text from screen or image. Returns {text, lines[]}.
- ax_tree: Raw accessibility tree (for debugging). Use snapshot instead.

Actions (all support ref OR x/y targeting):
- click: Click element. {"action":"click","ref":"e3"} or {"action":"click","x":500,"y":300}
- double_click: Double-click. Same targeting as click.
- scroll: Scroll at element. delta_x/delta_y in pixels (negative=up/left).
- drag: Drag between elements. start_ref/end_ref or start/end coordinates.
- hover: Move mouse to element without clicking.
- type_text: Type text. Optional ref to focus element first.
- key_combo: Press key combination. ["cmd","c"] for Copy.
- paste: Write to clipboard and paste (Cmd+V).
- launch_app: Launch app by bundle_id.
- window_list: List open windows.
- focus_window: Bring window to front.

Canvas:
- canvas_show/canvas_hide/canvas_update: HTML overlay with A2UI patches.
```

### Error Handling

| Scenario | Error Message | Suggestion |
|----------|--------------|------------|
| Ref not found | `"ref 'e99' not found in current snapshot"` | `"Run snapshot to refresh refs"` |
| No snapshot taken | `"No snapshot available. Run snapshot first."` | — |
| AX permission denied | `"Accessibility permission required"` | `"System Settings > Privacy > Accessibility"` |
| Element off-screen | `"ref 'e5' element appears off-screen"` | `"Try scrolling first"` |
| Stale snapshot warning | `"Snapshot is 30s+ old, refs may be stale"` | Warning only, still executes |

---

## Comparison with OpenClaw

| Aspect | OpenClaw | Aleph (This Design) |
|--------|----------|-------------------|
| **Ref source** | Playwright DOM (browser only) | AX Tree (all native apps) |
| **Coverage** | Web pages only | Entire desktop |
| **Tree format** | Playwright internal format | Indented text + ref annotations |
| **Coordinate system** | Browser viewport | Full screen absolute |
| **Ref lifetime** | Invalidated on page navigation | Invalidated on next snapshot |
| **Tool integration** | Separate browser tool | Unified desktop tool |
| **Platform** | Node.js + Playwright | Rust Core + Native Bridge |

### Where Aleph Exceeds OpenClaw

1. **Scope**: Any app on the desktop, not just browser tabs
2. **Native quality**: Uses OS accessibility APIs, not browser automation
3. **Unified tool**: One `desktop` tool for everything (no separate browser/canvas/screen tools)
4. **Architecture**: Clean Brain-Limb separation per R1, type-safe Rust core

### Where OpenClaw Still Leads (Future Work)

1. **Browser DOM snapshots**: Richer than AX tree for web content (future Playwright integration)
2. **Retry policies**: Exponential backoff with jitter (add to Aleph's bridge client)
3. **Idempotency keys**: UUID-based dedup for retries (already using UUID per request)
4. **Screen recording**: MP4 capture (separate feature, not in this design)
5. **Multi-monitor**: Screen index selection (can be added to snapshot params)

---

## Implementation Phases

### Phase 1: Core Types + Snapshot (Rust + Swift)
- Add new types to `core/src/desktop/types.rs`
- Add `Snapshot` variant to `DesktopRequest`
- Implement `RefStore` + enhanced AX traversal in Swift
- Implement `desktop.snapshot` RPC handler
- Wire through `request_to_jsonrpc` and tool args

### Phase 2: Ref-Aware Actions (Swift)
- Add ref resolution to `click`, `type_text`
- Implement new actions: `scroll`, `double_click`, `drag`, `hover`, `paste`
- Update `DesktopBridgeServer` routing

### Phase 3: Tool Layer Update (Rust)
- Update `DesktopArgs` with new fields
- Update `build_request()` for new actions
- Update tool description
- Add tests

### Phase 4: Integration Testing
- End-to-end: snapshot → click(ref) → verify
- Error cases: stale ref, missing snapshot, permission denied
- Backward compatibility: existing (x,y) calls still work
