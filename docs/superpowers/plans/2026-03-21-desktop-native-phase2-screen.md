# Desktop Native Phase 2: Screen Control Native Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate screen control (screenshot, OCR, click, type, scroll, window/app management) from the legacy `NativeDesktop` + Tauri bridge dual-path to the new `DesktopPlatform` / `ScreenCapability` architecture, removing the legacy path.

**Architecture:** A shared `NativeScreen` struct in `crates/desktop/` implements `ScreenCapability` by wrapping the existing `perception` and `action` module functions. Each platform crate stores a `NativeScreen` instance and returns it from `DesktopPlatform::screen()`. The `DesktopTool` in core is rewired to dispatch screen operations through `platform.screen()` instead of the legacy `NativeDesktop`. Non-screen operations (canvas, snapshot, ax_tree, ref-based actions) continue to use the IPC bridge.

**Tech Stack:** Rust (async-trait, tokio, xcap, enigo)

**Spec:** `docs/superpowers/specs/2026-03-21-desktop-native-capabilities-design.md` — Phase 2

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `crates/desktop/src/native_screen.rs` | `NativeScreen`: shared `ScreenCapability` impl wrapping `perception` + `action` |

### Modified Files

| File | Change |
|------|--------|
| `crates/desktop/src/lib.rs` | Add `pub mod native_screen;` and re-export |
| `crates/desktop-macos/src/lib.rs` | Store `NativeScreen`, return from `screen()` |
| `crates/desktop-linux/src/lib.rs` | Store `NativeScreen`, return from `screen()` |
| `crates/desktop-windows/src/lib.rs` | Store `NativeScreen`, return from `screen()` |
| `src/builtin_tools/desktop/mod.rs` | Add `platform` field, `with_platform()` builder |
| `src/builtin_tools/desktop/native.rs` | Rewrite to dispatch via `platform.screen()` first |
| `src/executor/builtin_registry/builder.rs` | Pass platform to DesktopTool, remove NativeDesktop |

---

## Task 1: Create NativeScreen (shared ScreenCapability impl)

**Files:**
- Create: `crates/desktop/src/native_screen.rs`
- Modify: `crates/desktop/src/lib.rs`

The existing `NativeDesktop` struct implements the legacy `DesktopCapability` trait by wrapping `perception::*` and `action::*` functions with `tokio::task::spawn_blocking`. `NativeScreen` does the same for the new `ScreenCapability` trait.

- [ ] **Step 1: Create `crates/desktop/src/native_screen.rs`**

```rust
//! Shared native screen capability implementation.
//!
//! Wraps the synchronous `perception` and `action` module functions in
//! async wrappers via `tokio::task::spawn_blocking`. Used by all three
//! platform crates (macOS, Linux, Windows).

use async_trait::async_trait;

use crate::traits::ScreenCapability;
use crate::{DesktopError, MouseButton, OcrResult, Result, ScreenRegion, Screenshot, WindowInfo};
use crate::{action, perception};

/// Native screen capability using `xcap` for capture and `enigo` for input.
///
/// This struct is platform-agnostic — the underlying `perception` and `action`
/// modules handle platform differences via `cfg(target_os)` internally.
pub struct NativeScreen {
    _private: (),
}

impl NativeScreen {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for NativeScreen {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ScreenCapability for NativeScreen {
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        tokio::task::spawn_blocking(move || perception::take_screenshot(region.as_ref()))
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
    }

    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult> {
        let png_bytes = match image_png {
            Some(bytes) => bytes.to_vec(),
            None => {
                tokio::task::spawn_blocking(perception::capture_screen_png)
                    .await
                    .map_err(|e| DesktopError::OcrFailed(format!("task join error: {e}")))??
            }
        };
        tokio::task::spawn_blocking(move || perception::perform_ocr(&png_bytes))
            .await
            .map_err(|e| DesktopError::OcrFailed(format!("task join error: {e}")))?
    }

    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        tokio::task::spawn_blocking(move || action::click(x, y, button))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        let text = text.to_string();
        tokio::task::spawn_blocking(move || action::type_text(&text))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()> {
        let modifiers = modifiers.to_vec();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || action::key_combo(&modifiers, &key))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn scroll(&self, direction: &str, amount: i32) -> Result<()> {
        let direction = direction.to_string();
        tokio::task::spawn_blocking(move || action::scroll(&direction, amount))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }

    async fn window_list(&self) -> Result<Vec<WindowInfo>> {
        tokio::task::spawn_blocking(action::window_list)
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("task join error: {e}")))?
    }

    async fn focus_window(&self, window_id: u64) -> Result<()> {
        tokio::task::spawn_blocking(move || action::focus_window(window_id))
            .await
            .map_err(|e| DesktopError::WindowFailed(format!("task join error: {e}")))?
    }

    async fn launch_app(&self, app_name: &str) -> Result<()> {
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || action::launch_app(&app_name))
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_screen_creation() {
        let _screen = NativeScreen::new();
        let _screen2 = NativeScreen::default();
    }

    #[tokio::test]
    async fn test_screenshot_returns_correct_types() {
        let screen = NativeScreen::new();
        match screen.screenshot(None).await {
            Ok(screenshot) => {
                assert!(!screenshot.image_base64.is_empty());
                assert!(screenshot.width > 0);
                assert!(screenshot.height > 0);
                assert_eq!(screenshot.format, "png");
            }
            Err(DesktopError::ScreenCapture(_)) => {
                // No display (CI) — acceptable.
            }
            Err(other) => panic!("Unexpected error: {other:?}"),
        }
    }
}
```

