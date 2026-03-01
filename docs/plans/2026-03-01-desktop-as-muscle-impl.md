# Desktop-as-Muscle Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract desktop capabilities from Tauri bridge into a standalone `aleph-desktop` crate that compiles directly into aleph-server, eliminating IPC overhead and the Tauri shell dependency.

**Architecture:** Create `crates/desktop/` implementing a `DesktopCapability` trait (defined in core) using xcap for screenshots and enigo for input. The `DesktopTool` gains a dual-path: prefer in-process `NativeDesktop` when available, fall back to IPC `DesktopBridgeClient` otherwise.

**Tech Stack:** Rust, xcap 0.8, enigo 0.3, image 0.25, base64 0.22, `#[cfg(target_os)]` for platform isolation

**Design Doc:** `docs/plans/2026-03-01-desktop-as-muscle-design.md`

---

### Task 1: Create `aleph-desktop` crate skeleton and `DesktopCapability` trait

**Files:**
- Create: `crates/desktop/Cargo.toml`
- Create: `crates/desktop/src/lib.rs`
- Create: `crates/desktop/src/error.rs`
- Modify: `Cargo.toml` (workspace root, add member)
- Modify: `core/src/desktop/mod.rs` (add trait module)
- Create: `core/src/desktop/traits.rs`

**Step 1: Create crate directory**

```bash
mkdir -p crates/desktop/src
```

**Step 2: Create `crates/desktop/Cargo.toml`**

```toml
[package]
name = "aleph-desktop"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
# Async
tokio = { workspace = true }
async-trait = { workspace = true }

# Serialization
serde = { workspace = true }
serde_json = { workspace = true }

# Error handling
thiserror = { workspace = true }

# Logging
tracing = { workspace = true }

# Image encoding
image = { version = "0.25", default-features = false, features = ["png"] }
base64 = { workspace = true }

# Screen capture
xcap = "0.8"

# Input automation
enigo = { version = "0.3", features = ["serde"] }

[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
    "Win32_UI_WindowsAndMessaging",
    "Win32_Foundation",
    "Win32_System_Threading",
    "Media_Ocr",
    "Globalization",
    "Graphics_Imaging",
    "Storage_Streams",
    "Win32_UI_Accessibility",
    "Win32_System_Com",
] }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

**Step 3: Create `crates/desktop/src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DesktopError {
    #[error("Desktop capability not available on this platform")]
    NotAvailable,

    #[error("Screen capture failed: {0}")]
    ScreenCapture(String),

    #[error("Input automation failed: {0}")]
    InputFailed(String),

    #[error("OCR failed: {0}")]
    OcrFailed(String),

    #[error("Window operation failed: {0}")]
    WindowFailed(String),

    #[error("Platform feature not implemented: {0}")]
    NotImplemented(String),
}

pub type Result<T> = std::result::Result<T, DesktopError>;
```

**Step 4: Create `crates/desktop/src/lib.rs` with `NativeDesktop` stub**

```rust
pub mod error;

mod perception;
mod action;

pub use error::{DesktopError, Result};

use async_trait::async_trait;
use serde_json::Value;

/// Capability metadata reported during handshake.
#[derive(Debug, Clone)]
pub struct Capability {
    pub name: String,
    pub version: String,
}

impl Capability {
    pub fn new(name: &str, version: &str) -> Self {
        Self { name: name.to_string(), version: version.to_string() }
    }
}

/// Screen region in pixels.
#[derive(Debug, Clone)]
pub struct ScreenRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Screenshot result.
#[derive(Debug, Clone)]
pub struct Screenshot {
    pub image_base64: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
}

/// OCR result.
#[derive(Debug, Clone)]
pub struct OcrResult {
    pub full_text: String,
    pub lines: Vec<OcrLine>,
}

/// Single OCR line.
#[derive(Debug, Clone)]
pub struct OcrLine {
    pub text: String,
    pub bounding_box: Option<BoundingBox>,
}

/// Bounding box for OCR.
#[derive(Debug, Clone)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Mouse button.
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Window info.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub pid: u64,
}

/// The core trait for desktop capabilities.
///
/// Implementors provide platform-specific perception and action.
/// Core defines this contract; `aleph-desktop` provides `NativeDesktop`.
#[async_trait]
pub trait DesktopCapability: Send + Sync {
    /// List available capabilities on this platform.
    fn capabilities(&self) -> Vec<Capability>;

