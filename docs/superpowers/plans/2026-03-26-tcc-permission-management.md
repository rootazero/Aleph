# TCC Permission Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add macOS TCC permission detection and request infrastructure as a 5th DesktopPlatform capability, exposed as a `permission` tool for the LLM.

**Architecture:** New `PermissionCapability` trait in `crates/desktop/src/traits/`, macOS implementation using objc2 + block2 + C FFI in `crates/desktop-macos/src/permission.rs`, `PermissionTool` in core. DesktopPlatform extended with `fn permission()`. Linux/Windows return `None`.

**Tech Stack:** `objc2` 0.6, `objc2-av-foundation` 0.3, `objc2-speech` 0.3, `block2` 0.6, C FFI for CoreGraphics/ApplicationServices

**Spec:** `docs/superpowers/specs/2026-03-26-tcc-permission-management-design.md`

**Implementation Note:** All objc2 method names in this plan use the crate's actual camelCase conventions (verified against crate source). C FFI functions use their exact macOS API names.

---

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `crates/desktop/src/permission_types.rs` | TccPermission, PermissionStatus, PermissionInfo |
| Create | `crates/desktop/src/traits/permission.rs` | PermissionCapability trait |
| Create | `crates/desktop-macos/src/permission.rs` | macOS TCC implementation (objc2 + C FFI) |
| Create | `src/builtin_tools/permission_tool.rs` | LLM-facing permission tool |
| Modify | `crates/desktop/src/traits/mod.rs` | Add `pub mod permission` + re-export |
| Modify | `crates/desktop/src/lib.rs` | Add `pub mod permission_types` |
| Modify | `crates/desktop/src/platform.rs` | Add `fn permission()` to DesktopPlatform |
| Modify | `crates/desktop-macos/src/lib.rs` | Wire MacOSPermission into MacOSPlatform |
| Modify | `crates/desktop-macos/Cargo.toml` | Add block2, objc2-av-foundation, objc2-speech |
| Modify | `crates/desktop-linux/src/lib.rs` | Return None for permission() |
| Modify | `crates/desktop-windows/src/lib.rs` | Return None for permission() |
| Modify | `src/builtin_tools/mod.rs` | Add `pub mod permission_tool` |
| Modify | `src/executor/builtin_registry/builder.rs` | Instantiate + register PermissionTool |

---

### Task 1: Types and Trait Definition

**Files:**
- Create: `crates/desktop/src/permission_types.rs`
- Create: `crates/desktop/src/traits/permission.rs`
- Modify: `crates/desktop/src/traits/mod.rs`
- Modify: `crates/desktop/src/lib.rs`

- [ ] **Step 1: Create permission_types.rs**

Create `crates/desktop/src/permission_types.rs`:

```rust
//! Types for TCC permission management.

use serde::{Deserialize, Serialize};

/// A macOS TCC permission type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TccPermission {
    ScreenRecording,
    Camera,
    Microphone,
    SpeechRecognition,
    Accessibility,
    Notifications,
}

impl TccPermission {
    /// All managed TCC permissions.
    pub const ALL: &'static [TccPermission] = &[
        TccPermission::ScreenRecording,
        TccPermission::Camera,
        TccPermission::Microphone,
        TccPermission::SpeechRecognition,
        TccPermission::Accessibility,
        TccPermission::Notifications,
    ];
}

/// Authorization status of a TCC permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    /// Permission granted.
    Granted,
    /// Permission denied (user explicitly denied or revoked).
    Denied,
    /// Permission not yet determined (never prompted).
    NotDetermined,
    /// Permission restricted by system policy (MDM, parental controls).
    Restricted,
    /// Cannot determine status on this platform.
    Unknown,
}

/// Information about a TCC permission's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub permission: TccPermission,
    pub status: PermissionStatus,
    /// Whether calling `request` will show a system prompt.
    pub can_request: bool,
}
```

- [ ] **Step 2: Create traits/permission.rs**

Create `crates/desktop/src/traits/permission.rs`:

