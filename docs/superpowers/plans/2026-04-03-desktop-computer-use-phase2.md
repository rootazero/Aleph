# Desktop Computer Use Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add safety mechanisms (session lock, escape abort) and interaction enhancements (multi-display, screenshot optimization, batch operations, smart paste) to the desktop computer use system.

**Architecture:** Session lock and escape abort integrate into DesktopTool's `call()` lifecycle (acquire lock → check abort → execute → release). Multi-display and screenshot optimization extend the perception layer. Batch and paste are new action handlers in the tool dispatch layer. EscapeListener is platform-specific (macOS CGEventTap) exposed via a cross-platform trait.

**Tech Stack:** Rust, enigo, xcap, image crate (JPEG encoding, resize), objc2 (CGEventTap), tokio, serde/schemars

**Spec:** `docs/superpowers/specs/2026-04-03-desktop-computer-use-phase2-design.md`

---

## File Map

### New files

| File | Responsibility | ~Lines |
|------|---------------|--------|
| `src/builtin_tools/desktop/session_lock.rs` | ComputerUseLock — file-based session locking | ~100 |
| `desktop/macos/src/escape_listener.rs` | macOS EscapeListener — CGEventTap Escape key monitor | ~120 |

### Modified files

| File | Changes |
|------|---------|
| `desktop/shared/src/lib.rs` | Add `DisplayInfo` struct |
| `desktop/shared/src/traits/screen.rs` | Add `display_list()` method with default |
| `desktop/shared/src/platform.rs` | Add `EscapeAbort` trait, extend `DesktopPlatform` with `escape_listener()` |
| `desktop/shared/src/perception/screenshot.rs` | Add `list_displays()`, `take_screenshot_display()`, `process_screenshot()` |
| `desktop/shared/src/perception/mod.rs` | Re-export new screenshot functions |
| `desktop/shared/src/native_screen.rs` | Implement `display_list()` |
| `desktop/macos/src/lib.rs` | Add `escape_listener` field, wire into `DesktopPlatform` |
| `desktop/macos/Cargo.toml` | Add `core-graphics` features for CGEventTap |
| `src/builtin_tools/desktop/mod.rs` | Add `session_lock` module, update `DesktopTool` struct + `call()` lifecycle, update DESCRIPTION + `classify_approval()` |
| `src/builtin_tools/desktop/native.rs` | Add handlers for `display_list`, `batch`, `paste`, screenshot optimization |
| `src/builtin_tools/desktop/types.rs` | Add new DesktopArgs fields |
| `src/builtin_tools/desktop/tests.rs` | Update `make_args()`, add session_lock tests |

---

## Task 1: Multi-display support + screenshot optimization (perception layer)

**Files:**
- Modify: `desktop/shared/src/lib.rs`
- Modify: `desktop/shared/src/traits/screen.rs`
- Modify: `desktop/shared/src/perception/screenshot.rs`
- Modify: `desktop/shared/src/perception/mod.rs`
- Modify: `desktop/shared/src/native_screen.rs`

- [ ] **Step 1: Add DisplayInfo struct to lib.rs**

Add after the `PressAction` enum in `desktop/shared/src/lib.rs`:

```rust
/// Information about a connected display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Platform-specific display identifier.
    pub id: u32,
    /// Display name (may be empty on some platforms).
    pub name: String,
    /// Logical width in pixels.
    pub width: u32,
    /// Logical height in pixels.
    pub height: u32,
    /// Scale factor (e.g., 2.0 for Retina).
    pub scale_factor: f64,
    /// Whether this is the primary display.
    pub is_primary: bool,
    /// X origin in the global coordinate space.
    pub origin_x: i32,
    /// Y origin in the global coordinate space.
    pub origin_y: i32,
}
```

- [ ] **Step 2: Add display_list to ScreenCapability trait**

In `desktop/shared/src/traits/screen.rs`, add to the imports:

```rust
use crate::{DisplayInfo, MouseButton, OcrResult, PressAction, Result, ScreenRegion, Screenshot, WindowInfo};
```

Add new method after `clipboard_write`:

```rust
    /// List all connected displays.
    async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
        Err(crate::DesktopError::NotImplemented("display_list".into()))
    }
```

- [ ] **Step 3: Add new functions to perception/screenshot.rs**

Append to `desktop/shared/src/perception/screenshot.rs`:

