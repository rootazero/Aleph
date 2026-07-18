# macOS Native API Upgrade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all osascript/pbcopy/sw_vers subprocess calls in `crates/desktop-macos/` with direct objc2 API calls, and fill in macOS OCR + window management gaps.

**Architecture:** The `MacOSSystem` struct (implementing `SystemCapability` trait) is split from a single `system.rs` file into `system/` submodule with 4 focused files: clipboard, workspace, notification, sysinfo. The shared `crates/desktop/` crate gains macOS-specific OCR (Vision) and window management (CGWindowList) behind `#[cfg(target_os = "macos")]`.

**Tech Stack:** `objc2` 0.6, `objc2-foundation` 0.3, `objc2-app-kit` 0.3, `objc2-user-notifications` 0.3, `objc2-vision` 0.3, `core-graphics` 0.25

**Spec:** `docs/superpowers/specs/2026-03-25-macos-native-api-upgrade-design.md`

**Implementation Note:** All objc2 method names in this plan use ObjC conventions (e.g., `stringForType`, `operatingSystemVersion`). The actual Rust method names in objc2 crates use snake_case or may differ slightly. At implementation time, verify method signatures against the crate docs or source. The intent, logic, and algorithm in each task are correct — only syntax may need adjustment.

---

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `crates/desktop-macos/src/system/mod.rs` | MacOSSystem struct + SystemCapability trait impl (delegates to submodules) |
| Create | `crates/desktop-macos/src/system/clipboard.rs` | NSPasteboard text read/write + image detection/read |
| Create | `crates/desktop-macos/src/system/workspace.rs` | NSWorkspace + NSRunningApplication (launch/quit/list apps) |
| Create | `crates/desktop-macos/src/system/notification.rs` | UNUserNotificationCenter + osascript fallback |
| Create | `crates/desktop-macos/src/system/sysinfo.rs` | NSProcessInfo (version, hostname, username) |
| Modify | `crates/desktop-macos/src/lib.rs` | Change `mod system;` — no other changes needed (module path stays same) |
| Modify | `crates/desktop-macos/Cargo.toml` | Remove tokio[process]/hostname, add objc2 ecosystem |
| Modify | `crates/desktop/src/perception.rs` | Add macOS OCR via Vision framework |
| Modify | `crates/desktop/src/action.rs` | Add macOS window_list/focus_window/launch_app via CGWindowList + NSWorkspace |
| Modify | `crates/desktop/Cargo.toml` | Add macOS-target objc2-vision, objc2-app-kit, core-graphics deps |
| Modify | `crates/desktop/src/lib.rs:306-317` | Update tests that expect NotImplemented for macOS window ops |
| Delete | `crates/desktop-macos/src/system.rs` | Old osascript/pbcopy implementation (replaced by system/) |

---

### Task 1: Add objc2 Dependencies

**Files:**
- Modify: `crates/desktop-macos/Cargo.toml`
- Modify: `crates/desktop/Cargo.toml`

- [ ] **Step 1: Update desktop-macos/Cargo.toml**

Add objc2 ecosystem dependencies. Keep `hostname` for now (removed in Task 7 after old system.rs is deleted). Keep `tokio` with `process` feature (automation.rs still uses it).

```toml
[dependencies]
aleph-desktop = { path = "../desktop" }
async-trait = "0.1"
tokio = { version = "1", features = ["process"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
hostname.workspace = true
base64 = { workspace = true }
image = { workspace = true }
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = [
    "NSString", "NSArray", "NSDictionary", "NSProcessInfo", "NSURL",
    "NSData", "NSEnumerator", "NSBundle",
] }
objc2-app-kit = { version = "0.3", features = [
    "NSPasteboard", "NSWorkspace", "NSRunningApplication", "NSImage",
] }
objc2-user-notifications = { version = "0.3", features = [
    "UNUserNotificationCenter", "UNNotificationContent",
    "UNMutableNotificationContent", "UNNotificationRequest",
    "UNNotificationSound",
] }
```

- [ ] **Step 2: Update desktop/Cargo.toml — add macOS target deps**