```rust
//! Permission detection and request capability.

use async_trait::async_trait;

use crate::permission_types::{PermissionInfo, TccPermission};
use crate::Result;

/// TCC permission detection and request.
///
/// Provides read-only status checks (`check`, `check_all`) and
/// interactive permission requests (`request`) that may show
/// system dialogs.
#[async_trait]
pub trait PermissionCapability: Send + Sync {
    /// Check status of one permission without prompting the user.
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo>;

    /// Check status of all managed permissions.
    async fn check_all(&self) -> Result<Vec<PermissionInfo>>;

    /// Request a permission, potentially showing a system prompt.
    /// Returns the updated status after the request attempt.
    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo>;
}
```

- [ ] **Step 3: Update traits/mod.rs**

Add permission module to `crates/desktop/src/traits/mod.rs`:

```rust
pub mod automation;
pub mod permission;
pub mod pim;
pub mod screen;
pub mod system;

pub use automation::AutomationCapability;
pub use permission::PermissionCapability;
pub use pim::PimCapability;
pub use screen::ScreenCapability;
pub use system::SystemCapability;
```

- [ ] **Step 4: Update lib.rs**

In `crates/desktop/src/lib.rs`, add `pub mod permission_types;` after the existing module declarations and add `PermissionCapability` to the re-exports:

After line `pub mod system_types;` add:
```rust
pub mod permission_types;
```

After `pub use traits::{AutomationCapability, PimCapability, ScreenCapability, SystemCapability};` change to:
```rust
pub use traits::{AutomationCapability, PermissionCapability, PimCapability, ScreenCapability, SystemCapability};
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-desktop`
Expected: compiles (trait exists but no implementors yet)

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/permission_types.rs crates/desktop/src/traits/permission.rs crates/desktop/src/traits/mod.rs crates/desktop/src/lib.rs
git commit -m "desktop: add PermissionCapability trait and TCC types"
```

---

### Task 2: Extend DesktopPlatform + Platform Stubs

**Files:**
- Modify: `crates/desktop/src/platform.rs`
- Modify: `crates/desktop-macos/src/lib.rs`
- Modify: `crates/desktop-linux/src/lib.rs`
- Modify: `crates/desktop-windows/src/lib.rs`

- [ ] **Step 1: Add permission() to DesktopPlatform trait**

In `crates/desktop/src/platform.rs`, add import and method:

```rust
use crate::traits::{AutomationCapability, PermissionCapability, PimCapability, ScreenCapability, SystemCapability};
```

Add to the trait:
```rust
    /// TCC permission detection and request, if available.
    fn permission(&self) -> Option<&dyn PermissionCapability>;
```

- [ ] **Step 2: Update Linux platform**

In `crates/desktop-linux/src/lib.rs`, add to the `DesktopPlatform` impl:
```rust
    fn permission(&self) -> Option<&dyn aleph_desktop::PermissionCapability> {
        None
    }
```

- [ ] **Step 3: Update Windows platform**

In `crates/desktop-windows/src/lib.rs`, add to the `DesktopPlatform` impl:
```rust
    fn permission(&self) -> Option<&dyn aleph_desktop::PermissionCapability> {
        None
    }
```

- [ ] **Step 4: Add temporary None to macOS**

In `crates/desktop-macos/src/lib.rs`, add temporarily (will be replaced in Task 3):
```rust
    fn permission(&self) -> Option<&dyn aleph_desktop::PermissionCapability> {
        None // TODO: wire MacOSPermission in Task 3
    }
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p aleph-desktop -p aleph-desktop-macos -p aleph-desktop-linux -p aleph-desktop-windows`
Expected: compiles

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/platform.rs crates/desktop-macos/src/lib.rs crates/desktop-linux/src/lib.rs crates/desktop-windows/src/lib.rs
git commit -m "desktop: extend DesktopPlatform with permission() method"
```

---

### Task 3: macOS Permission Implementation

**Files:**
- Create: `crates/desktop-macos/src/permission.rs`
- Modify: `crates/desktop-macos/src/lib.rs`
- Modify: `crates/desktop-macos/Cargo.toml`

This is the largest task — implements all 6 TCC permission checks and requests.

- [ ] **Step 1: Add dependencies to Cargo.toml**

Add to `crates/desktop-macos/Cargo.toml`:

```toml
block2 = "0.6"
objc2-av-foundation = { version = "0.3", features = [
    "AVCaptureDevice", "AVMediaFormat",
] }
objc2-speech = { version = "0.3", features = [
    "SFSpeechRecognizer",
] }
```