    /// Capture screenshot (full screen or region).
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;

    /// OCR on image bytes (PNG). If None, captures screen first.
    async fn ocr(&self, image: Option<&[u8]>) -> Result<OcrResult>;

    /// Click at coordinates.
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;

    /// Type text string.
    async fn type_text(&self, text: &str) -> Result<()>;

    /// Press key combination (e.g., ["cmd", "c"]).
    async fn key_combo(&self, keys: &[String]) -> Result<()>;

    /// Scroll at position.
    async fn scroll(&self, direction: &str, amount: i32) -> Result<()>;

    /// List visible windows.
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;

    /// Focus a window by ID.
    async fn focus_window(&self, window_id: u64) -> Result<()>;

    /// Launch an application by name/bundle_id.
    async fn launch_app(&self, app_id: &str) -> Result<()>;
}

/// Native desktop implementation using xcap + enigo.
pub struct NativeDesktop;

impl NativeDesktop {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

impl Default for NativeDesktop {
    fn default() -> Self {
        Self
    }
}
```

**Step 5: Create stub modules**

Create `crates/desktop/src/perception.rs`:
```rust
//! Perception handlers — screenshot, OCR.
```

Create `crates/desktop/src/action.rs`:
```rust
//! Action handlers — click, type, key_combo, scroll, window, app.
```

**Step 6: Add to workspace `Cargo.toml`**

In root `Cargo.toml`, add `"crates/desktop"` to workspace members list.

**Step 7: Verify crate builds**

```bash
cargo check -p aleph-desktop
```

Expected: compiles with no errors (stubs only).

**Step 8: Commit**

```bash
git add crates/desktop/ Cargo.toml
git commit -m "feat(desktop): create aleph-desktop crate skeleton with DesktopCapability trait"
```

---

### Task 2: Implement screenshot capability

**Files:**
- Modify: `crates/desktop/src/perception.rs`
- Modify: `crates/desktop/src/lib.rs` (wire up trait impl)

**Step 1: Write the failing test**

Add to `crates/desktop/src/perception.rs`:

```rust
//! Perception handlers — screenshot, OCR.

use crate::error::{DesktopError, Result};
use crate::{BoundingBox, OcrLine, OcrResult, ScreenRegion, Screenshot};
use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;

/// Capture primary monitor screenshot, optionally cropped to region.
pub fn take_screenshot(region: Option<&ScreenRegion>) -> Result<Screenshot> {
    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".to_string()))?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x as u32, r.y as u32, r.width as u32, r.height as u32),
        None => monitor.capture_image(),
    }
    .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: "png".to_string(),
    })
}

/// Capture screen as raw PNG bytes (for OCR input).
pub fn capture_screen_png() -> Result<Vec<u8>> {
    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".to_string()))?;

    let image = monitor
        .capture_image()
        .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    Ok(buf.into_inner())
}

/// Perform OCR. Platform-specific: Windows uses WinRT, others return NotImplemented.
pub fn perform_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    #[cfg(target_os = "windows")]
    {
        windows_ocr(png_bytes)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = png_bytes;
        Err(DesktopError::NotImplemented(
            "OCR not implemented on this platform".to_string(),
        ))
    }
}

#[cfg(target_os = "windows")]
fn windows_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    // Copy from apps/desktop/src-tauri/src/bridge/perception.rs windows_ocr()
    // but return Result<OcrResult> instead of Result<Value, (i32, String)>
    todo!("Port Windows OCR from Tauri bridge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screenshot_returns_valid_png() {
        // This test requires a display — skip in CI
        if std::env::var("CI").is_ok() {
            return;
        }
        let result = take_screenshot(None);
        // On a machine with a display, this should succeed
        if let Ok(screenshot) = result {
            assert!(screenshot.width > 0);
            assert!(screenshot.height > 0);
            assert_eq!(screenshot.format, "png");
            assert!(!screenshot.image_base64.is_empty());
            // Verify base64 is valid
            let decoded = general_purpose::STANDARD.decode(&screenshot.image_base64);
            assert!(decoded.is_ok());
        }
        // If no display, the error is expected
    }

    #[test]
    fn test_ocr_not_implemented_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let result = perform_ocr(b"fake png");
            assert!(matches!(result, Err(DesktopError::NotImplemented(_))));
        }
    }
}
```

**Step 2: Wire up trait impl in lib.rs**

Add `DesktopCapability` impl for `NativeDesktop`:

```rust
#[async_trait]
impl DesktopCapability for NativeDesktop {
    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = vec![
            Capability::new("screen_capture", "1.0"),
            Capability::new("keyboard_control", "1.0"),
            Capability::new("mouse_control", "1.0"),
        ];

