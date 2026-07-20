# Desktop Computer Use Phase 2: Safety + Interaction Enhancements

**Date**: 2026-04-03
**Status**: Approved
**Scope**: `desktop/shared/`, `desktop/macos/`, `src/builtin_tools/desktop/`
**Prerequisite**: Phase 1 complete (file restructuring, input primitives, legacy cleanup)

## Background

Phase 1 established a clean code structure and complete input primitives. Phase 2 adds safety mechanisms and interaction enhancements learned from Claude Code's computer use implementation, adapted to Aleph's lightweight, self-hosted philosophy.

### Safety Philosophy

Aleph uses **lightweight protection** (option A from brainstorming):
- Session lock + Escape abort + existing ApprovalPolicy
- No application whitelist UI or per-session approval dialogs
- No display preparation (hiding apps) — `focus_window` from Phase 1 is sufficient
- Trust that the user's Aleph instance is self-hosted and personal

## Features

### 1. Session Lock (ComputerUseLock)

Prevents multiple agent sessions from simultaneously controlling the desktop.

**Lock file**: `~/.aleph/data/computer-use.lock`

**Lock file content** (JSON):
```json
{
  "session_id": "abc123",
  "pid": 12345,
  "acquired_at": "2026-04-03T10:30:00Z"
}
```

**Lifecycle**:
- Acquired on first mutating action in a turn
- Re-entrant within same session (check `is_held()`, skip re-acquire)
- Released on turn end or Drop
- Stale lock recovery: check PID liveness via `kill(pid, 0)`, dead process → force takeover

**API** (`src/builtin_tools/desktop/session_lock.rs`, ~100 lines):
```rust
pub struct ComputerUseLock {
    lock_path: PathBuf,
    session_id: String,
    held: bool,
}

impl ComputerUseLock {
    pub fn new(session_id: &str) -> Self;
    pub fn acquire(&mut self) -> Result<()>;
    pub fn release(&mut self) -> Result<()>;
    pub fn is_held(&self) -> bool;
}

impl Drop for ComputerUseLock {
    fn drop(&mut self) { let _ = self.release(); }
}
```

**Integration with DesktopTool**:
- New field: `session_lock: Option<ComputerUseLock>`
- New builder: `with_session_id(id: &str)` creates the lock
- `call()`: mutating actions call `acquire()` before execution
- Read-only actions (screenshot, ocr, cursor_position, clipboard_read, display_list) skip locking

### 2. Escape Abort Hotkey

Global Escape key listener to immediately abort AI desktop control.

**Platform implementation** (`desktop/macos/src/escape_listener.rs`, ~120 lines):
```rust
pub struct EscapeListener {
    abort_flag: Arc<AtomicBool>,
    active: AtomicBool,
}

impl EscapeListener {
    pub fn new() -> Self;
    pub fn start(&self) -> Result<()>;  // CGEventTap on background thread
    pub fn stop(&self);
    pub fn is_aborted(&self) -> bool;
    pub fn reset(&self);
    pub fn abort_flag(&self) -> Arc<AtomicBool>;
}
```

**macOS CGEventTap details**:
- Registers system-level keyboard event tap
- Only intercepts Escape key (keycode 53), all other keys pass through
- Requires Accessibility permission (`AXIsProcessTrusted`)
- Runs on dedicated thread via `CFRunLoop`
- Graceful degradation: if no Accessibility permission, `start()` returns Ok with warning log

**Cross-platform trait** (added to `desktop/shared/src/platform.rs`):
```rust
pub trait EscapeAbort: Send + Sync {
    fn start(&self) -> crate::Result<()>;
    fn stop(&self);
    fn is_aborted(&self) -> bool;
    fn reset(&self);
}
```

`DesktopPlatform` gets new method: `fn escape_listener(&self) -> Option<&dyn EscapeAbort> { None }`

**Integration with DesktopTool**:
- First mutating action triggers `escape_listener().start()`
- Each action checks `is_aborted()` before execution → returns "Computer use aborted by user (Escape pressed)"
- `drag()` animation loop checks abort_flag each step for early termination
- `batch` stops iteration on abort
- Turn end calls `stop()` + `reset()`
- Linux/Windows: `escape_listener()` returns `None`, no abort capability (graceful degradation)

### 3. Multi-Display Support