- [ ] **Step 2: Create permission.rs**

Create `crates/desktop-macos/src/permission.rs`:

```rust
//! macOS TCC permission detection and request via native APIs.

use std::sync::mpsc;
use std::time::Duration;

use aleph_desktop::permission_types::{PermissionInfo, PermissionStatus, TccPermission};
use aleph_desktop::traits::PermissionCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct MacOSPermission {
    _private: (),
}

impl MacOSPermission {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MacOSPermission {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PermissionCapability for MacOSPermission {
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo> {
        tokio::task::spawn_blocking(move || check_permission(permission))
            .await
            .map_err(|e| DesktopError::NotAvailable(format!("permission check failed: {e}")))?
    }

    async fn check_all(&self) -> Result<Vec<PermissionInfo>> {
        tokio::task::spawn_blocking(|| {
            TccPermission::ALL
                .iter()
                .map(|&p| check_permission(p))
                .collect::<Result<Vec<_>>>()
        })
        .await
        .map_err(|e| DesktopError::NotAvailable(format!("permission check_all failed: {e}")))?
    }

    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo> {
        tokio::task::spawn_blocking(move || request_permission(permission))
            .await
            .map_err(|e| DesktopError::NotAvailable(format!("permission request failed: {e}")))?
    }
}

// ── Check (non-interactive) ─────────────────────────────────────

fn check_permission(permission: TccPermission) -> Result<PermissionInfo> {
    let (status, can_request) = match permission {
        TccPermission::ScreenRecording => check_screen_recording(),
        TccPermission::Camera => check_av_capture(true),
        TccPermission::Microphone => check_av_capture(false),
        TccPermission::SpeechRecognition => check_speech_recognition(),
        TccPermission::Accessibility => check_accessibility(),
        TccPermission::Notifications => check_notifications(),
    };
    Ok(PermissionInfo {
        permission,
        status,
        can_request,
    })
}

// ── Request (interactive) ───────────────────────────────────────

fn request_permission(permission: TccPermission) -> Result<PermissionInfo> {
    let (status, can_request) = match permission {
        TccPermission::ScreenRecording => request_screen_recording(),
        TccPermission::Camera => request_av_capture(true),
        TccPermission::Microphone => request_av_capture(false),
        TccPermission::SpeechRecognition => request_speech_recognition(),
        TccPermission::Accessibility => request_accessibility(),
        TccPermission::Notifications => request_notifications(),
    };
    Ok(PermissionInfo {
        permission,
        status,
        can_request,
    })
}

// ── Screen Recording (C FFI) ────────────────────────────────────

extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

fn check_screen_recording() -> (PermissionStatus, bool) {
    let granted = unsafe { CGPreflightScreenCaptureAccess() };
    if granted {
        (PermissionStatus::Granted, false)
    } else {
        // Cannot distinguish NotDetermined from Denied; use NotDetermined as conservative default
        (PermissionStatus::NotDetermined, true)
    }
}

fn request_screen_recording() -> (PermissionStatus, bool) {
    unsafe { CGRequestScreenCaptureAccess() };
    // Re-check after request
    check_screen_recording()
}

// ── Camera / Microphone (objc2-av-foundation) ───────────────────

fn check_av_capture(is_video: bool) -> (PermissionStatus, bool) {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo};

    let media_type = if is_video {
        unsafe { AVMediaTypeVideo.unwrap() }
    } else {
        unsafe { AVMediaTypeAudio.unwrap() }
    };

    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    match status {
        AVAuthorizationStatus::Authorized => (PermissionStatus::Granted, false),
        AVAuthorizationStatus::Denied => (PermissionStatus::Denied, false),
        AVAuthorizationStatus::Restricted => (PermissionStatus::Restricted, false),
        AVAuthorizationStatus::NotDetermined | _ => (PermissionStatus::NotDetermined, true),
    }
}

fn request_av_capture(is_video: bool) -> (PermissionStatus, bool) {
    use block2::RcBlock;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio, AVMediaTypeVideo};

    let media_type = if is_video {
        unsafe { AVMediaTypeVideo.unwrap() }
    } else {
        unsafe { AVMediaTypeAudio.unwrap() }
    };

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = tx.send(granted.as_bool());
    });

    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &block);
    }

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => (PermissionStatus::Granted, false),
        Ok(false) => (PermissionStatus::Denied, false),
        Err(_) => (PermissionStatus::Unknown, false),
    }
}

// ── Speech Recognition (objc2-speech) ───────────────────────────

fn check_speech_recognition() -> (PermissionStatus, bool) {
    use objc2_speech::SFSpeechRecognizer;

    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    // SFSpeechRecognizerAuthorizationStatus: 0=NotDetermined, 1=Denied, 2=Restricted, 3=Authorized
    match status {
        3 => (PermissionStatus::Granted, false),
        1 => (PermissionStatus::Denied, false),
        2 => (PermissionStatus::Restricted, false),
        0 | _ => (PermissionStatus::NotDetermined, true),
    }
}

fn request_speech_recognition() -> (PermissionStatus, bool) {
    use block2::RcBlock;
    use objc2_speech::SFSpeechRecognizer;

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |status: objc2_foundation::NSInteger| {
        let _ = tx.send(status);
    });

    unsafe {
        SFSpeechRecognizer::requestAuthorization(&block);
    }

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(3) => (PermissionStatus::Granted, false),
        Ok(1) => (PermissionStatus::Denied, false),
        Ok(2) => (PermissionStatus::Restricted, false),
        Ok(_) => (PermissionStatus::NotDetermined, true),
        Err(_) => (PermissionStatus::Unknown, false),
    }
}

// ── Accessibility (C FFI) ───────────────────────────────────────

extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

fn check_accessibility() -> (PermissionStatus, bool) {
    let trusted = unsafe { AXIsProcessTrusted() };
    if trusted {
        (PermissionStatus::Granted, false)
    } else {
        // Accessibility can always open System Settings
        (PermissionStatus::NotDetermined, true)
    }
}

fn request_accessibility() -> (PermissionStatus, bool) {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    let key = CFString::from_static_string("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(&key, &value)]);

    let trusted = unsafe {
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const std::ffi::c_void)
    };

    if trusted {
        (PermissionStatus::Granted, false)
    } else {
        // Even after requesting, can_request stays true for accessibility
        (PermissionStatus::Denied, true)
    }
}

// ── Notifications (objc2-user-notifications) ────────────────────

fn check_notifications() -> (PermissionStatus, bool) {
    // Check bundle ID first — UNUserNotificationCenter requires it
    let has_bundle = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some();

    if !has_bundle {
        return (PermissionStatus::Unknown, false);
    }

    // getNotificationSettings requires block2 — use synchronous channel pattern
    use block2::RcBlock;
    use objc2_user_notifications::UNUserNotificationCenter;

    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |settings: core::ptr::NonNull<objc2_user_notifications::UNNotificationSettings>| {
        let status = unsafe { settings.as_ref().authorizationStatus() };
        let _ = tx.send(status);
    });

    unsafe {
        center.getNotificationSettingsWithCompletionHandler(&block);
    }

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(status) => {
            // UNAuthorizationStatus: 0=NotDetermined, 1=Denied, 2=Authorized, 3=Provisional, 4=Ephemeral
            match status {
                2 | 3 | 4 => (PermissionStatus::Granted, false),
                1 => (PermissionStatus::Denied, false),
                0 | _ => (PermissionStatus::NotDetermined, true),
            }
        }
        Err(_) => (PermissionStatus::Unknown, false),
    }
}

fn request_notifications() -> (PermissionStatus, bool) {
    let has_bundle = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some();

    if !has_bundle {
        return (PermissionStatus::Unknown, false);
    }

    use block2::RcBlock;
    use objc2_user_notifications::UNUserNotificationCenter;

    let center = unsafe { UNUserNotificationCenter::currentNotificationCenter() };

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {
        let _ = tx.send(granted.as_bool());
    });

    unsafe {
        center.requestAuthorizationWithOptions_completionHandler(
            objc2_user_notifications::UNAuthorizationOptions::Alert
                | objc2_user_notifications::UNAuthorizationOptions::Sound,
            &block,
        );
    }

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => (PermissionStatus::Granted, false),
        Ok(false) => (PermissionStatus::Denied, false),
        Err(_) => (PermissionStatus::Unknown, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_accessibility() {
        let info = check_permission(TccPermission::Accessibility).unwrap();
        assert_eq!(info.permission, TccPermission::Accessibility);
        // Status is either Granted or NotDetermined — both are valid
        assert!(
            info.status == PermissionStatus::Granted || info.status == PermissionStatus::NotDetermined,
            "Accessibility should be Granted or NotDetermined, got: {:?}",
            info.status
        );
    }

    #[test]
    fn test_check_screen_recording() {
        let info = check_permission(TccPermission::ScreenRecording).unwrap();
        assert_eq!(info.permission, TccPermission::ScreenRecording);
    }

    #[test]
    fn test_check_all_returns_six() {
        let results: Vec<PermissionInfo> = TccPermission::ALL
            .iter()
            .map(|&p| check_permission(p).unwrap())
            .collect();
        assert_eq!(results.len(), 6);
    }
}
```