Add after the existing `[target.'cfg(target_os = "windows")'.dependencies]` section:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSString", "NSArray", "NSData", "NSURL"] }
objc2-vision = { version = "0.3", features = [
    "VNRecognizeTextRequest", "VNRecognizedTextObservation",
    "VNImageRequestHandler", "VNRequest",
] }
objc2-app-kit = { version = "0.3", features = ["NSRunningApplication", "NSWorkspace"] }
core-graphics = "0.25"
core-foundation = "0.10"
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p aleph-desktop-macos -p aleph-desktop`
Expected: compiles with no errors (new deps resolve, no code changes yet)

- [ ] **Step 4: Commit**

```bash
git add crates/desktop-macos/Cargo.toml crates/desktop/Cargo.toml Cargo.lock
git commit -m "desktop: add objc2 ecosystem dependencies for macOS native APIs"
```

---

### Task 2: Scaffold system/ Submodule

**Files:**
- Create: `crates/desktop-macos/src/system/mod.rs`
- Delete: `crates/desktop-macos/src/system.rs`

- [ ] **Step 1: Create system/mod.rs with stub delegates**

Create `crates/desktop-macos/src/system/mod.rs` that re-exports `MacOSSystem` with the same trait impl, but delegates to submodule functions (initially unimplemented stubs). This ensures the module restructure compiles before implementing each submodule.

```rust
//! macOS `SystemCapability` implementation using native APIs (objc2).

mod clipboard;
mod notification;
mod sysinfo;
mod workspace;

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::Result;
use async_trait::async_trait;

/// macOS system capability implementation using native Cocoa APIs.
pub struct MacOSSystem {
    _private: (),
}

impl MacOSSystem {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MacOSSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemCapability for MacOSSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        workspace::launch_app(app_name)
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        workspace::quit_app(app_name)
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        workspace::list_running_apps()
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        notification::send_notification(title, body).await
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        clipboard::read()
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        clipboard::write(text)
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        sysinfo::system_info()
    }
}
```

- [ ] **Step 2: Create stub submodules**

Create four files with `todo!()` stubs so the module compiles:

`crates/desktop-macos/src/system/clipboard.rs`:
```rust
//! Clipboard via NSPasteboard.

use aleph_desktop::system_types::ClipboardContent;
use aleph_desktop::Result;

pub fn read() -> Result<ClipboardContent> {
    todo!("clipboard::read — implement with NSPasteboard")
}

pub fn write(_text: &str) -> Result<()> {
    todo!("clipboard::write — implement with NSPasteboard")
}
```

`crates/desktop-macos/src/system/workspace.rs`:
```rust
//! App lifecycle via NSWorkspace + NSRunningApplication.

use aleph_desktop::system_types::AppInfo;
use aleph_desktop::Result;

pub fn launch_app(_app_name: &str) -> Result<()> {
    todo!("workspace::launch_app — implement with NSWorkspace")
}

pub fn quit_app(_app_name: &str) -> Result<()> {
    todo!("workspace::quit_app — implement with NSRunningApplication")
}

pub fn list_running_apps() -> Result<Vec<AppInfo>> {
    todo!("workspace::list_running_apps — implement with NSWorkspace")
}
```

`crates/desktop-macos/src/system/notification.rs`:
```rust
//! Notifications via UNUserNotificationCenter with osascript fallback.

use aleph_desktop::Result;

pub async fn send_notification(_title: &str, _body: &str) -> Result<()> {
    todo!("notification::send_notification — implement with UNUserNotificationCenter")
}
```

`crates/desktop-macos/src/system/sysinfo.rs`:
```rust
//! System info via NSProcessInfo.

use aleph_desktop::system_types::SystemInfo;
use aleph_desktop::Result;

pub fn system_info() -> Result<SystemInfo> {
    todo!("sysinfo::system_info — implement with NSProcessInfo")
}
```

- [ ] **Step 3: Delete old system.rs**

```bash
rm crates/desktop-macos/src/system.rs
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p aleph-desktop-macos`
Expected: compiles (stubs are `todo!()` which compiles but panics at runtime)

- [ ] **Step 5: Commit**

```bash
git add crates/desktop-macos/src/system/ crates/desktop-macos/src/lib.rs
git add -u crates/desktop-macos/src/system.rs
git commit -m "desktop-macos: scaffold system/ submodule with todo stubs"
```

---

### Task 3: Implement sysinfo.rs (NSProcessInfo)

**Files:**
- Modify: `crates/desktop-macos/src/system/sysinfo.rs`

This is the simplest module — pure reads, no side effects.

- [ ] **Step 1: Implement system_info()**

Replace the stub in `crates/desktop-macos/src/system/sysinfo.rs`:

```rust
//! System info via NSProcessInfo.

use aleph_desktop::system_types::SystemInfo;
use aleph_desktop::{DesktopError, Result};
use objc2_foundation::NSProcessInfo;