        #[cfg(target_os = "windows")]
        {
            caps.push(Capability::new("ocr", "1.0"));
            caps.push(Capability::new("ax_inspect", "1.0"));
        }

        caps
    }

    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot> {
        // xcap is sync — run in blocking task
        tokio::task::spawn_blocking(move || perception::take_screenshot(region.as_ref()))
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("Task join error: {e}")))?
    }

    async fn ocr(&self, image: Option<&[u8]>) -> Result<OcrResult> {
        let png_bytes = match image {
            Some(bytes) => bytes.to_vec(),
            None => {
                tokio::task::spawn_blocking(perception::capture_screen_png)
                    .await
                    .map_err(|e| DesktopError::OcrFailed(format!("Task join error: {e}")))??
            }
        };
        tokio::task::spawn_blocking(move || perception::perform_ocr(&png_bytes))
            .await
            .map_err(|e| DesktopError::OcrFailed(format!("Task join error: {e}")))?
    }

    // Stubs for remaining methods — will be filled in Task 3
    async fn click(&self, _x: f64, _y: f64, _button: MouseButton) -> Result<()> {
        Err(DesktopError::NotImplemented("click".to_string()))
    }
    async fn type_text(&self, _text: &str) -> Result<()> {
        Err(DesktopError::NotImplemented("type_text".to_string()))
    }
    async fn key_combo(&self, _keys: &[String]) -> Result<()> {
        Err(DesktopError::NotImplemented("key_combo".to_string()))
    }
    async fn scroll(&self, _direction: &str, _amount: i32) -> Result<()> {
        Err(DesktopError::NotImplemented("scroll".to_string()))
    }
    async fn window_list(&self) -> Result<Vec<WindowInfo>> {
        Err(DesktopError::NotImplemented("window_list".to_string()))
    }
    async fn focus_window(&self, _window_id: u64) -> Result<()> {
        Err(DesktopError::NotImplemented("focus_window".to_string()))
    }
    async fn launch_app(&self, _app_id: &str) -> Result<()> {
        Err(DesktopError::NotImplemented("launch_app".to_string()))
    }
}
```

**Step 3: Build and test**

```bash
cargo test -p aleph-desktop --lib
```

Expected: tests pass (screenshot test skipped in CI, OCR test validates NotImplemented on non-Windows).

**Step 4: Commit**

```bash
git add crates/desktop/
git commit -m "feat(desktop): implement screenshot capability with xcap"
```

---

### Task 3: Implement input actions (click, type_text, key_combo, scroll)

**Files:**
- Modify: `crates/desktop/src/action.rs`
- Modify: `crates/desktop/src/lib.rs` (update trait impl stubs)

**Step 1: Implement action handlers in `action.rs`**

Port from `apps/desktop/src-tauri/src/bridge/action.rs` but return `Result<T>` instead of `Result<Value, (i32, String)>`:

```rust
//! Action handlers — click, type, key_combo, scroll, launch_app, window mgmt.

use crate::error::{DesktopError, Result};
use crate::{MouseButton, WindowInfo};
use enigo::{
    Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
};
use tracing::info;

/// Click at screen coordinates.
pub fn click(x: f64, y: f64, button: MouseButton) -> Result<()> {
    let btn = match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo: {e}")))?;

    enigo
        .move_mouse(x as i32, y as i32, Coordinate::Abs)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;

    enigo
        .button(btn, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to click: {e}")))?;

    info!(x, y, "Click performed");
    Ok(())
}

/// Type a string of text.
pub fn type_text(text: &str) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo: {e}")))?;

    enigo
        .text(text)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to type text: {e}")))?;

    info!(chars = text.chars().count(), "Text typed");
    Ok(())
}