**Important notes for the implementor:**
- The `SFSpeechRecognizer::authorizationStatus()` and `requestAuthorization` method signatures must be verified against the actual `objc2-speech` crate source. The status values (0-3) follow Apple's `SFSpeechRecognizerAuthorizationStatus` enum.
- `UNNotificationSettings.authorizationStatus()` returns `UNAuthorizationStatus` (NSInteger). The exact type name in objc2 may differ.
- The `RcBlock::new()` closure signature must match the ObjC block signature exactly. If compilation fails on block type mismatches, check the crate source for the exact `DynBlock` signature in the method declaration and adjust the `RcBlock::new()` parameters.
- `core-foundation` is already available via the desktop crate's macOS dependencies.

- [ ] **Step 3: Wire into MacOSPlatform**

In `crates/desktop-macos/src/lib.rs`:

Add `mod permission;` and update the struct:

```rust
mod automation;
mod permission;
mod pim;
mod system;
```

Add `permission: permission::MacOSPermission` field to `MacOSPlatform` struct and initialize in `new()`.

Replace the temporary `None` in `fn permission()`:
```rust
    fn permission(&self) -> Option<&dyn PermissionCapability> {
        Some(&self.permission)
    }
```

Update the test `screen_is_some` to also check `platform.permission().is_some()`.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p aleph-desktop-macos`
Expected: compiles

- [ ] **Step 5: Run tests**

Run: `cargo test -p aleph-desktop-macos --lib permission`
Expected: 3 tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/desktop-macos/src/permission.rs crates/desktop-macos/src/lib.rs crates/desktop-macos/Cargo.toml Cargo.lock
git commit -m "desktop-macos: implement TCC permission check/request via objc2 + block2"
```