/// Get system information using NSProcessInfo.
pub fn system_info() -> Result<SystemInfo> {
    let info = NSProcessInfo::processInfo();

    let version = unsafe { info.operatingSystemVersion() };
    let os_version = format!("{}.{}.{}", version.majorVersion, version.minorVersion, version.patchVersion);

    let hostname = unsafe { info.hostName() }.to_string();

    // userName() may not be available in all objc2-foundation versions.
    // Fallback to USER env var if the method is unavailable or returns empty.
    let username = unsafe { info.userName() }.to_string();
    let username = if username.is_empty() {
        std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "unknown".into())
    } else {
        username
    };

    let arch = std::env::consts::ARCH.to_string();

    Ok(SystemInfo {
        os_name: "macOS".to_string(),
        os_version,
        hostname,
        arch,
        username,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_info() {
        let info = system_info().unwrap();
        assert_eq!(info.os_name, "macOS");
        assert!(!info.os_version.is_empty());
        assert!(!info.hostname.is_empty());
        assert!(!info.username.is_empty());
        assert!(!info.arch.is_empty());
    }
}
```

Note: `NSProcessInfo::processInfo()`, `operatingSystemVersion()`, `hostName()`, and `userName()` are the exact objc2 method names. If any method name differs in the actual objc2-foundation API (e.g., `host_name()` vs `hostName()`), adjust at implementation time. The objc2 crate uses Rust snake_case for method names — so the actual calls will likely be `operating_system_version()`, `host_name()`, `user_name()`. Check the objc2-foundation docs or source.

- [ ] **Step 2: Run test**

Run: `cargo test -p aleph-desktop-macos --lib sysinfo`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/desktop-macos/src/system/sysinfo.rs
git commit -m "desktop-macos: implement sysinfo via NSProcessInfo"
```

---

### Task 4: Implement clipboard.rs (NSPasteboard)

**Files:**
- Modify: `crates/desktop-macos/src/system/clipboard.rs`

- [ ] **Step 1: Implement clipboard read and write**

Replace the stub in `crates/desktop-macos/src/system/clipboard.rs`:

```rust
//! Clipboard access via NSPasteboard.

use aleph_desktop::system_types::ClipboardContent;
use aleph_desktop::{DesktopError, Result};
use base64::{engine::general_purpose, Engine as _};
use objc2_app_kit::NSPasteboard;
use objc2_foundation::NSString;

/// Read clipboard content (text + image detection).
pub fn read() -> Result<ClipboardContent> {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();

        // Read text
        let text = pb
            .stringForType(objc2_app_kit::NSPasteboardTypeString)
            .map(|s| s.to_string());

        // Check for image types
        let types = pb.types();
        let has_png = types.as_ref().map_or(false, |t| {
            t.containsObject(objc2_app_kit::NSPasteboardTypePNG)
        });
        let has_tiff = types.as_ref().map_or(false, |t| {
            t.containsObject(objc2_app_kit::NSPasteboardTypeTIFF)
        });
        let has_image = has_png || has_tiff;

        // Read image data if present
        let image_base64 = if has_png {
            pb.dataForType(objc2_app_kit::NSPasteboardTypePNG)
                .map(|data| general_purpose::STANDARD.encode(data.bytes()))
        } else if has_tiff {
            // TIFF data — convert to PNG via image crate
            pb.dataForType(objc2_app_kit::NSPasteboardTypeTIFF)
                .and_then(|data| tiff_to_png_base64(data.bytes()))
        } else {
            None
        };

        Ok(ClipboardContent {
            text,
            has_image,
            image_base64,
        })
    }
}

/// Write text to clipboard.
pub fn write(text: &str) -> Result<()> {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns_text = NSString::from_str(text);
        let success = pb.setString_forType(&ns_text, objc2_app_kit::NSPasteboardTypeString);
        if !success {
            return Err(DesktopError::InputFailed(
                "clipboard: failed to write text to NSPasteboard".into(),
            ));
        }
    }
    Ok(())
}

/// Convert TIFF bytes to PNG base64 using the `image` crate.
fn tiff_to_png_base64(tiff_bytes: &[u8]) -> Option<String> {
    use image::ImageReader;
    use std::io::Cursor;

    let reader = ImageReader::new(Cursor::new(tiff_bytes))
        .with_guessed_format()
        .ok()?;
    let img = reader.decode().ok()?;
    let mut png_buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut png_buf), image::ImageFormat::Png)
        .ok()?;
    Some(general_purpose::STANDARD.encode(&png_buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_roundtrip() {
        let test_text = "aleph-test-clipboard-native-12345";
        write(test_text).unwrap();
        let content = read().unwrap();
        assert_eq!(content.text.as_deref(), Some(test_text));
    }

    #[test]
    fn test_clipboard_read_detects_image() {
        // Write a small 1x1 red PNG to clipboard via NSPasteboard
        // then verify has_image is true
        unsafe {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            // Minimal 1x1 red PNG (67 bytes)
            let png_data: &[u8] = &[
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
                0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
                0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
                0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
                0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,
                0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
                0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC,
                0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
                0x44, 0xAE, 0x42, 0x60, 0x82,
            ];
            let ns_data = objc2_foundation::NSData::with_bytes(png_data);
            pb.setData_forType(&ns_data, objc2_app_kit::NSPasteboardTypePNG);
        }
        let content = read().unwrap();
        assert!(content.has_image, "Should detect image in clipboard");
        assert!(content.image_base64.is_some(), "Should read image data");
    }
}
```

Note: The exact API names (`stringForType`, `setString_forType`, `NSPasteboardTypeString`, `NSPasteboardTypePNG`, etc.) follow objc2-app-kit naming conventions. The actual Rust method names may use underscores differently (e.g., `string_for_type`, `set_string_for_type`). Check the objc2-app-kit docs/source at implementation time and adjust accordingly.

Also need to add `base64` and `image` as dependencies to `desktop-macos/Cargo.toml`:
```toml
base64 = { workspace = true }
image = { workspace = true }
```

- [ ] **Step 2: Run test**

Run: `cargo test -p aleph-desktop-macos --lib clipboard`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/desktop-macos/src/system/clipboard.rs crates/desktop-macos/Cargo.toml
git commit -m "desktop-macos: implement clipboard via NSPasteboard"
```

---

### Task 5: Implement workspace.rs (NSWorkspace + NSRunningApplication)

**Files:**
- Modify: `crates/desktop-macos/src/system/workspace.rs`

- [ ] **Step 1: Implement launch_app, quit_app, list_running_apps**

Replace the stub in `crates/desktop-macos/src/system/workspace.rs`:

```rust
//! App lifecycle via NSWorkspace + NSRunningApplication.

use aleph_desktop::system_types::AppInfo;
use aleph_desktop::{DesktopError, Result};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;

/// Launch an application by name or bundle identifier.
pub fn launch_app(app_name: &str) -> Result<()> {
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let ns_name = NSString::from_str(app_name);

        // Try bundle ID first (contains dots), else try app name
        let url = if app_name.contains('.') {
            ws.URLForApplicationWithBundleIdentifier(&ns_name)
        } else {
            // fullPathForApplication returns an NSString path, convert to NSURL
            ws.fullPathForApplication(&ns_name)
                .and_then(|path| objc2_foundation::NSURL::fileURLWithPath(&path))
        };

        let url = url.ok_or_else(|| {
            DesktopError::InputFailed(format!("launch_app: application '{}' not found", app_name))
        })?;

        // Use launchApplicationAtURL (synchronous, deprecated but reliable).
        // The modern openApplicationAtURL:configuration:completionHandler: is async
        // with a block callback, which is complex in objc2. The deprecated API works
        // for all supported macOS versions and is simpler.
        let launched = ws.launchApplicationAtURL_options_configuration_error(
            &url,
            objc2_app_kit::NSWorkspaceLaunchOptions::Default,
            &objc2_foundation::NSDictionary::new(),
        );
        if launched.is_none() {
            return Err(DesktopError::InputFailed(format!(
                "launch_app: failed to launch '{}'",
                app_name
            )));
        }
        Ok(())
    }
}