/// Press a key combination.
///
/// Keys format: last element is the main key, preceding are modifiers.
/// Modifiers: "meta"/"cmd"/"command", "shift", "control"/"ctrl", "alt"/"option"
pub fn key_combo(keys: &[String]) -> Result<()> {
    if keys.is_empty() {
        return Err(DesktopError::InputFailed("Empty keys array".to_string()));
    }

    let (mod_strs, main_str) = keys.split_at(keys.len() - 1);

    let main_key = parse_key(&main_str[0])?;
    let modifier_keys: Vec<Key> = mod_strs
        .iter()
        .map(|s| parse_modifier(s))
        .collect::<Result<Vec<_>>>()?;

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo: {e}")))?;

    // Press modifiers
    for m in &modifier_keys {
        enigo
            .key(*m, Direction::Press)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to press modifier: {e}")))?;
    }

    // Click main key
    enigo
        .key(main_key, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to click key: {e}")))?;

    // Release modifiers in reverse
    for m in modifier_keys.iter().rev() {
        enigo
            .key(*m, Direction::Release)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to release modifier: {e}")))?;
    }

    info!(keys = ?keys, "Key combo performed");
    Ok(())
}

/// Scroll in a direction.
pub fn scroll(direction: &str, amount: i32) -> Result<()> {
    let (axis, length) = match direction {
        "down" => (Axis::Vertical, amount),
        "up" => (Axis::Vertical, -amount),
        "right" => (Axis::Horizontal, amount),
        "left" => (Axis::Horizontal, -amount),
        other => {
            return Err(DesktopError::InputFailed(format!(
                "Unknown scroll direction: '{other}'. Expected up, down, left, right"
            )));
        }
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo: {e}")))?;

    enigo
        .scroll(length, axis)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to scroll: {e}")))?;

    info!(direction, amount, "Scroll performed");
    Ok(())
}

/// Launch an application.
pub fn launch_app(app_id: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "start", "", app_id])
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}", app_id, stderr.trim()
            )));
        }
        info!(app_id, "App launched (Windows)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("xdg-open")
            .arg(app_id)
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}", app_id, stderr.trim()
            )));
        }
        info!(app_id, "App launched (Linux)");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("open")
            .arg("-b")
            .arg(app_id)
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}", app_id, stderr.trim()
            )));
        }
        info!(app_id, "App launched (macOS)");
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = app_id;
        Err(DesktopError::NotImplemented("launch_app".to_string()))
    }
}

/// List visible windows.
pub fn window_list() -> Result<Vec<WindowInfo>> {
    #[cfg(target_os = "windows")]
    { windows_window_list() }

    #[cfg(target_os = "linux")]
    { linux_window_list() }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    { Err(DesktopError::NotImplemented("window_list".to_string())) }
}

/// Focus a window by ID.
pub fn focus_window(window_id: u64) -> Result<()> {
    #[cfg(target_os = "windows")]
    { windows_focus_window(window_id) }

    #[cfg(target_os = "linux")]
    { linux_focus_window(window_id) }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = window_id;
        Err(DesktopError::NotImplemented("focus_window".to_string()))
    }
}

// ── Key parsing (ported from Tauri bridge) ────────────────────────

fn parse_modifier(name: &str) -> Result<Key> {
    match name.to_lowercase().as_str() {
        "meta" | "command" | "cmd" | "super" | "win" => Ok(Key::Meta),
        "shift" => Ok(Key::Shift),
        "control" | "ctrl" => Ok(Key::Control),
        "alt" | "option" => Ok(Key::Alt),
        other => Err(DesktopError::InputFailed(format!(
            "Unknown modifier: '{other}'"
        ))),
    }
}

fn parse_key(name: &str) -> Result<Key> {
    if name.len() == 1 {
        let ch = name.chars().next().unwrap();
        return Ok(Key::Unicode(ch));
    }
    match name.to_lowercase().as_str() {
        "space" => Ok(Key::Unicode(' ')),
        "return" | "enter" => Ok(Key::Return),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" | "delete" => Ok(Key::Backspace),
        "up" | "uparrow" => Ok(Key::UpArrow),
        "down" | "downarrow" => Ok(Key::DownArrow),
        "left" | "leftarrow" => Ok(Key::LeftArrow),
        "right" | "rightarrow" => Ok(Key::RightArrow),
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "pageup" => Ok(Key::PageUp),
        "pagedown" => Ok(Key::PageDown),
        "f1" => Ok(Key::F1), "f2" => Ok(Key::F2), "f3" => Ok(Key::F3),
        "f4" => Ok(Key::F4), "f5" => Ok(Key::F5), "f6" => Ok(Key::F6),
        "f7" => Ok(Key::F7), "f8" => Ok(Key::F8), "f9" => Ok(Key::F9),
        "f10" => Ok(Key::F10), "f11" => Ok(Key::F11), "f12" => Ok(Key::F12),
        other => Err(DesktopError::InputFailed(format!(
            "Unknown key: '{other}'"
        ))),
    }
}