---

### Task 4: Permission Tool

**Files:**
- Create: `src/builtin_tools/permission_tool.rs`
- Modify: `src/builtin_tools/mod.rs`
- Modify: `src/executor/builtin_registry/builder.rs`

- [ ] **Step 1: Create permission_tool.rs**

Create `src/builtin_tools/permission_tool.rs`:

```rust
//! Permission tool — TCC permission detection and request.
//!
//! Delegates to `DesktopPlatform::permission()` (PermissionCapability).

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::tools::AlephTool;

/// Permission tool — check or request macOS TCC permissions.
#[derive(Clone)]
pub struct PermissionTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl PermissionTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionArgs {
    /// Action: "check", "check_all", or "request"
    pub action: String,
    /// Permission name (required for "check" and "request"):
    /// "screen_recording", "camera", "microphone", "speech_recognition",
    /// "accessibility", "notifications"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn parse_permission(name: &str) -> Option<aleph_desktop::permission_types::TccPermission> {
    use aleph_desktop::permission_types::TccPermission;
    match name {
        "screen_recording" => Some(TccPermission::ScreenRecording),
        "camera" => Some(TccPermission::Camera),
        "microphone" => Some(TccPermission::Microphone),
        "speech_recognition" => Some(TccPermission::SpeechRecognition),
        "accessibility" => Some(TccPermission::Accessibility),
        "notifications" => Some(TccPermission::Notifications),
        _ => None,
    }
}

#[async_trait]
impl AlephTool for PermissionTool {
    const NAME: &'static str = "permission";
    const DESCRIPTION: &'static str = r#"Check or request macOS system permissions (TCC). Use before accessing camera, microphone, screen recording, etc.

Actions:
- check: Check one permission status (no system prompt). Required: permission
- check_all: Check all permission statuses
- request: Request a permission (may show system dialog). Required: permission

Permission values: screen_recording, camera, microphone, speech_recognition, accessibility, notifications

Examples:
{"action":"check","permission":"camera"}
{"action":"check_all"}
{"action":"request","permission":"microphone"}"#;

    type Args = PermissionArgs;
    type Output = PermissionOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let perm_cap = match self.platform.permission() {
            Some(p) => p,
            None => {
                return Ok(PermissionOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "Permission capability is not available on {}.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        match args.action.as_str() {
            "check" => {
                let perm_name = match args.permission {
                    Some(ref name) => name.as_str(),
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some("check requires 'permission' parameter.".into()),
                        });
                    }
                };
                let perm = match parse_permission(perm_name) {
                    Some(p) => p,
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some(format!(
                                "Unknown permission '{}'. Valid: screen_recording, camera, microphone, speech_recognition, accessibility, notifications",
                                perm_name
                            )),
                        });
                    }
                };
                match perm_cap.check(perm).await {
                    Ok(info) => Ok(PermissionOutput {
                        success: true,
                        data: Some(serde_json::to_value(&info).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "check_all" => {
                match perm_cap.check_all().await {
                    Ok(infos) => Ok(PermissionOutput {
                        success: true,
                        data: Some(serde_json::to_value(&infos).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            "request" => {
                let perm_name = match args.permission {
                    Some(ref name) => name.as_str(),
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some("request requires 'permission' parameter.".into()),
                        });
                    }
                };
                let perm = match parse_permission(perm_name) {
                    Some(p) => p,
                    None => {
                        return Ok(PermissionOutput {
                            success: false,
                            data: None,
                            message: Some(format!("Unknown permission '{}'.", perm_name)),
                        });
                    }
                };
                match perm_cap.request(perm).await {
                    Ok(info) => Ok(PermissionOutput {
                        success: true,
                        data: Some(serde_json::to_value(&info).unwrap_or_default()),
                        message: None,
                    }),
                    Err(e) => Ok(PermissionOutput {
                        success: false,
                        data: None,
                        message: Some(e.to_string()),
                    }),
                }
            }

            other => Ok(PermissionOutput {
                success: false,
                data: None,
                message: Some(format!(
                    "Unknown action '{}'. Valid: check, check_all, request",
                    other
                )),
            }),
        }
    }
}
```