- [ ] **Step 2: Add module to `crates/desktop/src/lib.rs`**

Add `pub mod native_screen;` after the existing module declarations, and add re-export:
```rust
pub use native_screen::NativeScreen;
```

- [ ] **Step 3: Verify compilation and run tests**

Run: `cargo check -p aleph-desktop && cargo test -p aleph-desktop --lib native_screen`
Expected: compiles, tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/native_screen.rs crates/desktop/src/lib.rs
git commit -m "desktop: add NativeScreen shared ScreenCapability implementation"
```

---

## Task 2: Wire NativeScreen into Platform Crates

**Files:**
- Modify: `crates/desktop-macos/src/lib.rs`
- Modify: `crates/desktop-macos/Cargo.toml` (add tokio dep if missing)
- Modify: `crates/desktop-linux/src/lib.rs`
- Modify: `crates/desktop-linux/Cargo.toml` (add tokio dep)
- Modify: `crates/desktop-windows/src/lib.rs`
- Modify: `crates/desktop-windows/Cargo.toml` (add tokio dep)

- [ ] **Step 1: Update `crates/desktop-macos/src/lib.rs`**

Replace the full file with:

```rust
//! macOS desktop platform — full native implementation.
//!
//! Screen capability uses the shared `NativeScreen` from `aleph-desktop`.
//! PIM, System, and Automation will be added in Phase 3.

use aleph_desktop::native_screen::NativeScreen;
use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// macOS desktop platform with full native capabilities.
pub struct MacOSPlatform {
    screen: NativeScreen,
}

impl MacOSPlatform {
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
        }
    }
}

impl Default for MacOSPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for MacOSPlatform {
    fn screen(&self) -> Option<&dyn ScreenCapability> {
        Some(&self.screen)
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        None // Phase 3
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        None // Phase 3
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        None // Phase 3
    }