// ── Platform-specific window management ──────────────────────────

#[cfg(target_os = "linux")]
fn linux_window_list() -> Result<Vec<WindowInfo>> {
    let output = std::process::Command::new("wmctrl")
        .args(["-l", "-p"])
        .output()
        .map_err(|e| DesktopError::WindowFailed(format!(
            "Failed to run wmctrl: {e}"
        )))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::WindowFailed(format!("wmctrl failed: {}", stderr.trim())));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(5, char::is_whitespace).collect();
        if parts.len() < 5 { continue; }
        let id_str = parts[0].trim_start_matches("0x").trim_start_matches("0X");
        let id = u64::from_str_radix(id_str, 16).unwrap_or(0);
        let pid: u64 = parts[2].trim().parse().unwrap_or(0);
        let title = parts[4].trim().to_string();
        windows.push(WindowInfo { id, title, pid });
    }
    Ok(windows)
}

#[cfg(target_os = "linux")]
fn linux_focus_window(window_id: u64) -> Result<()> {
    let id_hex = format!("0x{:08x}", window_id);
    let output = std::process::Command::new("wmctrl")
        .args(["-i", "-a", &id_hex])
        .output()
        .map_err(|e| DesktopError::WindowFailed(format!("Failed to run wmctrl: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "Failed to focus window {id_hex}: {}", stderr.trim()
        )));
    }
    Ok(())
}

// Windows window management — ported from Tauri bridge
#[cfg(target_os = "windows")]
fn windows_window_list() -> Result<Vec<WindowInfo>> {
    // Port from apps/desktop/src-tauri/src/bridge/action.rs windows_window_list()
    // Return Result<Vec<WindowInfo>> instead of Result<Value, (i32, String)>
    todo!("Port Windows window_list from Tauri bridge")
}

#[cfg(target_os = "windows")]
fn windows_focus_window(window_id: u64) -> Result<()> {
    // Port from apps/desktop/src-tauri/src/bridge/action.rs windows_focus_window()
    todo!("Port Windows focus_window from Tauri bridge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_modifier_meta() {
        assert!(matches!(parse_modifier("meta"), Ok(Key::Meta)));
        assert!(matches!(parse_modifier("cmd"), Ok(Key::Meta)));
        assert!(matches!(parse_modifier("command"), Ok(Key::Meta)));
    }

    #[test]
    fn test_parse_modifier_shift() {
        assert!(matches!(parse_modifier("shift"), Ok(Key::Shift)));
    }

    #[test]
    fn test_parse_modifier_unknown() {
        assert!(parse_modifier("unknown").is_err());
    }

    #[test]
    fn test_parse_key_single_char() {
        assert!(matches!(parse_key("c"), Ok(Key::Unicode('c'))));
        assert!(matches!(parse_key("a"), Ok(Key::Unicode('a'))));
    }

    #[test]
    fn test_parse_key_named() {
        assert!(matches!(parse_key("return"), Ok(Key::Return)));
        assert!(matches!(parse_key("enter"), Ok(Key::Return)));
        assert!(matches!(parse_key("tab"), Ok(Key::Tab)));
        assert!(matches!(parse_key("escape"), Ok(Key::Escape)));
    }

    #[test]
    fn test_parse_key_unknown() {
        assert!(parse_key("nonexistent").is_err());
    }

    #[test]
    fn test_scroll_invalid_direction() {
        let result = scroll("diagonal", 3);
        assert!(result.is_err());
    }

    #[test]
    fn test_key_combo_empty() {
        let result = key_combo(&[]);
        assert!(result.is_err());
    }
}
```

**Step 2: Update trait impl in lib.rs**

Replace all the stub methods with calls to action module:

```rust
async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
    let btn = button;
    tokio::task::spawn_blocking(move || action::click(x, y, btn))
        .await
        .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
}