```rust
use crate::DisplayInfo;

/// List all connected displays.
pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let mut displays = Vec::new();
    for m in &monitors {
        displays.push(DisplayInfo {
            id: m.id(),
            name: m.name().unwrap_or_default().to_string(),
            width: m.width(),
            height: m.height(),
            scale_factor: m.scale_factor(),
            is_primary: m.is_primary().unwrap_or(false),
            origin_x: m.x(),
            origin_y: m.y(),
        });
    }
    Ok(displays)
}

/// Capture a screenshot of a specific display by ID.
pub fn take_screenshot_display(
    display_id: u32,
    region: Option<&ScreenRegion>,
) -> Result<Screenshot> {
    debug!("Taking screenshot of display {display_id}, region: {region:?}");

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.id() == display_id)
        .ok_or_else(|| {
            DesktopError::ScreenCapture(format!("Display with id {display_id} not found"))
        })?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x, r.y, r.width, r.height),
        None => monitor.capture_image(),
    }
    .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());
    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());
    debug!("Screenshot captured from display {display_id}: {width}x{height}");

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: "png".to_string(),
    })
}

/// Post-process a captured image: resize and/or convert format.
///
/// Returns a `Screenshot` with the processed image as base64.
pub fn process_screenshot(
    raw_png: &[u8],
    max_width: Option<u32>,
    max_height: Option<u32>,
    format: &str,
    quality: f64,
) -> Result<Screenshot> {
    let img = image::load_from_memory(raw_png)
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to decode image: {e}")))?;

    // 1. Resize if exceeds limits (maintain aspect ratio)
    let img = match (max_width, max_height) {
        (Some(mw), Some(mh)) if img.width() > mw || img.height() > mh => {
            img.resize(mw, mh, image::imageops::FilterType::Lanczos3)
        }
        (Some(mw), None) if img.width() > mw => {
            let ratio = mw as f64 / img.width() as f64;
            let mh = (img.height() as f64 * ratio) as u32;
            img.resize(mw, mh, image::imageops::FilterType::Lanczos3)
        }
        (None, Some(mh)) if img.height() > mh => {
            let ratio = mh as f64 / img.height() as f64;
            let mw = (img.width() as f64 * ratio) as u32;
            img.resize(mw, mh, image::imageops::FilterType::Lanczos3)
        }
        _ => img,
    };

    let (width, height) = (img.width(), img.height());

    // 2. Encode to requested format
    let mut buf = Cursor::new(Vec::new());
    let fmt_str = match format {
        "jpeg" | "jpg" => {
            let q = (quality.clamp(0.0, 1.0) * 100.0) as u8;
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
            img.write_with_encoder(encoder)
                .map_err(|e| DesktopError::ScreenCapture(format!("JPEG encoding failed: {e}")))?;
            "jpeg"
        }
        _ => {
            img.write_to(&mut buf, image::ImageFormat::Png)
                .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;
            "png"
        }
    };

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());
    debug!("Screenshot processed: {width}x{height} as {fmt_str}");

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: fmt_str.to_string(),
    })
}
```

- [ ] **Step 4: Update perception/mod.rs re-exports**

Add to the re-exports in `desktop/shared/src/perception/mod.rs`:

```rust
pub use screenshot::{capture_screen_png, list_displays, process_screenshot,
                     take_screenshot, take_screenshot_display};
```

- [ ] **Step 5: Implement display_list in NativeScreen**

In `desktop/shared/src/native_screen.rs`, add `DisplayInfo` to imports:

```rust
use crate::{
    action, perception, DesktopError, DisplayInfo, MouseButton, OcrResult, PressAction, Result,
    ScreenRegion, Screenshot, WindowInfo,
};
```

Add implementation in the `impl ScreenCapability for NativeScreen` block:

```rust
    async fn display_list(&self) -> Result<Vec<DisplayInfo>> {
        tokio::task::spawn_blocking(perception::list_displays)
            .await
            .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
    }
```

- [ ] **Step 6: Verify compilation and tests**

Run: `cargo check -p aleph-desktop && cargo test -p aleph-desktop --lib`
Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add desktop/shared/
git commit -m "feat(desktop): add multi-display support and screenshot optimization"
```

---

## Task 2: EscapeAbort trait + macOS EscapeListener

**Files:**
- Modify: `desktop/shared/src/platform.rs`
- Create: `desktop/macos/src/escape_listener.rs`
- Modify: `desktop/macos/src/lib.rs`
- Modify: `desktop/macos/Cargo.toml`

- [ ] **Step 1: Add EscapeAbort trait to platform.rs**

In `desktop/shared/src/platform.rs`, add the trait and extend DesktopPlatform:

```rust
/// Cross-platform escape abort interface.
///
/// Allows the user to press Escape to abort AI desktop control.
/// Platform implementations provide the actual key listening mechanism.
pub trait EscapeAbort: Send + Sync {
    /// Start listening for Escape key presses.
    fn start(&self) -> crate::Result<()>;

    /// Stop listening and clean up.
    fn stop(&self);

