# Desktop Computer Use Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure desktop computer use code (split oversized files, extend ScreenCapability with missing input primitives, remove legacy duplication) to prepare for Phase 2 safety and interaction features.

**Architecture:** Split `action.rs` and `perception.rs` into sub-module directories. Add 8 new methods to `ScreenCapability` trait with default implementations. Remove `DesktopCapability`/`NativeDesktop` legacy layer (dead code — `with_native()` is never called in production). Update `DesktopTool` to support new actions and simplify dispatch.

**Tech Stack:** Rust, enigo (input automation), xcap (screen capture), objc2 (macOS native APIs), async-trait, serde/schemars

**Spec:** `docs/superpowers/specs/2026-04-03-desktop-computer-use-phase1-design.md`

---

## File Map

### New files (create)

| File | Responsibility |
|------|---------------|
| `desktop/shared/src/action/mod.rs` | Re-export + `validate_coordinate()` + `new_enigo()` + `to_enigo_button()` |
| `desktop/shared/src/action/input.rs` | Mouse/keyboard: click, double_click, drag, hover, mouse_button, type_text, key_combo, scroll, cursor_position |
| `desktop/shared/src/action/key_parse.rs` | `parse_modifier`, `parse_key` + all tests |
| `desktop/shared/src/action/window.rs` | `window_list`, `focus_window` (macOS/Linux/Windows cfg) |
| `desktop/shared/src/action/app_launch.rs` | `launch_app`, `quit_app` (per-platform cfg) |
| `desktop/shared/src/perception/mod.rs` | Re-export + `perform_ocr()` dispatch |
| `desktop/shared/src/perception/screenshot.rs` | `take_screenshot`, `capture_screen_png` |
| `desktop/shared/src/perception/ocr_macos.rs` | `macos_ocr`, `png_dimensions` |
| `desktop/shared/src/perception/ocr_windows.rs` | `windows_ocr` |
| `desktop/shared/src/perception/screen_record.rs` | SCRecordingDelegate + recording impls |

### Modified files

| File | Changes |
|------|---------|
| `desktop/shared/src/lib.rs` | Delete `DesktopCapability` trait, `NativeDesktop` struct+impl+tests. Add `PressAction` enum. Change `pub mod action;` / `pub mod perception;` to directory modules. |
| `desktop/shared/src/traits/screen.rs` | Add 8 new methods with default impls |
| `desktop/shared/src/native_screen.rs` | Implement 8 new methods delegating to `action::*` |
| `src/builtin_tools/desktop/mod.rs` | Remove `native` field + `with_native()` + `call_native()` fallback. Add new action dispatch. Simplify `unsupported_action_output()`. Update DESCRIPTION. |
| `src/builtin_tools/desktop/native.rs` | Delete `call_native()`. Rename to reflect single-path dispatch. Add new action handlers in `call_via_platform()`. |
| `src/builtin_tools/desktop/types.rs` | Remove legacy fields, add `press_action`. Delete `CanvasPosition`. |
| `src/builtin_tools/desktop/tests.rs` | Update `make_args()` to match new DesktopArgs. |

### Deleted files

| File | Reason |
|------|--------|
| `desktop/shared/src/action.rs` | Replaced by `action/` directory |
| `desktop/shared/src/perception.rs` | Replaced by `perception/` directory |

---

## Task 1: Split perception.rs into perception/ directory

**Files:**
- Delete: `desktop/shared/src/perception.rs`
- Create: `desktop/shared/src/perception/mod.rs`
- Create: `desktop/shared/src/perception/screenshot.rs`
- Create: `desktop/shared/src/perception/ocr_macos.rs`
- Create: `desktop/shared/src/perception/ocr_windows.rs`
- Create: `desktop/shared/src/perception/screen_record.rs`

This is a pure code-move refactor. No logic changes.

- [ ] **Step 1: Create the perception/ directory and mod.rs**

```rust
// desktop/shared/src/perception/mod.rs

//! Perception capabilities — screen capture, OCR, screen recording.
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

mod screenshot;

#[cfg(target_os = "macos")]
mod ocr_macos;

#[cfg(target_os = "windows")]
mod ocr_windows;

#[cfg(target_os = "macos")]
mod screen_record;

// Re-export public API (preserves all existing call paths).
pub use screenshot::{capture_screen_png, take_screenshot};

#[cfg(target_os = "macos")]
pub use screen_record::screen_record;

use crate::error::{DesktopError, Result};
use crate::OcrResult;

/// Perform OCR on raw PNG image bytes.
///
/// Dispatches to platform-specific implementations.
pub fn perform_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    #[cfg(target_os = "windows")]
    {
        ocr_windows::windows_ocr(png_bytes)
    }

    #[cfg(target_os = "macos")]
    {
        ocr_macos::macos_ocr(png_bytes)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = png_bytes;
        Err(DesktopError::NotImplemented(
            "OCR not implemented on this platform".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn test_ocr_not_implemented_on_non_windows() {
        let dummy_png = b"fake png data";
        let result = perform_ocr(dummy_png);
        assert!(result.is_err());
        match result.unwrap_err() {
            DesktopError::NotImplemented(msg) => {
                assert!(msg.contains("OCR not implemented"));
            }
            other => panic!("Expected NotImplemented, got: {other:?}"),
        }
    }

    #[test]
    fn test_take_screenshot_returns_correct_types() {
        let result = take_screenshot(None);
        match result {
            Ok(screenshot) => {
                assert!(!screenshot.image_base64.is_empty());
                assert!(screenshot.width > 0);
                assert!(screenshot.height > 0);
                assert_eq!(screenshot.format, "png");
            }
            Err(DesktopError::ScreenCapture(_)) => {}
            Err(other) => panic!("Expected Ok or ScreenCapture, got: {other:?}"),
        }
    }

    #[test]
    fn test_capture_screen_png_returns_correct_types() {
        let result = capture_screen_png();
        match result {
            Ok(bytes) => {
                assert!(bytes.len() > 8);
                assert_eq!(&bytes[..4], b"\x89PNG");
            }
            Err(DesktopError::ScreenCapture(_)) => {}
            Err(other) => panic!("Expected Ok or ScreenCapture, got: {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Create screenshot.rs**

Move `take_screenshot` (lines 27-61) and `capture_screen_png` (lines 71-92) from `perception.rs` into `desktop/shared/src/perception/screenshot.rs`:

```rust
// desktop/shared/src/perception/screenshot.rs

