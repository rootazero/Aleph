# Desktop Computer Use Phase 1: Code Quality + Structure Completion

**Date**: 2026-04-03
**Status**: Approved
**Scope**: `desktop/shared/`, `desktop/macos/`, `src/builtin_tools/desktop/`

## Background

Comparative analysis of Aleph desktop macOS vs Claude Code computer use implementation revealed:

1. **Aleph strengths**: Native Rust (objc2), OCR with bounding boxes, multimedia (screen record, camera, audio, STT), PIM integration, cross-platform trait architecture
2. **Aleph gaps**: Missing input primitives (double_click, drag, hover, cursor_position, mouse press/release, clipboard), legacy code duplication, oversized files (action.rs 800 lines, perception.rs 815 lines)
3. **Phase 2 prerequisites**: Safety mechanisms (session lock, escape abort, app whitelist) and interaction enhancements (mouse animation, batch operations) depend on a clean, complete input primitive layer

This Phase 1 focuses on: file restructuring, extending ScreenCapability with missing primitives, and removing legacy duplication.

## Approach

**Chosen**: Approach A — Split files + extend ScreenCapability + remove legacy.

**Rejected**: Approach B (split ScreenCapability into 3 sub-traits: ScreenPerception, InputControl, WindowManager) — more architecturally pure but premature given only macOS is fully implemented. Can be reconsidered in Phase 3.

## Design

### 1. File Restructuring

#### action.rs (800 lines) -> action/ directory

```
desktop/shared/src/action/
├── mod.rs          — re-export all public functions + validate_coordinate()
├── input.rs        — mouse/keyboard input (enigo-based, cross-platform)
│                     click, double_click, drag, hover, mouse_button,
│                     type_text, key_combo, scroll, cursor_position
│                     + helper fns: new_enigo(), to_enigo_button()
├── key_parse.rs    — parse_modifier, parse_key (pure functions + all tests)
├── window.rs       — window_list, focus_window (macOS CGWindowList + Linux wmctrl + Windows stub)
└── app_launch.rs   — launch_app, quit_app (per-platform cfg implementations)
```

Split principle: by responsibility domain, each file 100-250 lines.

#### perception.rs (815 lines) -> perception/ directory

```
desktop/shared/src/perception/
├── mod.rs           — re-export + perform_ocr() dispatch
├── screenshot.rs    — take_screenshot, capture_screen_png (xcap-based)
├── ocr_macos.rs     — macos_ocr, png_dimensions (#[cfg(target_os = "macos")])
├── ocr_windows.rs   — windows_ocr (#[cfg(target_os = "windows")])
└── screen_record.rs — SCRecordingDelegate + sc_recording_output_record
                       + screencapture_cli_record + helpers
                       (#[cfg(target_os = "macos")])
```

#### Backward compatibility

All existing call paths (`action::click()`, `perception::take_screenshot()`) preserved via `mod.rs` re-exports. No changes needed in NativeScreen or any consumer code.

### 2. ScreenCapability Trait Extension

Add 8 new methods to `ScreenCapability`, all with default implementations returning `NotImplemented`:

```rust
// New methods added to ScreenCapability trait
async fn double_click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;
async fn drag(&self, start_x: f64, start_y: f64, end_x: f64, end_y: f64, duration_ms: Option<u64>) -> Result<()>;
async fn hover(&self, x: f64, y: f64) -> Result<()>;
async fn cursor_position(&self) -> Result<(f64, f64)>;
async fn mouse_button(&self, x: f64, y: f64, button: MouseButton, action: PressAction) -> Result<()>;
async fn quit_app(&self, app_name: &str) -> Result<()>;
async fn clipboard_read(&self) -> Result<String>;
async fn clipboard_write(&self, text: &str) -> Result<()>;
```