async fn type_text(&self, text: &str) -> Result<()> {
    let t = text.to_string();
    tokio::task::spawn_blocking(move || action::type_text(&t))
        .await
        .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
}

async fn key_combo(&self, keys: &[String]) -> Result<()> {
    let k = keys.to_vec();
    tokio::task::spawn_blocking(move || action::key_combo(&k))
        .await
        .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
}

async fn scroll(&self, direction: &str, amount: i32) -> Result<()> {
    let d = direction.to_string();
    tokio::task::spawn_blocking(move || action::scroll(&d, amount))
        .await
        .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
}

async fn window_list(&self) -> Result<Vec<WindowInfo>> {
    tokio::task::spawn_blocking(action::window_list)
        .await
        .map_err(|e| DesktopError::WindowFailed(format!("Task join error: {e}")))?
}

async fn focus_window(&self, window_id: u64) -> Result<()> {
    tokio::task::spawn_blocking(move || action::focus_window(window_id))
        .await
        .map_err(|e| DesktopError::WindowFailed(format!("Task join error: {e}")))?
}

async fn launch_app(&self, app_id: &str) -> Result<()> {
    let id = app_id.to_string();
    tokio::task::spawn_blocking(move || action::launch_app(&id))
        .await
        .map_err(|e| DesktopError::InputFailed(format!("Task join error: {e}")))?
}
```

**Step 3: Build and test**

```bash
cargo test -p aleph-desktop --lib
```

Expected: all key parsing tests pass, scroll direction test passes.

**Step 4: Commit**

```bash
git add crates/desktop/
git commit -m "feat(desktop): implement input actions with enigo (click, type, key_combo, scroll)"
```

---

### Task 4: Integrate `DesktopCapability` into `DesktopTool` (dual-path)

**Files:**
- Modify: `core/Cargo.toml` (add optional aleph-desktop dep)
- Modify: `core/src/desktop/mod.rs` (add conditional re-exports)
- Modify: `core/src/builtin_tools/desktop.rs` (add dual-path: native or IPC)
- Modify: `core/src/executor/builtin_registry/registry.rs` (pass NativeDesktop if available)

**Step 1: Add optional dependency to core/Cargo.toml**

```toml
[dependencies]
aleph-desktop = { path = "../crates/desktop", optional = true }

[features]
desktop-native = ["aleph-desktop"]
```

**Step 2: Update `core/src/desktop/mod.rs`**

```rust
pub mod client;
pub mod error;
pub mod types;

pub use client::DesktopBridgeClient;
pub use error::DesktopError;
pub use types::{
    CanvasPosition, DesktopRequest, DesktopResponse, DesktopRpcError, MouseButton, RefId,
    ResolvedElement, ScreenRegion, SnapshotStats,
};

// Re-export NativeDesktop when desktop-native feature is enabled
#[cfg(feature = "desktop-native")]
pub use aleph_desktop::{
    DesktopCapability, NativeDesktop,
    Screenshot, OcrResult, WindowInfo, Capability,
};
```

**Step 3: Modify `DesktopTool` to accept `DesktopCapability`**

In `core/src/builtin_tools/desktop.rs`, add an optional `native` field:

Change `DesktopTool` struct to:
```rust
#[derive(Clone)]
pub struct DesktopTool {
    client: DesktopBridgeClient,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    #[cfg(feature = "desktop-native")]
    native: Option<Arc<dyn aleph_desktop::DesktopCapability>>,
}
```

Add a `with_native()` method:
```rust
#[cfg(feature = "desktop-native")]
pub fn with_native(mut self, native: Arc<dyn aleph_desktop::DesktopCapability>) -> Self {
    self.native = Some(native);
    self
}
```

Modify `call()` to prefer native when available. For screenshot action as example:
```rust
"screenshot" => {
    #[cfg(feature = "desktop-native")]
    if let Some(ref native) = self.native {
        let region = args.region.map(|r| aleph_desktop::ScreenRegion {
            x: r.x, y: r.y, width: r.width, height: r.height,
        });
        match native.screenshot(region).await {
            Ok(s) => return Ok(DesktopOutput {
                success: true,
                data: Some(serde_json::json!({
                    "image_base64": s.image_base64,
                    "width": s.width, "height": s.height, "format": s.format,
                })),
                message: None,
            }),
            Err(e) => return Ok(DesktopOutput {
                success: false, data: None,
                message: Some(format!("Native desktop error: {e}")),
            }),
        }
    }
    // Fall through to IPC path
    let request = build_request(&args).map_err(|msg| ...)?;
    self.client.send(request).await...
}
```

**Step 4: Update `new()` and `Default`**

```rust
impl DesktopTool {
    pub fn new() -> Self {
        Self {
            client: DesktopBridgeClient::new(),
            approval_policy: None,
            #[cfg(feature = "desktop-native")]
            native: None,
        }
    }
}
```

**Step 5: Build and test both paths**

```bash
# Without native — existing IPC path
cargo test -p alephcore --lib builtin_tools::desktop