//! Screenshot capture via xcap.

use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;
use tracing::debug;

use crate::error::{DesktopError, Result};
use crate::{ScreenRegion, Screenshot};

/// Capture a screenshot of the primary monitor, optionally cropped to a region.
pub fn take_screenshot(region: Option<&ScreenRegion>) -> Result<Screenshot> {
    // ... exact content from perception.rs lines 27-61
}

/// Capture the primary monitor as raw PNG bytes.
pub fn capture_screen_png() -> Result<Vec<u8>> {
    // ... exact content from perception.rs lines 71-92
}
```

Copy the function bodies verbatim from the existing `perception.rs`.

- [ ] **Step 3: Create ocr_macos.rs**

Move `macos_ocr` (lines 129-216) and `png_dimensions` (lines 220-232) from `perception.rs` into `desktop/shared/src/perception/ocr_macos.rs`:

```rust
// desktop/shared/src/perception/ocr_macos.rs

//! macOS Vision framework OCR.

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, OcrLine, OcrResult};

/// Perform OCR using macOS Vision framework (VNRecognizeTextRequest).
pub(super) fn macos_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    // ... exact content from perception.rs lines 129-216
}

/// Extract width/height from PNG header (IHDR chunk).
fn png_dimensions(png_bytes: &[u8]) -> Option<(f64, f64)> {
    // ... exact content from perception.rs lines 220-232
}
```

- [ ] **Step 4: Create ocr_windows.rs**

Move `windows_ocr` (lines 604-742) from `perception.rs` into `desktop/shared/src/perception/ocr_windows.rs`:

```rust
// desktop/shared/src/perception/ocr_windows.rs

//! Windows WinRT OCR.

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, OcrLine, OcrResult};

/// Perform OCR using the Windows WinRT OcrEngine API.
pub(super) fn windows_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    // ... exact content from perception.rs lines 604-742
}
```

- [ ] **Step 5: Create screen_record.rs**

Move the entire screen recording section (lines 238-593) from `perception.rs` into `desktop/shared/src/perception/screen_record.rs`:

```rust
// desktop/shared/src/perception/screen_record.rs

//! macOS screen recording — SCRecordingOutput (15+) with screencapture CLI fallback.

use tracing::debug;

use crate::error::{DesktopError, Result};

// SCRecordingDelegate ObjC class definition
mod sc_recording_delegate { ... }
use sc_recording_delegate::{SCRecordingDelegate, SCRecordingDelegateIvars};

pub fn screen_record(...) -> Result<...> { ... }
fn screen_record_output_path() -> Result<...> { ... }
fn can_use_sc_recording_output() -> bool { ... }
fn sc_recording_output_record(...) -> Result<...> { ... }
fn screencapture_cli_record(...) -> Result<...> { ... }
```

Copy all screen recording functions and the `sc_recording_delegate` inner module verbatim.

- [ ] **Step 6: Delete perception.rs and verify**

```bash
rm desktop/shared/src/perception.rs
```

The `pub mod perception;` in `lib.rs` now resolves to the `perception/` directory automatically.

- [ ] **Step 7: Run tests to verify the refactor**

Run: `cargo test -p aleph-desktop --lib`
Expected: All existing tests pass with zero changes.

- [ ] **Step 8: Run compile check for all platforms**

Run: `cargo check -p aleph-desktop`
Expected: No errors.

- [ ] **Step 9: Commit**

```bash
git add desktop/shared/src/perception/ && git add -u desktop/shared/src/perception.rs
git commit -m "refactor(desktop): split perception.rs into perception/ sub-modules"
```

---

## Task 2: Split action.rs into action/ directory

**Files:**
- Delete: `desktop/shared/src/action.rs`
- Create: `desktop/shared/src/action/mod.rs`
- Create: `desktop/shared/src/action/input.rs`
- Create: `desktop/shared/src/action/key_parse.rs`
- Create: `desktop/shared/src/action/window.rs`
- Create: `desktop/shared/src/action/app_launch.rs`

Pure code-move refactor. No logic changes.

- [ ] **Step 1: Create action/mod.rs**

```rust
// desktop/shared/src/action/mod.rs

//! Action capabilities — mouse, keyboard, scroll, app launch, window management.
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

mod input;
mod key_parse;
mod window;
mod app_launch;

// Re-export public API (preserves all existing call paths).
pub use input::{click, type_text, key_combo, scroll};
pub use key_parse::{parse_key, parse_modifier};
pub use window::{window_list, focus_window};
pub use app_launch::launch_app;

use enigo::{Button, Enigo, Settings};

use crate::error::{DesktopError, Result};
use crate::MouseButton;

// ── Shared helpers ──────────────────────────────────────────────