    /// Check if the user has pressed Escape since the last reset.
    fn is_aborted(&self) -> bool;

    /// Reset the abort flag (prepare for next action).
    fn reset(&self);
}
```

Add to `DesktopPlatform` trait:

```rust
    /// Escape key abort listener, if available on this platform.
    fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
        None
    }
```

- [ ] **Step 2: Create macOS EscapeListener**

Create `desktop/macos/src/escape_listener.rs`:

```rust
//! macOS Escape key listener using CGEventTap.
//!
//! Monitors global keyboard events for the Escape key (keycode 53).
//! When detected, sets an atomic abort flag that can be polled by the
//! desktop tool to stop ongoing computer use operations.

use aleph_desktop::error::{DesktopError, Result};
use aleph_desktop::platform::EscapeAbort;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, warn};

/// macOS Escape key listener via CGEventTap.
pub struct EscapeListener {
    abort_flag: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
}

impl EscapeListener {
    pub fn new() -> Self {
        Self {
            abort_flag: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Default for EscapeListener {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAbort for EscapeListener {
    fn start(&self) -> Result<()> {
        if self.active.load(Ordering::Relaxed) {
            return Ok(()); // Already running
        }

        // Check Accessibility permission
        let trusted: bool = unsafe {
            core_foundation::base::Boolean::from(
                core_graphics::access::CGPreflightScreenCaptureAccess(),
            )
            .into()
        };
        // Note: We actually need AXIsProcessTrusted for event taps
        // CGPreflightScreenCaptureAccess is for screen capture
        // Use the accessibility check from objc2
        let ax_trusted: bool = {
            // AXIsProcessTrusted() from ApplicationServices
            extern "C" {
                fn AXIsProcessTrusted() -> bool;
            }
            unsafe { AXIsProcessTrusted() }
        };

        if !ax_trusted {
            warn!("Accessibility permission not granted — Escape abort hotkey unavailable");
            return Ok(()); // Graceful degradation
        }

        let abort_flag = Arc::clone(&self.abort_flag);
        let active = Arc::clone(&self.active);
        active.store(true, Ordering::Relaxed);

        std::thread::spawn(move || {
            use core_graphics::event::{
                CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
            };

            let abort = abort_flag.clone();
            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![CGEventType::KeyDown],
                move |_proxy, _event_type, event| {
                    // Escape keycode = 53
                    let keycode = event.get_integer_value_field(
                        core_graphics::event::EventField::KEYBOARD_EVENT_KEYCODE,
                    );
                    if keycode == 53 {
                        abort.store(true, Ordering::Relaxed);
                        debug!("Escape key detected — setting abort flag");
                    }
                    Some(event)
                },
            );

            match tap {
                Ok(tap) => {
                    let source = tap.mach_port_create_runloop_source(0);
                    match source {
                        Ok(source) => {
                            unsafe {
                                CFRunLoop::get_current().add_source(
                                    &source,
                                    kCFRunLoopCommonModes,
                                );
                            }
                            tap.enable();
                            debug!("Escape listener started (CGEventTap)");
                            // Run until active flag is cleared
                            while active.load(Ordering::Relaxed) {
                                CFRunLoop::run_in_mode(
                                    kCFRunLoopCommonModes,
                                    std::time::Duration::from_millis(100),
                                    false,
                                );
                            }
                            debug!("Escape listener stopped");
                        }
                        Err(e) => {
                            warn!("Failed to create run loop source for escape listener");
                            active.store(false, Ordering::Relaxed);
                        }
                    }
                }
                Err(()) => {
                    warn!("Failed to create CGEventTap — Escape abort unavailable");
                    active.store(false, Ordering::Relaxed);
                }
            }
        });

        Ok(())
    }

    fn stop(&self) {
        self.active.store(false, Ordering::Relaxed);
    }

    fn is_aborted(&self) -> bool {
        self.abort_flag.load(Ordering::Relaxed)
    }

    fn reset(&self) {
        self.abort_flag.store(false, Ordering::Relaxed);
    }
}
```

**Important**: The CGEventTap API may require different imports depending on the `core-graphics` crate version. The implementer should check the actual API and adjust. The key principle is: create an event tap that listens for keyDown events, check if keycode == 53 (Escape), and set the abort flag. If the API doesn't match exactly, adapt while keeping the same behavior.

- [ ] **Step 3: Wire into MacOSPlatform**

In `desktop/macos/src/lib.rs`:

Add `mod escape_listener;` and `use escape_listener::EscapeListener;`.

Add field to `MacOSPlatform`:
```rust
pub struct MacOSPlatform {
    screen: NativeScreen,
    automation: MacOSAutomation,
    media: MacOSMedia,
    permission: MacOSPermission,
    pim: MacOSPim,
    system: MacOSSystem,
    escape: EscapeListener,  // new
}
```

Update `new()`:
```rust
pub fn new() -> Self {
    Self {
        screen: NativeScreen::new(),
        automation: MacOSAutomation::new(),
        media: MacOSMedia::new(),
        permission: MacOSPermission::new(),
        pim: MacOSPim::new(),
        system: MacOSSystem::new(),
        escape: EscapeListener::new(),  // new
    }
}
```

Add the import and implement in `DesktopPlatform`:
```rust
use aleph_desktop::platform::EscapeAbort;

// In impl DesktopPlatform for MacOSPlatform:
fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
    Some(&self.escape)
}
```

- [ ] **Step 4: Update desktop/macos/Cargo.toml if needed**

The `core-graphics` crate with event tap features may need to be added or its features extended. Check if `core-graphics = "0.25"` (already in `desktop/shared/Cargo.toml`) needs to also be in `desktop/macos/Cargo.toml`. Add it if not present:

```toml
core-graphics = "0.25"
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-desktop -p aleph-desktop-macos`
Expected: No errors. The CGEventTap API may need adjustments — fix any compilation errors in escape_listener.rs while preserving the design (listen for Escape keycode 53, set AtomicBool).

- [ ] **Step 6: Commit**

```bash
git add desktop/
git commit -m "feat(desktop): add EscapeAbort trait and macOS CGEventTap listener"
```

---

## Task 3: Session Lock (ComputerUseLock)

**Files:**
- Create: `src/builtin_tools/desktop/session_lock.rs`

- [ ] **Step 1: Create session_lock.rs**

Create `src/builtin_tools/desktop/session_lock.rs`:

```rust
//! File-based session lock for desktop computer use.
//!
//! Prevents multiple agent sessions from simultaneously controlling the desktop.
//! The lock file is stored at `~/.aleph/data/computer-use.lock`.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Lock file content.
#[derive(Debug, Serialize, Deserialize)]
struct LockInfo {
    session_id: String,
    pid: u32,
    acquired_at: String,
}

/// File-based lock for exclusive desktop control.
///
/// Only one session can hold the lock at a time. Stale locks from dead
/// processes are automatically recovered.
pub struct ComputerUseLock {
    lock_path: PathBuf,
    session_id: String,
    held: bool,
}

impl ComputerUseLock {
    /// Create a new lock instance (does NOT acquire the lock).
    pub fn new(session_id: &str) -> Self {
        let lock_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph/data/computer-use.lock");
        Self {
            lock_path,
            session_id: session_id.to_string(),
            held: false,
        }
    }

    /// Try to acquire the lock.
    ///
    /// - If no lock file exists, create it and acquire.
    /// - If lock exists and held by this session, treat as re-entrant (skip).
    /// - If lock exists and held by a dead process, force takeover.
    /// - If lock exists and held by a live process, return error.
    pub fn acquire(&mut self) -> std::result::Result<(), String> {
        if self.held {
            return Ok(()); // Re-entrant
        }

        // Ensure parent directory exists
        if let Some(parent) = self.lock_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Check existing lock
        if self.lock_path.exists() {
            match fs::read_to_string(&self.lock_path) {
                Ok(content) => {
                    if let Ok(info) = serde_json::from_str::<LockInfo>(&content) {
                        // Same session? Re-entrant.
                        if info.session_id == self.session_id {
                            self.held = true;
                            return Ok(());
                        }

                        // Check if the holding process is still alive
                        if is_process_alive(info.pid) {
                            return Err(format!(
                                "Desktop is being controlled by another session (pid: {}, session: {})",
                                info.pid, info.session_id
                            ));
                        }

                        // Stale lock — force takeover
                        warn!(
                            stale_pid = info.pid,
                            stale_session = %info.session_id,
                            "Taking over stale computer-use lock"
                        );
                    }
                }
                Err(_) => {
                    // Corrupt lock file — overwrite
                    warn!("Corrupt computer-use lock file — overwriting");
                }
            }
        }

        // Write new lock
        let info = LockInfo {
            session_id: self.session_id.clone(),
            pid: std::process::id(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
        };

        fs::write(&self.lock_path, serde_json::to_string_pretty(&info).unwrap_or_default())
            .map_err(|e| format!("Failed to write lock file: {e}"))?;

        self.held = true;
        debug!(session_id = %self.session_id, "Computer-use lock acquired");
        Ok(())
    }

    /// Release the lock.
    pub fn release(&mut self) -> std::result::Result<(), String> {
        if !self.held {
            return Ok(());
        }

        if self.lock_path.exists() {
            // Only delete if we own the lock
            if let Ok(content) = fs::read_to_string(&self.lock_path) {
                if let Ok(info) = serde_json::from_str::<LockInfo>(&content) {
                    if info.session_id != self.session_id {
                        // Someone else took over — don't delete
                        self.held = false;
                        return Ok(());
                    }
                }
            }
            let _ = fs::remove_file(&self.lock_path);
        }

        self.held = false;
        debug!(session_id = %self.session_id, "Computer-use lock released");
        Ok(())
    }

    /// Check if the lock is currently held by this instance.
    pub fn is_held(&self) -> bool {
        self.held
    }
}

impl Drop for ComputerUseLock {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Check if a process with the given PID is still alive.
fn is_process_alive(pid: u32) -> bool {
    // kill(pid, 0) checks existence without sending a signal
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock_path() -> PathBuf {
        let dir = std::env::temp_dir().join("aleph-test-locks");
        let _ = fs::create_dir_all(&dir);
        dir.join(format!("test-lock-{}.lock", std::process::id()))
    }

    fn make_lock(session_id: &str) -> ComputerUseLock {
        let mut lock = ComputerUseLock::new(session_id);
        lock.lock_path = temp_lock_path();
        lock
    }

    #[test]
    fn test_acquire_and_release() {
        let mut lock = make_lock("sess-1");
        assert!(!lock.is_held());
        lock.acquire().unwrap();
        assert!(lock.is_held());
        assert!(lock.lock_path.exists());
        lock.release().unwrap();
        assert!(!lock.is_held());
        assert!(!lock.lock_path.exists());
    }

    #[test]
    fn test_reentrant_acquire() {
        let mut lock = make_lock("sess-2");
        lock.acquire().unwrap();
        lock.acquire().unwrap(); // Should not error
        assert!(lock.is_held());
        lock.release().unwrap();
    }

    #[test]
    fn test_stale_lock_recovery() {
        let lock_path = temp_lock_path();
        // Write a lock from a dead PID
        let stale = LockInfo {
            session_id: "dead-session".into(),
            pid: 999999999, // Very unlikely to be alive
            acquired_at: "2020-01-01T00:00:00Z".into(),
        };
        fs::write(&lock_path, serde_json::to_string(&stale).unwrap()).unwrap();

        let mut lock = ComputerUseLock::new("new-session");
        lock.lock_path = lock_path.clone();
        lock.acquire().unwrap(); // Should take over
        assert!(lock.is_held());

        // Verify the lock file has our session
        let content = fs::read_to_string(&lock_path).unwrap();
        let info: LockInfo = serde_json::from_str(&content).unwrap();
        assert_eq!(info.session_id, "new-session");

        lock.release().unwrap();
    }

    #[test]
    fn test_live_lock_blocks() {
        let lock_path = temp_lock_path();
        // Write a lock from our own PID (which is alive) but different session
        let live = LockInfo {
            session_id: "other-session".into(),
            pid: std::process::id(),
            acquired_at: chrono::Utc::now().to_rfc3339(),
        };
        fs::write(&lock_path, serde_json::to_string(&live).unwrap()).unwrap();

        let mut lock = ComputerUseLock::new("my-session");
        lock.lock_path = lock_path.clone();
        let err = lock.acquire().unwrap_err();
        assert!(err.contains("another session"));

        // Clean up
        let _ = fs::remove_file(&lock_path);
    }

    #[test]
    fn test_drop_releases_lock() {
        let lock_path = temp_lock_path();
        {
            let mut lock = ComputerUseLock::new("drop-session");
            lock.lock_path = lock_path.clone();
            lock.acquire().unwrap();
            assert!(lock_path.exists());
        } // Drop should release
        assert!(!lock_path.exists());
    }
}
```

- [ ] **Step 2: Add `libc` dependency if not present**

Check if `libc` is already in the workspace or alephcore dependencies. If not, add to `Cargo.toml`:

```toml
libc = "0.2"
```

Or use an alternative PID check without libc:
```rust
fn is_process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()  // Linux only
}
```

On macOS, the `kill(pid, 0)` approach via `libc` is most reliable.

- [ ] **Step 3: Add module declaration to desktop tool mod.rs**

In `src/builtin_tools/desktop/mod.rs`, add:

```rust
mod session_lock;
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib -- desktop::session_lock`
Expected: All 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/desktop/session_lock.rs src/builtin_tools/desktop/mod.rs
git commit -m "feat(desktop): add ComputerUseLock with file-based session locking"
```

---

## Task 4: Update DesktopArgs with new fields

**Files:**
- Modify: `src/builtin_tools/desktop/types.rs`
- Modify: `src/builtin_tools/desktop/tests.rs`

- [ ] **Step 1: Add new fields to DesktopArgs**

In `src/builtin_tools/desktop/types.rs`, add after the existing fields (before the closing brace):

```rust
    /// Target display ID for screenshot (from display_list). If absent, uses primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,

    /// Screenshot format: "png" (default) or "jpeg".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// JPEG quality 0.0-1.0 (only when format="jpeg", default 0.75).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<f64>,

    /// Max screenshot width in pixels (scale down if wider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,

    /// Max screenshot height in pixels (scale down if taller).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_height: Option<u32>,

    /// Batch action list (only for action="batch").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<serde_json::Value>>,
```

- [ ] **Step 2: Update make_args() in tests.rs**

Add the new fields to the `make_args()` helper in `src/builtin_tools/desktop/tests.rs`:

```rust
        display_id: None,
        format: None,
        quality: None,
        max_width: None,
        max_height: None,
        actions: None,
```

- [ ] **Step 3: Verify compilation and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib -- desktop`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/desktop/types.rs src/builtin_tools/desktop/tests.rs
git commit -m "feat(desktop): add DesktopArgs fields for display_id, format, quality, batch"
```

---

## Task 5: Integrate session lock + escape into DesktopTool lifecycle

**Files:**
- Modify: `src/builtin_tools/desktop/mod.rs`

- [ ] **Step 1: Update DesktopTool struct and builders**

```rust
use session_lock::ComputerUseLock;
use std::sync::Mutex;

#[derive(Clone)]
pub struct DesktopTool {
    pub(super) approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    pub(super) platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,
    pub(super) session_lock: Option<Arc<Mutex<ComputerUseLock>>>,
    pub(super) escape_started: Arc<std::sync::atomic::AtomicBool>,
}

impl DesktopTool {
    pub fn new() -> Self {
        Self {
            approval_policy: None,
            platform: None,
            session_lock: None,
            escape_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    // ... existing with_platform, with_approval_policy ...

    /// Attach a session ID for computer-use locking.
    pub fn with_session_id(mut self, session_id: &str) -> Self {
        self.session_lock = Some(Arc::new(Mutex::new(ComputerUseLock::new(session_id))));
        self
    }
}
```

- [ ] **Step 2: Add helper methods for lock and escape**

```rust
impl DesktopTool {
    /// Acquire session lock for mutating actions.
    fn acquire_lock(&self) -> std::result::Result<(), DesktopOutput> {
        if let Some(ref lock) = self.session_lock {
            let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(msg) = guard.acquire() {
                return Err(DesktopOutput {
                    success: false,
                    data: None,
                    message: Some(msg),
                });
            }
        }
        Ok(())
    }

    /// Check escape abort and start listener if needed.
    fn check_escape(&self) -> std::result::Result<(), DesktopOutput> {
        if let Some(ref platform) = self.platform {
            if let Some(listener) = platform.escape_listener() {
                // Start on first call
                if !self.escape_started.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = listener.start();
                    self.escape_started.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                // Check abort flag
                if listener.is_aborted() {
                    return Err(DesktopOutput {
                        success: false,
                        data: None,
                        message: Some("Computer use aborted by user (Escape pressed)".into()),
                    });
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 3: Update call() lifecycle**

```rust
async fn call(&self, args: Self::Args) -> Result<Self::Output> {
    let is_mutating = classify_approval(&args).is_some();

    // 1. Approval check
    if let Some((action_type, target)) = classify_approval(&args) {
        if let Some(out) = self.check_approval(action_type, &target).await {
            return Ok(out);
        }
    }

    // 2. Session lock (mutating actions only)
    if is_mutating {
        if let Err(out) = self.acquire_lock() {
            return Ok(out);
        }
    }

    // 3. Escape abort check (mutating actions only)
    if is_mutating {
        if let Err(out) = self.check_escape() {
            return Ok(out);
        }
    }

    // 4. Execute via platform
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

- [ ] **Step 4: Update classify_approval for new actions**

Add to the `classify_approval` function:

```rust
// In the read-only match arm, add:
"display_list" => None,

// In the click-type match arm, add (batch needs approval):
"batch" => Some((ActionType::DesktopClick, "batch operation".into())),

// paste is type-like:
"paste" => Some((
    ActionType::DesktopType,
    args.text.clone().unwrap_or_default(),
)),
```

- [ ] **Step 5: Update DESCRIPTION**

Add to the DESCRIPTION constant in the actions list:

```
- display_list: List all connected displays with resolution and scale info.
- batch: Execute multiple actions sequentially. Requires actions array.
- paste: Paste text via clipboard (Cmd+V). Better for multiline text than type_text.
```

Add examples:

```
{"action":"display_list"}
{"action":"batch","actions":[{"action":"click","x":100,"y":200},{"action":"type_text","text":"hello"}]}
{"action":"paste","text":"line1\nline2\nline3"}
{"action":"screenshot","format":"jpeg","quality":0.75,"max_width":1280}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p alephcore`
Expected: No errors.

- [ ] **Step 7: Commit**

```bash
git add src/builtin_tools/desktop/mod.rs
git commit -m "feat(desktop): integrate session lock and escape abort into DesktopTool lifecycle"
```

---

## Task 6: New action handlers (display_list, batch, paste, screenshot optimization)

**Files:**
- Modify: `src/builtin_tools/desktop/native.rs`

- [ ] **Step 1: Add display_list handler**

Add in the match block of `call_via_platform()`, before `_ => Ok(None)`:

```rust
"display_list" => {
    match screen.display_list().await {
        Ok(displays) => {
            let data: Vec<serde_json::Value> = displays.iter().map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "name": d.name,
                    "width": d.width,
                    "height": d.height,
                    "scale_factor": d.scale_factor,
                    "is_primary": d.is_primary,
                    "origin_x": d.origin_x,
                    "origin_y": d.origin_y,
                })
            }).collect();
            Ok(Some(DesktopOutput {
                success: true,
                data: Some(serde_json::json!({"displays": data})),
                message: None,
            }))
        }
        Err(e) => Ok(Some(DesktopOutput {
            success: false, data: None,
            message: Some(format!("Screen capability error: {e}")),
        })),
    }
}
```

- [ ] **Step 2: Update screenshot handler for optimization params**

Replace the existing `"screenshot"` handler in `call_via_platform()` to support `display_id`, `format`, `quality`, `max_width`, `max_height`:

```rust
"screenshot" => {
    let region = match args.region.as_ref() {
        Some(r) => {
            if r.x < 0.0 || r.y < 0.0 || r.width < 0.0 || r.height < 0.0 {
                return Ok(Some(DesktopOutput {
                    success: false, data: None,
                    message: Some("screenshot region coordinates must be non-negative".into()),
                }));
            }
            if r.x > u32::MAX as f64 || r.y > u32::MAX as f64
                || r.width > u32::MAX as f64 || r.height > u32::MAX as f64
            {
                return Ok(Some(DesktopOutput {
                    success: false, data: None,
                    message: Some("screenshot region coordinates exceed maximum value".into()),
                }));
            }
            Some(aleph_desktop::ScreenRegion {
                x: r.x as u32, y: r.y as u32,
                width: r.width as u32, height: r.height as u32,
            })
        }
        None => None,
    };

    // Capture from specific display or primary
    let screenshot_result = if let Some(did) = args.display_id {
        tokio::task::spawn_blocking(move || {
            aleph_desktop::perception::take_screenshot_display(did, region.as_ref())
        }).await.map_err(|e| crate::error::Error::Internal(format!("task join: {e}")))?
    } else {
        screen.screenshot(region).await
    };

    // Apply post-processing if format/max_width/max_height specified
    let needs_processing = args.format.is_some() || args.max_width.is_some() || args.max_height.is_some();

    match screenshot_result {
        Ok(s) => {
            if needs_processing {
                use base64::Engine;
                let raw_png = base64::engine::general_purpose::STANDARD.decode(&s.image_base64)
                    .map_err(|e| crate::error::Error::Internal(format!("base64 decode: {e}")))?;
                let fmt = args.format.as_deref().unwrap_or("png");
                let quality = args.quality.unwrap_or(0.75);
                match tokio::task::spawn_blocking(move || {
                    aleph_desktop::perception::process_screenshot(
                        &raw_png, args.max_width, args.max_height, fmt, quality,
                    )
                }).await.map_err(|e| crate::error::Error::Internal(format!("task join: {e}")))? {
                    Ok(processed) => Ok(Some(DesktopOutput {
                        success: true,
                        data: Some(serde_json::json!({
                            "image_base64": processed.image_base64,
                            "width": processed.width,
                            "height": processed.height,
                            "format": processed.format,
                        })),
                        message: None,
                    })),
                    Err(e) => Ok(Some(DesktopOutput {
                        success: false, data: None,
                        message: Some(format!("Screenshot processing error: {e}")),
                    })),
                }
            } else {
                Ok(Some(DesktopOutput {
                    success: true,
                    data: Some(serde_json::json!({
                        "image_base64": s.image_base64,
                        "width": s.width,
                        "height": s.height,
                        "format": s.format,
                    })),
                    message: None,
                }))
            }
        }
        Err(e) => Ok(Some(DesktopOutput {
            success: false, data: None,
            message: Some(format!("Screen capability error: {e}")),
        })),
    }
}
```

- [ ] **Step 3: Add batch handler**

The batch handler lives in `call_via_platform()` but needs access to the full `DesktopTool` for recursive dispatch. Since `call_via_platform()` is a method on `DesktopTool`, we have `self` access. However, batch needs to call `self.call()` recursively, which requires care.

Add a separate method on DesktopTool and call it from `call_via_platform`:

In `native.rs`, for the `"batch"` match arm, return `Ok(None)` to signal it should be handled at the `call()` level instead.

In `mod.rs`, update `call()` to handle batch before calling `call_via_platform`:

```rust
// In call(), after escape check and before call_via_platform:
if args.action == "batch" {
    return Ok(self.execute_batch(&args).await?);
}
```

Add `execute_batch` method to DesktopTool:

```rust
async fn execute_batch(&self, args: &DesktopArgs) -> Result<DesktopOutput> {
    let actions = match &args.actions {
        Some(list) if !list.is_empty() => list,
        _ => return Ok(DesktopOutput {
            success: false, data: None,
            message: Some("batch requires non-empty 'actions' array".into()),
        }),
    };

    let mut results = Vec::new();
    for (i, action_json) in actions.iter().enumerate() {
        // Check escape abort between actions
        if let Err(out) = self.check_escape() {
            results.push(serde_json::json!({"index": i, "aborted": true, "message": out.message}));
            break;
        }

        // Deserialize sub-action
        let sub_args: DesktopArgs = match serde_json::from_value(action_json.clone()) {
            Ok(a) => a,
            Err(e) => {
                results.push(serde_json::json!({"index": i, "success": false, "message": format!("Invalid action: {e}")}));
                break;
            }
        };

        // Prevent nested batch
        if sub_args.action == "batch" {
            results.push(serde_json::json!({"index": i, "success": false, "message": "Nested batch not allowed"}));
            break;
        }

        // Execute sub-action (goes through full approval + lock + escape pipeline)
        let output = self.call(sub_args).await?;
        let success = output.success;
        results.push(serde_json::json!({
            "index": i,
            "success": success,
            "data": output.data,
            "message": output.message,
        }));

        if !success {
            break;
        }
    }

    let overall_success = results.last()
        .and_then(|r| r["success"].as_bool())
        .unwrap_or(false);

    Ok(DesktopOutput {
        success: overall_success,
        data: Some(serde_json::json!({"results": results})),
        message: None,
    })
}
```

- [ ] **Step 4: Add paste handler**

Add in `call_via_platform()`:

```rust
"paste" => {
    let text = args.text.as_deref().unwrap_or("");

    // 1. Save current clipboard (best effort)
    let saved = screen.clipboard_read().await.ok();

    // 2. Write target text to clipboard
    if let Err(e) = screen.clipboard_write(text).await {
        return Ok(Some(DesktopOutput {
            success: false, data: None,
            message: Some(format!("Failed to write to clipboard: {e}")),
        }));
    }

    // 3. Cmd+V to paste
    if let Err(e) = screen.key_combo(&["meta".into()], "v").await {
        // Try to restore clipboard before reporting error
        if let Some(ref original) = saved {
            let _ = screen.clipboard_write(original).await;
        }
        return Ok(Some(DesktopOutput {
            success: false, data: None,
            message: Some(format!("Failed to paste: {e}")),
        }));
    }

    // 4. Wait for paste to take effect
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 5. Restore original clipboard (best effort)
    if let Some(original) = saved {
        let _ = screen.clipboard_write(&original).await;
    }

    Ok(Some(DesktopOutput {
        success: true,
        data: Some(serde_json::json!({"pasted": true, "chars": text.chars().count()})),
        message: None,
    }))
}
```

- [ ] **Step 5: Verify compilation and tests**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib -- desktop`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/desktop/
git commit -m "feat(desktop): add display_list, batch, paste handlers and screenshot optimization"
```

---

## Task 7: Final verification and cleanup

**Files:** All modified files from Tasks 1-6.

- [ ] **Step 1: Full workspace build**

Run: `cargo check`
Expected: Entire workspace compiles.

- [ ] **Step 2: All desktop tests**

Run: `cargo test -p aleph-desktop --lib && cargo test -p alephcore --lib -- desktop`
Expected: All tests pass.

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p aleph-desktop -p aleph-desktop-macos -p alephcore -- -D warnings`
Expected: No warnings. Fix any issues.

- [ ] **Step 4: Verify no dead code**

Run: `grep -r "TODO\|FIXME\|HACK" src/builtin_tools/desktop/ desktop/ --include="*.rs" | grep -v "plumb agent_id"`
Expected: No new TODOs (the existing agent_id TODO is pre-existing and acceptable).

- [ ] **Step 5: Commit any cleanup**

```bash
git add -u
git commit -m "chore(desktop): Phase 2 final cleanup"
```