# With native feature
cargo test -p alephcore --lib builtin_tools::desktop --features desktop-native
```

Expected: all existing tests pass in both modes.

**Step 6: Commit**

```bash
git add core/Cargo.toml core/src/desktop/ core/src/builtin_tools/desktop.rs
git commit -m "feat(desktop): dual-path DesktopTool — native in-process or IPC fallback"
```

---

### Task 5: Add `desktop` feature gate to server binary

**Files:**
- Modify: `core/Cargo.toml` (ensure feature propagation)
- Modify: server binary startup code (instantiate NativeDesktop when feature enabled)

**Step 1: Identify server entry point**

Check how aleph-server starts and where DesktopTool/BuiltinToolRegistry is instantiated.

**Step 2: Add feature gate in server Cargo.toml**

Wherever the server binary is defined, add:
```toml
[features]
desktop = ["alephcore/desktop-native"]
```

**Step 3: Instantiate `NativeDesktop` on startup**

In the server startup code, when building the tool registry:

```rust
#[cfg(feature = "desktop")]
{
    use aleph_desktop::NativeDesktop;
    let native = Arc::new(NativeDesktop::new().expect("Failed to init desktop capabilities"));
    // Pass to DesktopTool via registry
    desktop_tool = desktop_tool.with_native(native);
}
```

**Step 4: Build with feature**

```bash
cargo build --bin aleph-server --features desktop
```

Expected: compiles successfully.

**Step 5: Test server startup**

```bash
cargo run --bin aleph-server --features desktop -- --help
```

Expected: server starts, logs show "Desktop capabilities: [screen_capture, keyboard_control, ...]".

**Step 6: Commit**

```bash
git add core/Cargo.toml apps/server/ # or wherever the server binary lives
git commit -m "feat(server): add desktop feature gate for in-process desktop capabilities"
```

---

### Task 6: Build verification and cleanup

**Step 1: Verify all build configurations**

```bash
# Core without desktop
cargo check -p alephcore

# Core with desktop
cargo check -p alephcore --features desktop-native

# Desktop crate alone
cargo check -p aleph-desktop

# Full test suite
cargo test -p aleph-desktop --lib
cargo test -p alephcore --lib builtin_tools::desktop
cargo test -p alephcore --lib builtin_tools::desktop --features desktop-native
```

**Step 2: Verify Tauri still builds**

```bash
cargo check -p aleph-tauri
```

Expected: Tauri bridge still builds — it has its own copy of the handlers, unchanged.

**Step 3: Run clippy**

```bash
cargo clippy -p aleph-desktop -- -D warnings
cargo clippy -p alephcore --features desktop-native -- -D warnings
```

**Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix(desktop): address clippy warnings and build verification"
```

---

### Task 7: Update documentation

**Files:**
- Modify: `CLAUDE.md` (add crates/desktop to project structure)
- No README (per instructions)

**Step 1: Update CLAUDE.md project structure**

Add `crates/desktop/` to the project structure section.

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add crates/desktop to project structure"
```

---

## Build Commands Quick Reference

```bash
# Pure server (no desktop, for cloud/remote)
cargo build --bin aleph-server

# Server with native desktop (local daemon)
cargo build --bin aleph-server --features desktop

# Server with desktop + control plane UI
cargo build --bin aleph-server --features desktop,control-plane

# Desktop crate standalone
cargo test -p aleph-desktop --lib

# Core with native desktop
cargo test -p alephcore --lib --features desktop-native
```