/// Validate an `f64` coordinate for safe conversion to `i32`.
pub(crate) fn validate_coordinate(value: f64, name: &str) -> Result<i32> {
    if value.is_nan() {
        return Err(DesktopError::InputFailed(format!(
            "Coordinate '{name}' is NaN"
        )));
    }
    if value.is_infinite() {
        return Err(DesktopError::InputFailed(format!(
            "Coordinate '{name}' is infinite"
        )));
    }
    if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(DesktopError::InputFailed(format!(
            "Coordinate '{name}' value {value} is outside i32 range"
        )));
    }
    Ok(value as i32)
}

/// Create a new Enigo instance.
pub(crate) fn new_enigo() -> Result<Enigo> {
    Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))
}

/// Convert Aleph's MouseButton to enigo's Button.
pub(crate) fn to_enigo_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_coordinate_normal() {
        assert_eq!(validate_coordinate(100.0, "x").unwrap(), 100);
        assert_eq!(validate_coordinate(-50.5, "y").unwrap(), -50);
        assert_eq!(validate_coordinate(0.0, "x").unwrap(), 0);
    }

    #[test]
    fn test_validate_coordinate_nan() {
        let err = validate_coordinate(f64::NAN, "x").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }

    #[test]
    fn test_validate_coordinate_infinity() {
        let err = validate_coordinate(f64::INFINITY, "x").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
        let err = validate_coordinate(f64::NEG_INFINITY, "y").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }

    #[test]
    fn test_validate_coordinate_out_of_range() {
        let err = validate_coordinate(3e10, "x").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }

    #[test]
    fn test_validate_coordinate_boundary() {
        assert_eq!(validate_coordinate(f64::from(i32::MAX), "x").unwrap(), i32::MAX);
        assert_eq!(validate_coordinate(f64::from(i32::MIN), "y").unwrap(), i32::MIN);
    }

    #[test]
    fn test_to_enigo_button() {
        assert_eq!(to_enigo_button(MouseButton::Left), Button::Left);
        assert_eq!(to_enigo_button(MouseButton::Right), Button::Right);
        assert_eq!(to_enigo_button(MouseButton::Middle), Button::Middle);
    }
}
```

- [ ] **Step 2: Create input.rs**

Move `click` (lines 49-72), `type_text` (lines 80-91), `key_combo` (lines 105-140), `scroll` (lines 153-176) from `action.rs`. Refactor to use `new_enigo()`, `to_enigo_button()`, `validate_coordinate()` from mod.rs:

```rust
// desktop/shared/src/action/input.rs

//! Mouse and keyboard input automation via enigo.

use enigo::{Axis, Coordinate, Direction, Keyboard, Mouse};
use tracing::info;

use super::{new_enigo, to_enigo_button, validate_coordinate};
use crate::error::Result;
use crate::MouseButton;

pub fn click(x: f64, y: f64, button: MouseButton) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let mut enigo = new_enigo()?;
    enigo.move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    enigo.button(to_enigo_button(button), Direction::Click)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to click: {e}")))?;
    info!(x, y, button = ?button, "Click performed");
    Ok(())
}

pub fn type_text(text: &str) -> Result<()> {
    let mut enigo = new_enigo()?;
    enigo.text(text)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to type text: {e}")))?;
    info!(chars = text.chars().count(), "Text typed");
    Ok(())
}

pub fn key_combo(modifiers: &[String], key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(crate::DesktopError::InputFailed("Key cannot be empty".into()));
    }
    let main_key = super::key_parse::parse_key(key)?;
    let modifier_keys: Vec<enigo::Key> = modifiers
        .iter()
        .map(|s| super::key_parse::parse_modifier(s))
        .collect::<Result<Vec<_>>>()?;
    let mut enigo = new_enigo()?;
    for m in &modifier_keys {
        enigo.key(*m, Direction::Press)
            .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to press modifier: {e}")))?;
    }
    enigo.key(main_key, Direction::Click)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to click key: {e}")))?;
    for m in modifier_keys.iter().rev() {
        enigo.key(*m, Direction::Release)
            .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to release modifier: {e}")))?;
    }
    info!(modifiers = ?modifiers, key = %key, "Key combo performed");
    Ok(())
}

pub fn scroll(direction: &str, amount: i32) -> Result<()> {
    let (axis, length) = match direction {
        "down" => (Axis::Vertical, amount),
        "up" => (Axis::Vertical, -amount),
        "right" => (Axis::Horizontal, amount),
        "left" => (Axis::Horizontal, -amount),
        other => {
            return Err(crate::DesktopError::InputFailed(format!(
                "Unknown scroll direction: '{other}'. Expected up, down, left, or right"
            )));
        }
    };
    let mut enigo = new_enigo()?;
    enigo.scroll(length, axis)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to scroll: {e}")))?;
    info!(direction, amount, "Scroll performed");
    Ok(())
}
```

- [ ] **Step 3: Create key_parse.rs**

Move `parse_modifier` (lines 536-547) and `parse_key` (lines 557-596) plus all their tests (lines 600-799) from `action.rs`:

```rust
// desktop/shared/src/action/key_parse.rs

//! Key name parsing — modifier and key string to enigo Key conversion.

use enigo::Key;

use crate::error::{DesktopError, Result};

pub fn parse_modifier(name: &str) -> Result<Key> {
    // ... exact content from action.rs lines 537-547
}

pub fn parse_key(name: &str) -> Result<Key> {
    // ... exact content from action.rs lines 558-596
}

#[cfg(test)]
mod tests {
    use super::*;