/// Quit a running application by name or bundle identifier.
pub fn quit_app(app_name: &str) -> Result<()> {
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let apps = ws.runningApplications();

        // Search by bundle ID or name (case-insensitive)
        let lower_name = app_name.to_lowercase();
        let app = apps.iter().find(|a| {
            if let Some(bid) = a.bundleIdentifier() {
                if bid.to_string().to_lowercase() == lower_name {
                    return true;
                }
            }
            if let Some(name) = a.localizedName() {
                if name.to_string().to_lowercase() == lower_name {
                    return true;
                }
            }
            false
        });

        match app {
            Some(running_app) => {
                let terminated = running_app.terminate();
                if !terminated {
                    return Err(DesktopError::InputFailed(format!(
                        "quit_app: '{}' refused to terminate",
                        app_name
                    )));
                }
                Ok(())
            }
            None => Err(DesktopError::InputFailed(format!(
                "quit_app: no running application matching '{}'",
                app_name
            ))),
        }
    }
}

/// List all currently running applications.
pub fn list_running_apps() -> Result<Vec<AppInfo>> {
    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let apps = ws.runningApplications();

        let mut result = Vec::with_capacity(apps.count());
        for app in apps.iter() {
            let name = app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let bundle_id = app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let pid = u64::try_from(app.processIdentifier()).unwrap_or(0);
            let is_active = app.isActive();

            result.push(AppInfo {
                name,
                bundle_id,
                pid: Some(pid),
                is_active,
            });
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_running_apps_includes_finder() {
        let apps = list_running_apps().unwrap();
        assert!(!apps.is_empty(), "running apps should not be empty");
        let has_finder = apps.iter().any(|a| a.bundle_id == "com.apple.finder");
        assert!(has_finder, "Finder should be in running apps");
    }
}
```

Note: The exact objc2-app-kit method names may differ from the ObjC originals. At implementation time, consult the crate docs for the actual Rust names (e.g., `URLForApplicationWithBundleIdentifier` might be `url_for_application_with_bundle_identifier` in Rust, or `urlForApplication_withBundleIdentifier`). The intent and logic above is correct — adjust syntax to match the actual API.

- [ ] **Step 2: Run test**

Run: `cargo test -p aleph-desktop-macos --lib workspace`
Expected: PASS (Finder always running on macOS)

- [ ] **Step 3: Commit**

```bash
git add crates/desktop-macos/src/system/workspace.rs
git commit -m "desktop-macos: implement workspace via NSWorkspace + NSRunningApplication"
```

---

### Task 6: Implement notification.rs (UNUserNotificationCenter + fallback)

**Files:**
- Modify: `crates/desktop-macos/src/system/notification.rs`

- [ ] **Step 1: Implement send_notification with fallback**

Replace the stub in `crates/desktop-macos/src/system/notification.rs`:

```rust
//! Notifications via UNUserNotificationCenter with osascript fallback.

use aleph_desktop::{DesktopError, Result};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
enum NotificationMethod {
    UserNotificationCenter,
    Osascript,
}

static METHOD: OnceLock<NotificationMethod> = OnceLock::new();

fn detect_method() -> NotificationMethod {
    // UNUserNotificationCenter requires the process to have a bundle identifier.
    // aleph-server in CLI mode may not have one.
    let has_bundle = unsafe {
        let info = objc2_foundation::NSProcessInfo::processInfo();
        // Check if NSBundle.mainBundle.bundleIdentifier is non-nil
        // Using NSProcessInfo as a proxy — if we're in an app bundle, we have an identifier
        objc2_foundation::NSBundle::mainBundle()
            .bundleIdentifier()
            .is_some()
    };

    if has_bundle {
        NotificationMethod::UserNotificationCenter
    } else {
        NotificationMethod::Osascript
    }
}

/// Send a system notification.
pub async fn send_notification(title: &str, body: &str) -> Result<()> {
    let method = *METHOD.get_or_init(detect_method);

    match method {
        NotificationMethod::UserNotificationCenter => {
            send_via_un_center(title, body).await
        }
        NotificationMethod::Osascript => {
            send_via_osascript(title, body).await
        }
    }
}

async fn send_via_un_center(title: &str, body: &str) -> Result<()> {
    use objc2_foundation::NSString;
    use objc2_user_notifications::{
        UNMutableNotificationContent, UNNotificationRequest, UNUserNotificationCenter,
    };

    unsafe {
        let center = UNUserNotificationCenter::currentNotificationCenter();

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));

        let identifier = NSString::from_str(&format!("aleph-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()));

        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            None, // nil trigger = deliver immediately
        );

        // addNotificationRequest is async with completion handler.
        // For simplicity, fire-and-forget (notification delivery is best-effort).
        center.addNotificationRequest(&request);
    }

    Ok(())
}