- [ ] **Step 2: Add to mod.rs**

In `src/builtin_tools/mod.rs`, add:
```rust
pub mod permission_tool;
```

- [ ] **Step 3: Register in builder.rs**

In `src/executor/builtin_registry/builder.rs`:

After `let automation_tool = AutomationTool::new(...)` add:
```rust
let permission_tool = PermissionTool::new(Arc::clone(&desktop_platform));
```

Add `permission_tool` to the struct construction (after `automation_tool`).

Add the import at the top of the file:
```rust
use crate::builtin_tools::permission_tool::PermissionTool;
```

Also add the field to the `BuiltinToolRegistry` struct in `registry.rs` and wire it into the tool definitions/schema registration (follow the pattern of `system_tool`).

- [ ] **Step 4: Verify full workspace compilation**

Run: `cargo check`
Expected: compiles

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/permission_tool.rs src/builtin_tools/mod.rs src/executor/builtin_registry/builder.rs src/executor/builtin_registry/registry.rs
git commit -m "core: add permission tool for TCC management"
```

---

### Task 5: Full Verification

**Files:** All modified crates

- [ ] **Step 1: Full workspace check**

Run: `cargo check`
Expected: compiles

- [ ] **Step 2: Run all desktop tests**

Run: `cargo test -p aleph-desktop --lib && cargo test -p aleph-desktop-macos --lib`
Expected: all pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p aleph-desktop -p aleph-desktop-macos -- -D warnings`
Expected: no warnings

- [ ] **Step 4: Commit if any fixes needed**

```bash
git add crates/desktop/ crates/desktop-macos/
git commit -m "desktop: fix clippy warnings from TCC permission implementation"
```