    // All tests from action.rs lines 606-799:
    // test_parse_modifier_meta, test_parse_modifier_shift, etc.
    // test_parse_key_single_char, test_parse_key_return, etc.
    // test_parse_key_multibyte_unicode_not_single_char, etc.
}
```

Copy all parse_modifier and parse_key tests verbatim.

- [ ] **Step 4: Create window.rs**

Move `window_list` (lines 283-307), `focus_window` (lines 318-344), and all macOS/Linux helpers (lines 348-521) from `action.rs`:

```rust
// desktop/shared/src/action/window.rs

//! Window management — list and focus windows.

use tracing::info;

use crate::error::{DesktopError, Result};
use crate::WindowInfo;

pub fn window_list() -> Result<Vec<WindowInfo>> {
    // ... exact content from action.rs lines 283-307
}

pub fn focus_window(window_id: u64) -> Result<()> {
    // ... exact content from action.rs lines 318-344
}

// macOS helpers (lines 348-447)
#[cfg(target_os = "macos")]
fn macos_window_list() -> Result<Vec<WindowInfo>> { ... }

#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u64) -> Result<()> { ... }

// Linux helpers (lines 451-521)
#[cfg(target_os = "linux")]
fn linux_window_list() -> Result<Vec<WindowInfo>> { ... }

#[cfg(target_os = "linux")]
fn linux_focus_window(window_id: u64) -> Result<()> { ... }
```

- [ ] **Step 5: Create app_launch.rs**

Move `launch_app` (lines 190-269) from `action.rs`:

```rust
// desktop/shared/src/action/app_launch.rs

//! Application launch and quit.

use tracing::info;

use crate::error::{DesktopError, Result};

pub fn launch_app(app_name: &str) -> Result<()> {
    // ... exact content from action.rs lines 190-269
}
```

`quit_app` will be added in Task 4.

- [ ] **Step 6: Delete action.rs and verify**

```bash
rm desktop/shared/src/action.rs
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p aleph-desktop --lib`
Expected: All tests pass.

- [ ] **Step 8: Run compile check**

Run: `cargo check -p aleph-desktop`
Expected: No errors.

- [ ] **Step 9: Commit**

```bash
git add desktop/shared/src/action/ && git add -u desktop/shared/src/action.rs
git commit -m "refactor(desktop): split action.rs into action/ sub-modules"
```

---

## Task 3: Add PressAction type and extend ScreenCapability trait

**Files:**
- Modify: `desktop/shared/src/lib.rs` — add `PressAction` enum
- Modify: `desktop/shared/src/traits/screen.rs` — add 8 new methods

- [ ] **Step 1: Add PressAction enum to lib.rs**

Add after the `MouseButton` enum (after line 118 in `desktop/shared/src/lib.rs`):

```rust
/// Mouse/keyboard press action type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PressAction {
    /// Hold button/key down without releasing.
    Press,
    /// Release a previously pressed button/key.
    Release,
    /// Press and immediately release (standard click).
    Click,
}
```

Update the re-exports at the top of `lib.rs` — in the types section, no new re-export needed since `PressAction` is defined directly in `lib.rs`.

- [ ] **Step 2: Extend ScreenCapability trait**

Replace `desktop/shared/src/traits/screen.rs` with:

```rust
//! Screen perception and input automation capability.

use async_trait::async_trait;

use crate::screen_types::{ScreenRecordConfig, ScreenRecordResult};
use crate::{MouseButton, OcrResult, PressAction, Result, ScreenRegion, Screenshot, WindowInfo};

/// Screen perception and input automation.
#[async_trait]
pub trait ScreenCapability: Send + Sync {
    // ── Existing methods (unchanged) ────────────────────────────

    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;
    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult>;
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()>;
    async fn scroll(&self, direction: &str, amount: i32) -> Result<()>;
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;
    async fn focus_window(&self, window_id: u64) -> Result<()>;
    async fn launch_app(&self, app_name: &str) -> Result<()>;

    async fn screen_record(&self, config: ScreenRecordConfig) -> Result<ScreenRecordResult> {
        let _ = config;
        Err(crate::DesktopError::NotImplemented(
            "screen recording not available on this platform".into(),
        ))
    }

    // ── New methods (Phase 1) ───────────────────────────────────

    /// Double-click at the specified coordinates.
    async fn double_click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        let _ = (x, y, button);
        Err(crate::DesktopError::NotImplemented("double_click".into()))
    }

    /// Drag from (start_x, start_y) to (end_x, end_y).
    /// duration_ms controls drag animation; None for instant.
    async fn drag(
        &self,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        let _ = (start_x, start_y, end_x, end_y, duration_ms);
        Err(crate::DesktopError::NotImplemented("drag".into()))
    }

    /// Move mouse to (x, y) without clicking.
    async fn hover(&self, x: f64, y: f64) -> Result<()> {
        let _ = (x, y);
        Err(crate::DesktopError::NotImplemented("hover".into()))
    }

    /// Get current mouse cursor position.
    async fn cursor_position(&self) -> Result<(f64, f64)> {
        Err(crate::DesktopError::NotImplemented("cursor_position".into()))
    }

    /// Press/release mouse button independently.
    async fn mouse_button(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        action: PressAction,
    ) -> Result<()> {
        let _ = (x, y, button, action);
        Err(crate::DesktopError::NotImplemented("mouse_button".into()))
    }

    /// Quit/close an application by name or bundle ID.
    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let _ = app_name;
        Err(crate::DesktopError::NotImplemented("quit_app".into()))
    }

    /// Read text content from system clipboard.
    async fn clipboard_read(&self) -> Result<String> {
        Err(crate::DesktopError::NotImplemented("clipboard_read".into()))
    }

    /// Write text to system clipboard.
    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let _ = text;
        Err(crate::DesktopError::NotImplemented("clipboard_write".into()))
    }
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p aleph-desktop`
Expected: No errors (all new methods have defaults, no implementor breakage).

- [ ] **Step 4: Commit**

```bash
git add desktop/shared/src/lib.rs desktop/shared/src/traits/screen.rs
git commit -m "feat(desktop): add PressAction type and extend ScreenCapability with 8 new methods"
```

---

## Task 4: Implement new input functions in action/

**Files:**
- Modify: `desktop/shared/src/action/mod.rs` — add re-exports
- Modify: `desktop/shared/src/action/input.rs` — add 5 new functions
- Modify: `desktop/shared/src/action/app_launch.rs` — add `quit_app`

- [ ] **Step 1: Add new functions to input.rs**

Append to `desktop/shared/src/action/input.rs`:

```rust
/// Double-click at (x, y).
pub fn double_click(x: f64, y: f64, button: MouseButton) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let btn = to_enigo_button(button);
    let mut enigo = new_enigo()?;
    enigo.move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    enigo.button(btn, Direction::Click)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to click: {e}")))?;
    enigo.button(btn, Direction::Click)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to double-click: {e}")))?;
    info!(x, y, button = ?button, "Double-click performed");
    Ok(())
}