    fn platform_name(&self) -> &str {
        "macOS"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macos_platform_creation() {
        let platform = MacOSPlatform::new();
        assert_eq!(platform.platform_name(), "macOS");
    }

    #[test]
    fn test_screen_capability_available() {
        let platform = MacOSPlatform::default();
        assert!(platform.screen().is_some(), "screen capability should be available");
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
```

- [ ] **Step 2: Update `crates/desktop-linux/src/lib.rs`**

Same pattern but with `LinuxPlatform` and `platform_name() = "Linux"`.

- [ ] **Step 3: Update `crates/desktop-windows/src/lib.rs`**

Same pattern but with `WindowsPlatform` and `platform_name() = "Windows"`.

- [ ] **Step 4: Update Cargo.toml for linux and windows crates**

Add `tokio` dependency to both `crates/desktop-linux/Cargo.toml` and `crates/desktop-windows/Cargo.toml` since `NativeScreen` requires `tokio::task::spawn_blocking` at runtime:

```toml
tokio = { version = "1", features = ["rt"] }
```

(macOS crate already has tokio.)

- [ ] **Step 5: Verify all platform crates compile and tests pass**

Run: `cargo check -p aleph-desktop-macos && cargo check -p aleph-desktop-linux && cargo test -p aleph-desktop-macos --lib && cargo test -p aleph-desktop-linux --lib`
Expected: compiles, tests pass (screen().is_some() now)

- [ ] **Step 6: Commit**

```bash
git add crates/desktop-macos/ crates/desktop-linux/ crates/desktop-windows/
git commit -m "desktop: wire NativeScreen into all platform crates"
```

---

## Task 3: Rewire DesktopTool to Use DesktopPlatform

**Files:**
- Modify: `src/builtin_tools/desktop/mod.rs`
- Modify: `src/builtin_tools/desktop/native.rs`

This is the core integration task. The DesktopTool currently has a dual-path architecture:
1. Try `self.native` (legacy `NativeDesktop` via `DesktopCapability` trait)
2. Fall back to `self.client` (IPC bridge)

We change it to:
1. Try `self.platform.screen()` (new `ScreenCapability` via `DesktopPlatform`)
2. Fall back to `self.client` (IPC bridge — for canvas, snapshot, ax_tree, ref-based actions only)

- [ ] **Step 1: Add platform field to DesktopTool**

In `src/builtin_tools/desktop/mod.rs`, modify the struct (around line 26):

```rust
pub struct DesktopTool {
    pub(super) client: DesktopBridgeClient,
    pub(super) approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    pub(super) native: Option<Arc<dyn aleph_desktop::DesktopCapability>>,  // legacy, kept temporarily
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,  // new
}
```

Update `new()` to initialize `platform: None`.

Add builder method:
```rust
pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
    self.platform = Some(platform);
    self
}
```

- [ ] **Step 2: Rewrite `call()` dispatch in mod.rs**

In the `call()` method, change the native dispatch block (lines 241-245) to prefer platform:

```rust
// Prefer DesktopPlatform.screen() for screen operations.
// Non-screen operations (canvas_*, snapshot, ax_tree, ref-based) fall through to IPC.
if let Some(ref platform) = self.platform {
    if let Some(output) = self.call_via_platform(platform, &args).await? {
        return Ok(output);
    }
}

// Legacy fallback: try NativeDesktop if platform didn't handle it
if let Some(ref native) = self.native {
    if let Some(output) = self.call_native(native, &args).await? {
        return Ok(output);
    }
}
```

- [ ] **Step 3: Implement `call_via_platform()` in native.rs**

Add a new method to `DesktopTool` in `native.rs` that dispatches screen operations through `platform.screen()`. This method handles the same actions as `call_native()` but uses `ScreenCapability` instead of `DesktopCapability`.

The method should:
- Check `platform.screen()` — if None, return `Ok(None)` to fall through
- Handle: screenshot, ocr, click (x/y only), type_text (no ref), key_combo, scroll, window_list, focus_window, launch_app
- Return `Ok(None)` for: snapshot, ax_tree, canvas_*, ref-based actions, double_click, drag, hover, paste
- Convert `DesktopArgs` fields to trait method parameters (same conversion as existing `call_native()`)
- Return `Ok(Some(DesktopOutput))` on success/error

The implementation mirrors the existing `call_native()` method but calls `screen.screenshot()` instead of `native.screenshot()`, etc. The type conversions (ScreenRegion f64→u32, MouseButton mapping) are identical.

- [ ] **Step 4: Verify core compiles**

Run: `cargo check -p alephcore`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/desktop/mod.rs src/builtin_tools/desktop/native.rs
git commit -m "core: rewire DesktopTool to dispatch via DesktopPlatform.screen()"
```

---

## Task 4: Update Builder and Remove Legacy NativeDesktop

**Files:**
- Modify: `src/executor/builtin_registry/builder.rs`

- [ ] **Step 1: Rewire builder to pass platform to DesktopTool**

In `builder.rs`, the current desktop_tool construction is:
```rust
let desktop_tool = {
    let native = std::sync::Arc::new(aleph_desktop::NativeDesktop::new());
    DesktopTool::new().with_native(native)
};
```

Change to:
```rust
let desktop_tool = DesktopTool::new()
    .with_platform(Arc::clone(&desktop_platform));
```

This removes `NativeDesktop` entirely. Screen operations now go through `desktop_platform.screen()` which returns `NativeScreen` (same underlying implementation, new trait path).

The IPC bridge client (`DesktopBridgeClient`) is still created inside `DesktopTool::new()` for canvas/snapshot/ax_tree fallback.

- [ ] **Step 2: Verify core compiles and all tests pass**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib`
Expected: compiles, all tests pass

- [ ] **Step 3: Commit**

```bash
git add src/executor/builtin_registry/builder.rs
git commit -m "core: remove legacy NativeDesktop, use DesktopPlatform for screen control"
```

---

## Task 5: Integration Verification

- [ ] **Step 1: Full compile check**

Run: `cargo check -p alephcore`
Expected: no errors

- [ ] **Step 2: Run all desktop crate tests**

Run: `cargo test -p aleph-desktop --lib`
Expected: all pass (existing + new NativeScreen tests)

- [ ] **Step 3: Run all platform crate tests**

Run: `cargo test -p aleph-desktop-macos --lib`
Expected: all pass (screen capability now returns Some)

- [ ] **Step 4: Run core tests for regressions**

Run: `cargo test -p alephcore --lib`
Expected: same results as before (8303+ tests, 0 failures)

- [ ] **Step 5: Verify DesktopTool tests still work**

Run: `cargo test -p alephcore --lib desktop`
Expected: existing desktop tool tests pass (they test args parsing, approval policy, etc.)

- [ ] **Step 6: Commit if fixes were needed**

```bash
git commit -m "desktop: Phase 2 screen control native — integration verified"
```