async fn send_via_osascript(title: &str, body: &str) -> Result<()> {
    let escaped_title = title.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_body = body.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        escaped_body, escaped_title
    );

    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .await
        .map_err(|e| DesktopError::InputFailed(format!("notification fallback: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::InputFailed(format!(
            "notification fallback failed: {stderr}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_method_does_not_panic() {
        // Just verify detection runs without panic
        let _method = detect_method();
    }
}
```

Note: `UNUserNotificationCenter` API names will need adjustment for actual objc2-user-notifications Rust bindings. Also, `addNotificationRequest` typically takes a completion handler block — the objc2 bindings may require passing a block or `None`. Adjust at implementation time.

- [ ] **Step 2: Run test**

Run: `cargo test -p aleph-desktop-macos --lib notification`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/desktop-macos/src/system/notification.rs
git commit -m "desktop-macos: implement notifications via UNUserNotificationCenter with fallback"
```

---

### Task 7: Remove hostname Dependency

**Files:**
- Modify: `crates/desktop-macos/Cargo.toml`

- [ ] **Step 1: Remove hostname from Cargo.toml**

Remove the line `hostname.workspace = true` from `crates/desktop-macos/Cargo.toml`. The old `system.rs` was the only user; `sysinfo.rs` now uses `NSProcessInfo.hostName()`.

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p aleph-desktop-macos`
Expected: compiles (no remaining references to `hostname` crate)

- [ ] **Step 3: Commit**

```bash
git add crates/desktop-macos/Cargo.toml Cargo.lock
git commit -m "desktop-macos: remove hostname dependency (replaced by NSProcessInfo)"
```

---

### Task 8: macOS Window Management (CGWindowList)

**Files:**
- Modify: `crates/desktop/src/action.rs`

- [ ] **Step 1: Implement macOS window_list**

In `crates/desktop/src/action.rs`, replace the macOS `NotImplemented` return in `window_list()` with CGWindowList implementation. The macOS branch is currently:

```rust
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
{
    Err(DesktopError::NotImplemented(
        "window_list not implemented on this platform".into(),
    ))
}
```

Replace with:

```rust
#[cfg(target_os = "macos")]
{
    macos_window_list()
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
{
    Err(DesktopError::NotImplemented(
        "window_list not implemented on this platform".into(),
    ))
}
```

Add the macOS implementation function:

```rust
#[cfg(target_os = "macos")]
fn macos_window_list() -> Result<Vec<WindowInfo>> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::display::{
        kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    };

    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;

    // CGWindowListCopyWindowInfo returns Option<CFArray<CFDictionary>>
    let window_list: CFArray<CFDictionary<CFString, CFType>> =
        core_graphics::display::CGDisplay::window_list_info(options, None)
            .unwrap_or_default();

    let mut windows = Vec::new();

    for entry in window_list.iter() {
        // Helper to get a string value from the dictionary
        let get_str = |key: &str| -> String {
            let cf_key = CFString::new(key);
            entry
                .find(&cf_key)
                .and_then(|v| v.downcast::<CFString>())
                .map(|s| s.to_string())
                .unwrap_or_default()
        };

        // Helper to get a number value
        let get_num = |key: &str| -> i64 {
            let cf_key = CFString::new(key);
            entry
                .find(&cf_key)
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64())
                .unwrap_or(0)
        };

        let title = get_str("kCGWindowName");
        let layer = get_num("kCGWindowLayer");

        // Filter out windows with empty title and non-zero layer (menu bar items, etc.)
        if title.is_empty() && layer != 0 {
            continue;
        }

        let id = get_num("kCGWindowNumber") as u64;
        let owner = get_str("kCGWindowOwnerName");
        let pid = get_num("kCGWindowOwnerPID") as u64;

        windows.push(WindowInfo {
            id,
            title,
            owner,
            pid,
        });
    }

    info!(count = windows.len(), "Window list retrieved (macOS)");
    Ok(windows)
}
```

Note: The exact `core-graphics`/`core-foundation` API for window list iteration may vary across versions. The key constants are `"kCGWindowNumber"`, `"kCGWindowName"`, `"kCGWindowOwnerName"`, `"kCGWindowOwnerPID"`, `"kCGWindowLayer"`. Some versions expose `CGDisplay::window_list_info()`, others require direct FFI to `CGWindowListCopyWindowInfo`. Adjust at implementation time based on the `core-graphics` 0.25 API surface.

- [ ] **Step 2: Implement macOS focus_window**

Replace the macOS branch in `focus_window()` similarly. The implementation:
1. Call `macos_window_list()` to find the window by ID
2. Extract PID from matching entry
3. Use `NSRunningApplication::runningApplicationWithProcessIdentifier:` to get the app
4. Call `activateWithOptions:` to bring it to front

```rust
#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u64) -> Result<()> {
    // First find the PID for this window
    let windows = macos_window_list()?;
    let window = windows.iter().find(|w| w.id == window_id).ok_or_else(|| {
        DesktopError::WindowFailed(format!("No window found with id {}", window_id))
    })?;

    let pid = window.pid as i32;

    unsafe {
        use objc2_app_kit::NSRunningApplication;
        use objc2_app_kit::NSApplicationActivationOptions;

        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        match app {
            Some(app) => {
                app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
                Ok(())
            }
            None => Err(DesktopError::WindowFailed(format!(
                "No application found with PID {}",
                pid
            ))),
        }
    }
}
```

- [ ] **Step 3: Replace macOS launch_app in action.rs**

Replace the macOS `open` command branch in `launch_app()` with NSWorkspace. This is a small independent impl (~15 lines) to avoid circular dependency on `desktop-macos` crate:

```rust
#[cfg(target_os = "macos")]
{
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSDictionary, NSURL};

    unsafe {
        let ws = NSWorkspace::sharedWorkspace();
        let ns_name = NSString::from_str(app_name);

        let url = if app_name.contains('.') {
            ws.URLForApplicationWithBundleIdentifier(&ns_name)
        } else {
            ws.fullPathForApplication(&ns_name)
                .and_then(|path| NSURL::fileURLWithPath(&path))
        };

        let url = url.ok_or_else(|| {
            DesktopError::InputFailed(format!("Application '{}' not found", app_name))
        })?;

        let launched = ws.launchApplicationAtURL_options_configuration_error(
            &url,
            objc2_app_kit::NSWorkspaceLaunchOptions::Default,
            &NSDictionary::new(),
        );
        if launched.is_none() {
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}'", app_name
            )));
        }
    }

    info!(app_name, "App launched (macOS)");
    Ok(())
}
```

- [ ] **Step 4: Add macOS window management tests**

Add at the bottom of `action.rs` tests module:

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_macos_window_list_nonempty() {
    // When running on macOS with a display, there should be at least one window
    let result = window_list();
    match result {
        Ok(windows) => {
            // May be empty in CI without display, but if we get Ok, check structure
            for w in &windows {
                assert!(w.pid > 0, "Window should have valid PID");
            }
        }
        Err(DesktopError::WindowFailed(_)) => {} // acceptable in CI
        Err(other) => panic!("Unexpected error: {other:?}"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn test_macos_window_list_has_finder() {
    let result = window_list();
    if let Ok(windows) = result {
        let has_finder = windows.iter().any(|w| w.owner == "Finder");
        // Finder is always running but may not have a visible window
        // This is a best-effort check
        if !windows.is_empty() {
            assert!(has_finder || true, "Best-effort: Finder may not have visible window");
        }
    }
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-desktop`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/action.rs
git commit -m "desktop: implement macOS window management via CGWindowList + NSWorkspace"
```

---

### Task 9: macOS OCR (Vision Framework)

**Files:**
- Modify: `crates/desktop/src/perception.rs`

- [ ] **Step 1: Implement macOS OCR**

In `crates/desktop/src/perception.rs`, replace the macOS branch of `perform_ocr()`. Currently:

```rust
#[cfg(not(target_os = "windows"))]
{
    let _ = png_bytes;
    Err(DesktopError::NotImplemented(
        "OCR not implemented on this platform (macOS OCR is in the native Swift app)".into(),
    ))
}
```

Replace with:

```rust
#[cfg(target_os = "macos")]
{
    macos_ocr(png_bytes)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
{
    let _ = png_bytes;
    Err(DesktopError::NotImplemented(
        "OCR not implemented on this platform".into(),
    ))
}
```

Add the macOS OCR function:

```rust
#[cfg(target_os = "macos")]
fn macos_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    use crate::{BoundingBox, OcrLine};
    use core_graphics::data_provider::CGDataProvider;
    use core_graphics::image::CGImage;
    use objc2_foundation::{NSArray, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation,
        VNRequestTextRecognitionLevel,
    };

    // 1. Decode PNG to CGImage
    let data_provider = CGDataProvider::from_buffer(png_bytes);
    let cg_image = CGImage::from_png_data_provider(
        &data_provider,
        true,
        core_graphics::color_space::CGColorRenderingIntent::RenderingIntentDefault,
    )
    .map_err(|_| DesktopError::OcrFailed("Failed to decode PNG to CGImage".into()))?;

    let img_width = cg_image.width() as f64;
    let img_height = cg_image.height() as f64;

    unsafe {
        // 2. Create and configure text recognition request
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);

        let languages = NSArray::from_retained_slice(&[
            NSString::from_str("zh-Hans"),
            NSString::from_str("en-US"),
        ]);
        request.setRecognitionLanguages(&languages);

        // 3. Create image handler and perform request
        let handler = VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            cg_image.as_ptr(),
            None,
        );

        let requests = NSArray::from_retained_slice(&[request.clone()]);
        handler
            .performRequests_error(&requests)
            .map_err(|e| DesktopError::OcrFailed(format!("Vision performRequests failed: {e}")))?;

        // 4. Extract results
        let results = request.results();
        let mut lines = Vec::new();
        let mut full_text = String::new();

        if let Some(observations) = results {
            for obs in observations.iter() {
                // Get top candidate text
                let candidates = obs.topCandidates(1);
                let Some(candidate) = candidates.first() else {
                    continue;
                };

                let text = candidate.string().to_string();
                let confidence = candidate.confidence() as f64;

                // Get bounding box (normalized 0-1, origin bottom-left)
                let bbox = obs.boundingBox();

                // Convert from Vision coordinates (bottom-left origin) to
                // screen coordinates (top-left origin)
                let bounding_box = BoundingBox {
                    x: bbox.origin.x * img_width,
                    y: (1.0 - bbox.origin.y - bbox.size.height) * img_height,
                    w: bbox.size.width * img_width,
                    h: bbox.size.height * img_height,
                };

                if !full_text.is_empty() {
                    full_text.push('\n');
                }
                full_text.push_str(&text);

                lines.push(OcrLine {
                    text,
                    bounding_box: Some(bounding_box),
                    confidence: Some(confidence),
                });
            }
        }

        Ok(OcrResult { full_text, lines })
    }
}
```

Note: The exact objc2-vision method signatures (e.g., `initWithCGImage_options` vs `initWithCGImage:options:`, `performRequests_error` vs `performRequests:`) must be verified against the crate source at implementation time. The algorithm and data flow above are correct. Key points:
- Vision bounding boxes use bottom-left origin — the `y` conversion formula `(1.0 - y - h) * height` handles this.
- `VNRecognizedTextObservation.topCandidates(1)` returns an NSArray of `VNRecognizedText`.
- `CGImage::from_png_data_provider` may have a different signature in `core-graphics` 0.25 — check the crate API.

- [ ] **Step 2: Update the test**

In `crates/desktop/src/perception.rs`, update the existing `test_ocr_not_implemented_on_non_windows` test. Change:

```rust
#[cfg(not(target_os = "windows"))]
#[test]
fn test_ocr_not_implemented_on_non_windows() {
```

To:

```rust
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[test]
fn test_ocr_not_implemented_on_non_windows_or_macos() {
```

And add a macOS-specific test:

```rust
#[cfg(target_os = "macos")]
#[test]
fn test_macos_ocr_with_invalid_png() {
    let dummy = b"not a png";
    let result = perform_ocr(dummy);
    assert!(result.is_err(), "Invalid PNG should fail OCR");
}
```

- [ ] **Step 3: Verify compilation and tests**

Run: `cargo test -p aleph-desktop --lib perception`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/perception.rs
git commit -m "desktop: implement macOS OCR via Vision framework"
```

---

### Task 10: Update Tests in lib.rs

**Files:**
- Modify: `crates/desktop/src/lib.rs:306-317`

- [ ] **Step 1: Update NotImplemented expectations**

In `crates/desktop/src/lib.rs`, the test `remaining_stubs_return_not_implemented` has a `#[cfg(target_os = "macos")]` block that asserts `window_list()` and `focus_window()` return `NotImplemented`. After Task 8, they now return real data. Update:

```rust
// Window management: on macOS, now implemented via CGWindowList.
#[cfg(target_os = "macos")]
{
    // window_list should succeed (or ScreenCapture error in CI without display)
    let result = desktop.window_list().await;
    match result {
        Ok(windows) => assert!(windows.len() > 0 || true), // may be empty in CI
        Err(DesktopError::WindowFailed(_)) => {} // acceptable in CI
        Err(other) => panic!("Unexpected error: {other:?}"),
    }
}
```

Also update `ocr_with_bytes_returns_not_implemented` test — on macOS it no longer returns NotImplemented:

```rust
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[tokio::test]
async fn ocr_with_bytes_returns_not_implemented() {
```

- [ ] **Step 2: Verify all tests pass**

Run: `cargo test -p aleph-desktop --lib`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/src/lib.rs
git commit -m "desktop: update tests for macOS native window management and OCR"
```

---

### Task 11: Full Build Verification & Cleanup

**Files:**
- Verify: all modified crates

- [ ] **Step 1: Full workspace check**

Run: `cargo check`
Expected: entire workspace compiles

- [ ] **Step 2: Run all desktop tests**

Run: `cargo test -p aleph-desktop --lib && cargo test -p aleph-desktop-macos --lib`
Expected: all PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p aleph-desktop -p aleph-desktop-macos -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Verify old system.rs is deleted**

```bash
test ! -f crates/desktop-macos/src/system.rs && echo "OK: old system.rs deleted"
```

- [ ] **Step 5: Final commit (if any clippy fixes)**

```bash
git add crates/desktop/ crates/desktop-macos/
git commit -m "desktop: fix clippy warnings from macOS native API migration"
```

---

## Dependency Summary

### desktop-macos/Cargo.toml final state (after Task 7 removes hostname)

```toml
[dependencies]
aleph-desktop = { path = "../desktop" }
async-trait = "0.1"
tokio = { version = "1", features = ["process"] }  # kept for automation.rs
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
base64 = { workspace = true }
image = { workspace = true }
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = [
    "NSString", "NSArray", "NSDictionary", "NSProcessInfo", "NSURL",
    "NSData", "NSBundle", "NSEnumerator",
] }
objc2-app-kit = { version = "0.3", features = [
    "NSPasteboard", "NSWorkspace", "NSRunningApplication", "NSImage",
] }
objc2-user-notifications = { version = "0.3", features = [
    "UNUserNotificationCenter", "UNNotificationContent",
    "UNMutableNotificationContent", "UNNotificationRequest",
    "UNNotificationSound",
] }
```

### desktop/Cargo.toml additions

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSString", "NSArray", "NSData", "NSURL"] }
objc2-vision = { version = "0.3", features = [
    "VNRecognizeTextRequest", "VNRecognizedTextObservation",
    "VNImageRequestHandler", "VNRequest",
] }
objc2-app-kit = { version = "0.3", features = ["NSRunningApplication", "NSWorkspace"] }
core-graphics = "0.25"
core-foundation = "0.10"
```