/// Drag from (start_x, start_y) to (end_x, end_y) with optional animation.
pub fn drag(
    start_x: f64,
    start_y: f64,
    end_x: f64,
    end_y: f64,
    duration_ms: Option<u64>,
) -> Result<()> {
    use std::time::Duration;

    let sx = validate_coordinate(start_x, "start_x")?;
    let sy = validate_coordinate(start_y, "start_y")?;
    let ex = validate_coordinate(end_x, "end_x")?;
    let ey = validate_coordinate(end_y, "end_y")?;

    let mut enigo = new_enigo()?;

    // Move to start, press left button
    enigo.move_mouse(sx, sy, Coordinate::Abs)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move to start: {e}")))?;
    enigo.button(enigo::Button::Left, Direction::Press)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to press for drag: {e}")))?;

    // Interpolated move or instant
    match duration_ms {
        Some(ms) if ms > 0 => {
            let steps = ((ms as f64 / 1000.0) * 60.0).ceil().max(1.0) as u64;
            let step_delay = Duration::from_millis(ms / steps.max(1));
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                // ease-out-cubic: 1 - (1 - t)^3
                let eased = 1.0 - (1.0 - t).powi(3);
                let cx = sx as f64 + (ex as f64 - sx as f64) * eased;
                let cy = sy as f64 + (ey as f64 - sy as f64) * eased;
                enigo.move_mouse(cx as i32, cy as i32, Coordinate::Abs)
                    .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move during drag: {e}")))?;
                std::thread::sleep(step_delay);
            }
        }
        _ => {
            enigo.move_mouse(ex, ey, Coordinate::Abs)
                .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move to end: {e}")))?;
        }
    }

    // Release left button
    enigo.button(enigo::Button::Left, Direction::Release)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to release after drag: {e}")))?;

    info!(start_x, start_y, end_x, end_y, ?duration_ms, "Drag performed");
    Ok(())
}

/// Move mouse to (x, y) without clicking.
pub fn hover(x: f64, y: f64) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let mut enigo = new_enigo()?;
    enigo.move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    info!(x, y, "Hover performed");
    Ok(())
}

/// Get current mouse cursor position.
pub fn cursor_position() -> Result<(f64, f64)> {
    let enigo = new_enigo()?;
    let (x, y) = enigo.location()
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to get cursor position: {e}")))?;
    Ok((x as f64, y as f64))
}