**New type** (`desktop/shared/src/lib.rs`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub is_primary: bool,
    pub origin_x: i32,
    pub origin_y: i32,
}
```

**ScreenCapability extension** (1 new method with default):
```rust
async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
    Err(DesktopError::NotImplemented("display_list".into()))
}
```

**Implementation**: Uses `xcap::Monitor::all()` which already supports multi-monitor enumeration.

**DesktopArgs extension**: New `display_id: Option<u32>` field. When present, `screenshot` captures that specific display instead of primary.

**New action**: `display_list` — returns all displays, read-only, skip approval.

**Screenshot with display_id**: New function `take_screenshot_display(display_id, region)` in `perception/screenshot.rs` that finds the specific monitor by ID.

### 4. Screenshot Optimization

Reduces base64 payload size by supporting JPEG format and resolution limiting.

**DesktopArgs new fields**:
```rust
pub format: Option<String>,      // "png" (default) or "jpeg"
pub quality: Option<f64>,         // 0.0-1.0, jpeg only, default 0.75
pub max_width: Option<u32>,       // scale down if wider
pub max_height: Option<u32>,      // scale down if taller
```

**Implementation** (`perception/screenshot.rs`, new function ~60 lines):
```rust
pub fn process_screenshot(
    image: image::DynamicImage,
    max_width: Option<u32>,
    max_height: Option<u32>,
    format: &str,
    quality: f64,
) -> Result<Screenshot>
```

Processing pipeline:
1. Resize if exceeds max_width/max_height (maintain aspect ratio, Lanczos3 filter)
2. Encode as JPEG (with quality parameter) or PNG
3. Return base64 + dimensions + format

**Backward compatibility**: Default behavior unchanged (PNG, no resize). Optimization only activates when format/max_width/max_height parameters are provided.

**OCR unaffected**: OCR always uses raw PNG via `capture_screen_png()`.

### 5. Batch Operations

New `batch` action in DesktopTool — sequential execution of multiple desktop actions.

**DesktopArgs new field**:
```rust
pub actions: Option<Vec<serde_json::Value>>,  // only for action="batch"
```

**Execution rules**:
- Sequential execution, stop on first failure
- Check escape abort between each action
- Each sub-action goes through approval check independently
- Nested batch forbidden (returns error immediately)
- Results array tracks each action's outcome with index

**Approval**: `batch` itself classified as `ActionType::DesktopClick`.

**Response format**:
```json
{
  "success": true,
  "data": {
    "results": [
      {"index": 0, "success": true, "data": {...}},
      {"index": 1, "success": true, "data": {...}}
    ]
  }
}
```

### 6. Clipboard Smart Paste

New `paste` action — multiline text input via clipboard + Cmd+V.

**Flow**:
1. Save current clipboard (best effort)
2. Write target text to clipboard
3. Cmd+V (key_combo meta+v)
4. Wait 100ms for paste to take effect
5. Restore original clipboard (best effort, errors swallowed)

**Approval**: Classified as `ActionType::DesktopType` (same as type_text).

**Coexistence with type_text**: Both remain available. `type_text` for short text and input event triggers, `paste` for multiline/long text.

## File Map

### New files

| File | Responsibility | Lines |
|------|---------------|-------|
| `src/builtin_tools/desktop/session_lock.rs` | ComputerUseLock | ~100 |
| `desktop/macos/src/escape_listener.rs` | macOS EscapeListener (CGEventTap) | ~120 |

### Modified files

| File | Changes |
|------|---------|
| `desktop/shared/src/lib.rs` | Add `DisplayInfo` struct |
| `desktop/shared/src/traits/screen.rs` | Add `display_list()` method |
| `desktop/shared/src/platform.rs` | Add `EscapeAbort` trait, extend `DesktopPlatform` |
| `desktop/shared/src/perception/screenshot.rs` | Add `process_screenshot()`, `take_screenshot_display()`, `list_displays()` |
| `desktop/shared/src/action/mod.rs` | Re-export `list_displays` from perception |
| `desktop/shared/src/native_screen.rs` | Implement `display_list()` |
| `desktop/macos/src/lib.rs` | Wire `escape_listener()` to MacOSPlatform |
| `src/builtin_tools/desktop/mod.rs` | Add session_lock field, update call() lifecycle, update DESCRIPTION, update classify_approval() |
| `src/builtin_tools/desktop/native.rs` | Add handlers for batch, paste, display_list, screenshot optimization params |
| `src/builtin_tools/desktop/types.rs` | Add new DesktopArgs fields (display_id, format, quality, max_width, max_height, actions) |
| `src/builtin_tools/desktop/tests.rs` | Update make_args(), add session_lock and batch tests |

## New Actions Summary

| Action | Parameters | Approval | Description |
|--------|-----------|----------|-------------|
| `display_list` | (none) | Skip | List all displays |
| `batch` | actions: [...] | Required | Sequential multi-action |
| `paste` | text | Required | Clipboard + Cmd+V paste |

## Updated screenshot Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `display_id` | u32 | primary | Target display |
| `format` | string | "png" | "png" or "jpeg" |
| `quality` | f64 | 0.75 | JPEG quality 0.0-1.0 |
| `max_width` | u32 | none | Scale down if wider |
| `max_height` | u32 | none | Scale down if taller |

## Non-Goals

- Application whitelist / per-session approval dialog
- Display preparation (hiding non-relevant apps)
- Click validation (JPEG decode+crop to verify click target)
- Normalized coordinate mode (pixels only)
- Auto-unhide apps at turn end