New type:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PressAction {
    Press,    // hold down
    Release,  // release
    Click,    // press + release (existing behavior)
}
```

### 3. Implementation Details (action/input.rs)

**double_click**: enigo `move_mouse` + two `button(Click)`. May need CGEvent fallback if apps don't recognize two rapid clicks.

**drag**: Move to start -> `button(Press)` -> interpolated `move_mouse` steps -> `button(Release)`. With `duration_ms`: ease-out-cubic easing at 60fps cap. Without: instant move.

```
ease-out-cubic: t' = 1 - (1 - t)^3
```

**hover**: `move_mouse` only.

**cursor_position**: `enigo.location()` -> `(f64, f64)`.

**mouse_button**: `move_mouse` + `button(Press|Release|Click)` based on PressAction.

**quit_app**: macOS `NSRunningApplication.terminate()`, Linux `pkill`, Windows stub.

**clipboard_read/write**: macOS `NSPasteboard`, Linux `xclip`/`xsel`, Windows stub or `clipboard-win`.

**Helper extraction**: `new_enigo()` and `to_enigo_button()` eliminate repeated Enigo creation and button conversion across all input functions.

### 4. Legacy Cleanup

#### Delete

| Item | Location | Lines | Reason |
|------|----------|-------|--------|
| `DesktopCapability` trait | lib.rs:133-180 | 48 | Duplicated by `ScreenCapability` |
| `NativeDesktop` struct + impl | lib.rs:182-281 | 100 | Duplicated by `NativeScreen` |
| `NativeDesktop` tests | lib.rs:283-368 | 86 | Covered by NativeScreen tests |
| `DesktopArgs.ref_id` | types.rs:102 | - | Legacy ref-based targeting |
| `DesktopArgs.start_ref` | types.rs:106 | - | Legacy ref-based drag |
| `DesktopArgs.end_ref` | types.rs:118 | - | Legacy ref-based drag |
| `DesktopArgs.html` | types.rs:89 | - | Legacy canvas_show |
| `DesktopArgs.position` | types.rs:93 | - | Legacy canvas_show |
| `DesktopArgs.patch` | types.rs:97 | - | Legacy canvas_update |
| `DesktopArgs.app_bundle_id` | types.rs:57 | - | Legacy ax_tree (bundle_id suffices) |
| `DesktopArgs.max_depth` | types.rs:143 | - | Legacy snapshot |
| `DesktopArgs.include_non_interactive` | types.rs:147 | - | Legacy snapshot |
| `CanvasPosition` struct | types.rs:26-33 | 8 | Only used by deleted canvas fields |

#### Simplify

- `DesktopTool`: Remove `native: Option<Arc<dyn DesktopCapability>>` field and `call_native()` method. Single dispatch path via `platform`.
- `DesktopTool::call()`: Extract `classify_approval()` as standalone function. Remove legacy action special messages.
- `unsupported_action_output()`: Simplify to single generic message for any unknown action.

#### Update references

All code referencing `DesktopCapability` / `NativeDesktop` must be updated to use `ScreenCapability` / `NativeScreen`. Primary impact: `DesktopTool` construction in server startup code.

### 5. DesktopTool Interface Update

#### New actions

| Action | Parameters | Approval | Description |
|--------|-----------|----------|-------------|
| `double_click` | x, y, button? | Required | Double-click at coordinates |
| `drag` | start_x, start_y, end_x, end_y, duration_ms? | Required | Drag operation |
| `hover` | x, y | Required | Move mouse without clicking |
| `cursor_position` | (none) | Skip | Query cursor position (read-only) |
| `mouse_button` | x, y, button?, press_action | Required | Press/release separation |
| `quit_app` | bundle_id | Required | Close application |
| `clipboard_read` | (none) | Skip | Read clipboard (read-only) |
| `clipboard_write` | text | Required | Write to clipboard |

#### New DesktopArgs field

```rust
pub press_action: Option<PressAction>,
```

All other parameters reuse existing fields (x/y, start_x/start_y, end_x/end_y, button, text, bundle_id, duration_ms).

#### Approval classification

```rust
fn classify_approval(args: &DesktopArgs) -> Option<(ActionType, String)> {
    match args.action.as_str() {
        // Read-only — skip approval
        "screenshot" | "ocr" | "window_list" | "cursor_position"
        | "clipboard_read" | "screen_record" => None,
        // Click-type
        "click" | "double_click" | "hover" | "mouse_button" => Some((
            ActionType::DesktopClick,
            format!("{}({},{})", args.action, args.x.unwrap_or(0.0), args.y.unwrap_or(0.0)),
        )),
        "drag" => Some((ActionType::DesktopClick, "drag".into())),
        "scroll" => Some((ActionType::DesktopClick, "scroll".into())),
        // Type-type
        "type_text" | "clipboard_write" => Some((
            ActionType::DesktopType,
            args.text.clone().unwrap_or_default(),
        )),
        "key_combo" => Some((
            ActionType::DesktopKeyCombo,
            args.keys.as_ref().map(|k| k.join("+")).unwrap_or_default(),
        )),
        // App management
        "launch_app" | "quit_app" => Some((
            ActionType::DesktopLaunchApp,
            args.bundle_id.clone().unwrap_or_default(),
        )),
        "focus_window" => None,
        _ => Some((ActionType::DesktopClick, format!("unknown: {}", args.action))),
    }
}
```

### 6. Testing Strategy

- **Pure function tests** (CI-safe): key_parse, coordinate validation, to_enigo_button, classify_approval, PressAction serde
- **Input operation tests** (require desktop): marked `#[ignore]`, manual local verification
- **NativeScreen trait tests**: Existing screenshot/OCR tests preserved, new method tests also `#[ignore]`
- **Regression**: `cargo test -p aleph-desktop --lib` must pass with zero failures after refactoring

## Non-Goals (Phase 2)

These are explicitly deferred to Phase 2:

- Session locking (file-based lock preventing concurrent computer use)
- Escape hotkey abort (global escape key to abort computer use)
- Application whitelist/approval dialog
- Display preparation (hide non-relevant apps before screenshot)
- Batch operations (computer_batch tool)
- Multi-display auto-targeting
- Screenshot optimization (JPEG compression, API size limits)
- Clipboard paste-via-clipboard pattern for multiline text

## Risk Assessment

| Risk | Mitigation |
|------|-----------|
| double_click not recognized by some apps | CGEvent fallback available, test with common apps first |
| Removing legacy breaks server startup | Grep all DesktopCapability/NativeDesktop references before deletion |
| enigo clipboard may not work on all platforms | Platform-specific implementations, NotImplemented fallback |
| File restructuring breaks imports | mod.rs re-exports maintain all existing call paths |