/// Press, release, or click a mouse button at (x, y).
pub fn mouse_button(
    x: f64,
    y: f64,
    button: MouseButton,
    action: crate::PressAction,
) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let btn = to_enigo_button(button);
    let dir = match action {
        crate::PressAction::Press => Direction::Press,
        crate::PressAction::Release => Direction::Release,
        crate::PressAction::Click => Direction::Click,
    };
    let mut enigo = new_enigo()?;
    enigo.move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    enigo.button(btn, dir)
        .map_err(|e| crate::DesktopError::InputFailed(format!("Failed mouse button action: {e}")))?;
    info!(x, y, button = ?button, action = ?action, "Mouse button action performed");
    Ok(())
}
```

- [ ] **Step 2: Add clipboard functions to input.rs**

Append to `desktop/shared/src/action/input.rs`:

```rust
/// Read text from system clipboard.
pub fn clipboard_read() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::NSString;

        let pb = unsafe { NSPasteboard::generalPasteboard() };
        let ns_string_type = unsafe { NSString::from_str("public.utf8-plain-text") };
        match unsafe { pb.stringForType(&ns_string_type) } {
            Some(s) => Ok(s.to_string()),
            None => Ok(String::new()),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .or_else(|_| {
                std::process::Command::new("xsel")
                    .args(["--clipboard", "--output"])
                    .output()
            })
            .map_err(|e| crate::DesktopError::InputFailed(
                format!("Failed to read clipboard (install xclip or xsel): {e}")
            ))?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    #[cfg(target_os = "windows")]
    {
        Err(crate::DesktopError::NotImplemented(
            "clipboard_read not yet implemented for Windows".into(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(crate::DesktopError::NotImplemented(
            "clipboard_read not implemented on this platform".into(),
        ))
    }
}

/// Write text to system clipboard.
pub fn clipboard_write(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::{NSArray, NSString};

        let pb = unsafe { NSPasteboard::generalPasteboard() };
        pb.clearContents();
        let ns_string = NSString::from_str(text);
        let ns_string_type = NSString::from_str("public.utf8-plain-text");
        let types = NSArray::from_retained_slice(&[ns_string_type]);
        pb.declareTypes_owner(&types, None);
        pb.setString_forType(&ns_string, &NSString::from_str("public.utf8-plain-text"));
        info!("Clipboard write performed");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .or_else(|_| {
                std::process::Command::new("xsel")
                    .args(["--clipboard", "--input"])
                    .stdin(std::process::Stdio::piped())
                    .spawn()
            })
            .map_err(|e| crate::DesktopError::InputFailed(
                format!("Failed to write clipboard (install xclip or xsel): {e}")
            ))?;
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes())
                .map_err(|e| crate::DesktopError::InputFailed(format!("Failed to write to clipboard: {e}")))?;
        }
        child.wait()
            .map_err(|e| crate::DesktopError::InputFailed(format!("Clipboard process failed: {e}")))?;
        info!("Clipboard write performed");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let _ = text;
        Err(crate::DesktopError::NotImplemented(
            "clipboard_write not yet implemented for Windows".into(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        Err(crate::DesktopError::NotImplemented(
            "clipboard_write not implemented on this platform".into(),
        ))
    }
}
```

- [ ] **Step 3: Add quit_app to app_launch.rs**

Append to `desktop/shared/src/action/app_launch.rs`:

```rust
/// Quit/close an application by name or bundle ID.
pub fn quit_app(app_name: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSRunningApplication;
        use objc2_foundation::NSString;

        let apps = unsafe { NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str(app_name),
        ) };
        if apps.count() == 0 {
            return Err(DesktopError::InputFailed(format!(
                "No running application found with identifier '{app_name}'"
            )));
        }
        for app in apps.iter() {
            app.terminate();
        }
        info!(app_name, "App quit requested (macOS)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let status = std::process::Command::new("pkill")
            .args(["-f", app_name])
            .status()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to quit app: {e}")))?;
        if !status.success() {
            return Err(DesktopError::InputFailed(format!(
                "Failed to quit '{app_name}'"
            )));
        }
        info!(app_name, "App quit requested (Linux)");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let _ = app_name;
        Err(DesktopError::NotImplemented(
            "quit_app not yet implemented for Windows".into(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app_name;
        Err(DesktopError::NotImplemented(
            "quit_app not implemented on this platform".into(),
        ))
    }
}
```

- [ ] **Step 4: Update action/mod.rs re-exports**

Add to the re-exports in `action/mod.rs`:

```rust
pub use input::{click, double_click, drag, hover, cursor_position, mouse_button,
                clipboard_read, clipboard_write, type_text, key_combo, scroll};
pub use app_launch::{launch_app, quit_app};
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-desktop`
Expected: No errors. New functions compile but are not yet called by NativeScreen.

- [ ] **Step 6: Commit**

```bash
git add desktop/shared/src/action/
git commit -m "feat(desktop): add double_click, drag, hover, cursor_position, mouse_button, clipboard, quit_app"
```

---

## Task 5: Implement new methods in NativeScreen

**Files:**
- Modify: `desktop/shared/src/native_screen.rs` — implement 8 new trait methods

- [ ] **Step 1: Add implementations to NativeScreen**

Add after the existing `screen_record` implementation in the `impl ScreenCapability for NativeScreen` block:

```rust
    async fn double_click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        tokio::task::spawn_blocking(move || action::double_click(x, y, button))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn drag(
        &self,
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        duration_ms: Option<u64>,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || action::drag(start_x, start_y, end_x, end_y, duration_ms))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn hover(&self, x: f64, y: f64) -> Result<()> {
        tokio::task::spawn_blocking(move || action::hover(x, y))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn cursor_position(&self) -> Result<(f64, f64)> {
        tokio::task::spawn_blocking(action::cursor_position)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn mouse_button(
        &self,
        x: f64,
        y: f64,
        button: MouseButton,
        press_action: PressAction,
    ) -> Result<()> {
        tokio::task::spawn_blocking(move || action::mouse_button(x, y, button, press_action))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || action::quit_app(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_read(&self) -> Result<String> {
        tokio::task::spawn_blocking(action::clipboard_read)
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || action::clipboard_write(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }
```

Also add `PressAction` to the imports at the top of `native_screen.rs`:

```rust
use crate::{
    action, perception, DesktopError, MouseButton, OcrResult, PressAction, Result,
    ScreenRegion, Screenshot, WindowInfo,
};
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p aleph-desktop`
Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add desktop/shared/src/native_screen.rs
git commit -m "feat(desktop): implement new ScreenCapability methods in NativeScreen"
```

---

## Task 6: Remove legacy DesktopCapability + NativeDesktop

**Files:**
- Modify: `desktop/shared/src/lib.rs` — delete DesktopCapability trait, NativeDesktop struct+impl+tests
- Modify: `src/builtin_tools/desktop/mod.rs` — remove `native` field, `with_native()`, legacy fallback
- Delete: `src/builtin_tools/desktop/native.rs` — merge `call_via_platform` into mod.rs or rename

- [ ] **Step 1: Clean lib.rs**

In `desktop/shared/src/lib.rs`, delete:
- Lines 133-180: The entire `DesktopCapability` trait definition
- Lines 182-281: `NativeDesktop` struct, `impl NativeDesktop`, `impl Default`, `impl DesktopCapability for NativeDesktop`
- Lines 283-368: The `#[cfg(test)] mod tests` block (all tests reference NativeDesktop)
- The `use async_trait::async_trait;` import (only used by DesktopCapability impl)

Keep: All type definitions (ScreenRegion, Screenshot, OcrResult, MouseButton, WindowInfo, etc.), module declarations, re-exports, and the new PressAction.

- [ ] **Step 2: Remove native field from DesktopTool**

In `src/builtin_tools/desktop/mod.rs`:

Remove:
- Field `pub(super) native: Option<Arc<dyn aleph_desktop::DesktopCapability>>,` (line 24)
- Method `with_native()` (lines 42-45)
- The `call_native` fallback block in `call()` (lines 260-267)
- The `unsupported_action_output` method's legacy-specific messages for "snapshot", "ax_tree", "canvas_*" (lines 134-139) — replace with a single generic message

Update `DesktopTool::new()` to not include `native`:

```rust
pub fn new() -> Self {
    Self {
        approval_policy: None,
        platform: None,
    }
}
```

Simplify `call()` to remove the native fallback:

```rust
async fn call(&self, args: Self::Args) -> Result<Self::Output> {
    // 1. Approval check
    if let Some((action_type, target)) = classify_approval(&args) {
        if let Some(out) = self.check_approval(action_type, &target).await {
            return Ok(out);
        }
    }

    // 2. Execute via platform (single path)
    if let Some(ref platform) = self.platform {
        if let Some(output) = self.call_via_platform(platform, &args).await? {
            return Ok(output);
        }
    }

    if self.platform.is_none() {
        return Ok(self.no_capability_output());
    }

    Ok(self.unsupported_action_output(&args))
}
```

- [ ] **Step 3: Remove native.rs call_native method**

In `src/builtin_tools/desktop/native.rs`, delete the entire `call_native()` method (lines 10-322). Keep `call_via_platform()` (lines 324-652).

Remove `mod native;` is not needed — the file still exists for `call_via_platform`.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore`
Expected: No errors. `with_native()` was never called in production.

- [ ] **Step 5: Run all tests**

Run: `cargo test -p alephcore --lib -- desktop`
Expected: All existing tests pass (they use `DesktopTool::new()` without `with_native`).

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "refactor(desktop): remove legacy DesktopCapability trait and NativeDesktop"
```

---

## Task 7: Update DesktopArgs and DesktopTool interface

**Files:**
- Modify: `src/builtin_tools/desktop/types.rs` — remove legacy fields, add `press_action`
- Modify: `src/builtin_tools/desktop/mod.rs` — update DESCRIPTION, extract `classify_approval()`
- Modify: `src/builtin_tools/desktop/native.rs` — add new action handlers in `call_via_platform()`
- Modify: `src/builtin_tools/desktop/tests.rs` — update `make_args()`

- [ ] **Step 1: Clean up DesktopArgs**

In `src/builtin_tools/desktop/types.rs`:

Delete `CanvasPosition` struct (lines 26-33).

Remove these fields from `DesktopArgs`:
- `app_bundle_id` (line 57)
- `html` (line 89)
- `position` (line 93)
- `patch` (line 97)
- `ref_id` (line 102)
- `start_ref` (line 106)
- `end_ref` (line 118)
- `max_depth` (line 143)
- `include_non_interactive` (line 147)

Add new field:

```rust
    /// Press action for mouse_button: "press", "release", "click".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press_action: Option<aleph_desktop::PressAction>,
```

- [ ] **Step 2: Update make_args() in tests.rs**

Update the `make_args()` helper in `src/builtin_tools/desktop/tests.rs` to match the new DesktopArgs (remove deleted fields, add `press_action: None`):

```rust
fn make_args(action: &str) -> DesktopArgs {
    DesktopArgs {
        action: action.into(),
        region: None,
        image_base64: None,
        x: None,
        y: None,
        button: None,
        text: None,
        keys: None,
        bundle_id: None,
        window_id: None,
        start_x: None,
        start_y: None,
        end_x: None,
        end_y: None,
        delta_x: None,
        delta_y: None,
        duration_ms: None,
        duration: None,
        fps: None,
        with_audio: None,
        press_action: None,
    }
}
```

- [ ] **Step 3: Update DESCRIPTION in mod.rs**

Replace the DESCRIPTION constant in `src/builtin_tools/desktop/mod.rs`:

```rust
const DESCRIPTION: &'static str = r#"Control the desktop — see the screen and interact with it.

Actions:
- screenshot: Capture screen as base64 PNG. Optional region: {x,y,width,height}
- ocr: Extract text from screen with bounding boxes. Optional image_base64.
- click: Click at (x, y). Optional button (left/right/middle).
- double_click: Double-click at (x, y). Optional button.
- drag: Drag from (start_x, start_y) to (end_x, end_y). Optional duration_ms for animation.
- hover: Move mouse to (x, y) without clicking.
- cursor_position: Get current mouse cursor position.
- mouse_button: Press/release mouse at (x, y). Requires press_action (press/release/click).
- type_text: Type text at current cursor position.
- key_combo: Press key combination, e.g. keys=["cmd","c"].
- scroll: Scroll via delta_x/delta_y.
- launch_app: Launch app by bundle_id.
- quit_app: Close app by bundle_id.
- window_list: List open windows.
- focus_window: Bring window to front by window_id.
- clipboard_read: Read clipboard text.
- clipboard_write: Write text to clipboard.
- screen_record: Record screen as MP4. Optional duration/fps/with_audio.

Examples:
{"action":"click","x":500,"y":300}
{"action":"double_click","x":500,"y":300}
{"action":"drag","start_x":100,"start_y":100,"end_x":500,"end_y":500,"duration_ms":300}
{"action":"hover","x":250,"y":250}
{"action":"cursor_position"}
{"action":"clipboard_read"}
{"action":"scroll","delta_y":-300}
{"action":"type_text","text":"Hello"}
{"action":"screen_record","duration":3.0,"fps":30}"#;
```

- [ ] **Step 4: Extract classify_approval() and update approval logic**

In `src/builtin_tools/desktop/mod.rs`, extract the approval match block into a standalone function. Add it as a free function at the bottom of the file (before tests module):

```rust
/// Classify a desktop action for approval checking.
/// Returns None for read-only actions (skip approval).
fn classify_approval(args: &DesktopArgs) -> Option<(ActionType, String)> {
    match args.action.as_str() {
        // Read-only — skip approval
        "screenshot" | "ocr" | "window_list" | "cursor_position"
        | "clipboard_read" | "screen_record" | "focus_window" => None,

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

        // Unknown — require approval for safety
        _ => Some((ActionType::DesktopClick, format!("unknown: {}", args.action))),
    }
}
```

Update `call()` to use the extracted function (replace the inline match block).

- [ ] **Step 5: Add new action handlers in call_via_platform()**

In `src/builtin_tools/desktop/native.rs`, add handlers for the 8 new actions in the `call_via_platform` match block, before the `_ => Ok(None)` arm:

```rust
            "double_click" => {
                let x = args.x.unwrap_or(0.0);
                let y = args.y.unwrap_or(0.0);
                let button = match args.button.as_ref().unwrap_or(&MouseButton::Left) {
                    MouseButton::Left => aleph_desktop::MouseButton::Left,
                    MouseButton::Right => aleph_desktop::MouseButton::Right,
                    MouseButton::Middle => aleph_desktop::MouseButton::Middle,
                };
                match screen.double_click(x, y, button).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"double_clicked": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "drag" => {
                let sx = args.start_x.unwrap_or(0.0);
                let sy = args.start_y.unwrap_or(0.0);
                let ex = args.end_x.unwrap_or(0.0);
                let ey = args.end_y.unwrap_or(0.0);
                match screen.drag(sx, sy, ex, ey, args.duration_ms).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"dragged": true})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "hover" => {
                let x = args.x.unwrap_or(0.0);
                let y = args.y.unwrap_or(0.0);
                match screen.hover(x, y).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"hovered": true, "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "cursor_position" => {
                match screen.cursor_position().await {
                    Ok((x, y)) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "mouse_button" => {
                let x = args.x.unwrap_or(0.0);
                let y = args.y.unwrap_or(0.0);
                let button = match args.button.as_ref().unwrap_or(&MouseButton::Left) {
                    MouseButton::Left => aleph_desktop::MouseButton::Left,
                    MouseButton::Right => aleph_desktop::MouseButton::Right,
                    MouseButton::Middle => aleph_desktop::MouseButton::Middle,
                };
                let press_action = args.press_action.unwrap_or(aleph_desktop::PressAction::Click);
                match screen.mouse_button(x, y, button, press_action).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"action": format!("{:?}", press_action), "x": x, "y": y})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "quit_app" => {
                let bundle_id = match args.bundle_id.as_deref() {
                    Some(id) if !id.is_empty() => id,
                    _ => {
                        return Ok(Some(DesktopOutput {
                            success: false, data: None,
                            message: Some("quit_app requires 'bundle_id'".to_string()),
                        }));
                    }
                };
                match screen.quit_app(bundle_id).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"quit": true, "bundle_id": bundle_id})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "clipboard_read" => {
                match screen.clipboard_read().await {
                    Ok(text) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"text": text})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
            "clipboard_write" => {
                let text = args.text.as_deref().unwrap_or("");
                match screen.clipboard_write(text).await {
                    Ok(()) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({"written": true})),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screen capability error: {e}")),
                    })),
                }
            }
```

Also remove `ref_id` checks from the existing `click`, `type_text`, and `scroll` handlers since the field no longer exists.

- [ ] **Step 6: Remove ref_id checks in call_via_platform**

In the `click`, `type_text`, and `scroll` handlers within `call_via_platform()`, remove the lines:

```rust
// DELETE these lines:
if args.ref_id.is_some() {
    return Ok(None);
}
```

These fields no longer exist on DesktopArgs.

- [ ] **Step 7: Simplify unsupported_action_output**

Replace the entire `unsupported_action_output` method body:

```rust
fn unsupported_action_output(&self, args: &DesktopArgs) -> DesktopOutput {
    DesktopOutput {
        success: false,
        data: None,
        message: Some(format!(
            "Desktop action '{}' is not supported on this platform.",
            args.action
        )),
    }
}
```

- [ ] **Step 8: Verify full build**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 9: Run tests**

Run: `cargo test -p alephcore --lib -- desktop`
Expected: All tests pass.

- [ ] **Step 10: Commit**

```bash
git add -u
git commit -m "feat(desktop): add new actions to DesktopTool, clean up legacy fields and dispatch"
```

---

## Task 8: Final verification and cleanup

**Files:**
- All modified files from Tasks 1-7

- [ ] **Step 1: Full build check**

Run: `cargo check`
Expected: Entire workspace compiles without errors.

- [ ] **Step 2: Run all desktop tests**

Run: `cargo test -p aleph-desktop --lib && cargo test -p alephcore --lib -- desktop`
Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p aleph-desktop -p alephcore -- -D warnings`
Expected: No warnings.

- [ ] **Step 4: Verify no dead code**

Run: `cargo check -p aleph-desktop 2>&1 | grep "unused"` and `grep -r "DesktopCapability\|NativeDesktop\|CanvasPosition\|ref_id\|start_ref\|end_ref\|app_bundle_id\|max_depth\|include_non_interactive" src/ desktop/ --include="*.rs"`

Expected: No references to deleted items remain.

- [ ] **Step 5: Commit any final cleanup**

```bash
git add -u
git commit -m "chore(desktop): final cleanup after Phase 1 restructure"
```
