# TCC Permission Management Design

**Date:** 2026-03-26
**Status:** Approved
**Scope:** Add macOS TCC permission detection and request infrastructure as a new PermissionCapability trait, enabling future screen recording, camera, microphone, and voice features.

## Background

macOS uses Transparency, Consent, and Control (TCC) to gate access to sensitive resources — screen recording, camera, microphone, accessibility, etc. Aleph's upcoming P1 (screen recording, camera) and P2 (voice, PTT) features all require TCC permissions. Without a permission management layer, tools would fail silently or with cryptic errors when permissions are missing.

OpenClaw (reference project) implements a comprehensive `PermissionManager` in Swift with status checking, interactive requests, background monitoring, and a Settings UI. Aleph needs the core infrastructure — check and request — without the UI or monitoring components (Aleph runs as a headless server).

## Scope

### In Scope
- New `PermissionCapability` trait (5th capability alongside Screen/System/Automation/PIM)
- macOS implementation for 6 TCC permissions
- `permission` tool exposing check/request to LLM
- DesktopPlatform trait extension

### Out of Scope
- Background permission monitoring (no UI consumer)
- Command execution approval system (separate concern, Aleph has existing tool_permission system)
- Permission Settings UI (future Tauri/Leptos work)
- Location permission (Aleph doesn't need it)
- AppleScript automation permission (osascript handles its own prompts)

## Managed Permissions

| Permission | Check API | Request API | Framework |
|-----------|-----------|-------------|-----------|
| ScreenRecording | `CGPreflightScreenCaptureAccess()` | `CGRequestScreenCaptureAccess()` | CoreGraphics (C FFI) |
| Camera | `AVCaptureDevice.authorizationStatus(for: .video)` | `AVCaptureDevice.requestAccess(for: .video)` | AVFoundation (objc2) |
| Microphone | `AVCaptureDevice.authorizationStatus(for: .audio)` | `AVCaptureDevice.requestAccess(for: .audio)` | AVFoundation (objc2) |
| SpeechRecognition | `SFSpeechRecognizer.authorizationStatus()` | `SFSpeechRecognizer.requestAuthorization()` | Speech (objc2) |
| Accessibility | `AXIsProcessTrusted()` | `AXIsProcessTrustedWithOptions(prompt: true)` | ApplicationServices (C FFI) |
| Notifications | `UNUserNotificationCenter.getNotificationSettings()` | `UNUserNotificationCenter.requestAuthorization()` | UserNotifications (objc2) |

## Type Definitions

**File:** `crates/desktop/src/permission_types.rs`

```rust
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub permission: TccPermission,
    pub status: PermissionStatus,
    pub can_request: bool,
}
```

### can_request Logic

| Status | can_request |
|--------|------------|
| NotDetermined | `true` (all permissions) |
| Granted | `false` (already granted) |
| Denied | `false` (macOS won't re-prompt; user must go to System Settings) |
| Denied + Accessibility | `true` (AXIsProcessTrustedWithOptions always opens System Settings) |
| Restricted | `false` (system policy, cannot override) |
| Unknown | `false` |

## Trait Definition

**File:** `crates/desktop/src/traits/permission.rs`

```rust
#[async_trait]
pub trait PermissionCapability: Send + Sync {
    /// Check status of one permission without prompting the user.
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo>;

    /// Check status of all managed permissions.
    async fn check_all(&self) -> Result<Vec<PermissionInfo>>;

    /// Request a permission, potentially showing a system prompt.
    /// Returns the new status after the request attempt.
    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo>;
}
```

**Design rationale:**
- `check` is read-only, never shows system dialogs
- `request` may show a system dialog (TCC prompt), returns updated status
- `check_all` is a convenience method — loops over all 6 permissions
- No `reset` method — that's an admin operation (`tccutil`), not for LLM use

## DesktopPlatform Extension

**File:** `crates/desktop/src/platform.rs`

```rust
pub trait DesktopPlatform: Send + Sync {
    fn platform_name(&self) -> &str;
    fn screen(&self) -> Option<&dyn ScreenCapability>;
    fn pim(&self) -> Option<&dyn PimCapability>;
    fn system(&self) -> Option<&dyn SystemCapability>;
    fn automation(&self) -> Option<&dyn AutomationCapability>;
    fn permission(&self) -> Option<&dyn PermissionCapability>;  // NEW
}
```

- macOS: returns `Some(&self.permission)`
- Linux: returns `None`
- Windows: returns `None`

## macOS Implementation

**File:** `crates/desktop-macos/src/permission.rs` (~200 lines, single file)

### Structure

```rust
pub struct MacOSPermission {
    _private: (),
}

impl MacOSPermission {
    pub fn new() -> Self { Self { _private: () } }
}

#[async_trait]
impl PermissionCapability for MacOSPermission {
    async fn check(&self, permission: TccPermission) -> Result<PermissionInfo> { ... }
    async fn check_all(&self) -> Result<Vec<PermissionInfo>> { ... }
    async fn request(&self, permission: TccPermission) -> Result<PermissionInfo> { ... }
}
```

### Per-Permission Implementation

#### ScreenRecording (C FFI)
```
check: CGPreflightScreenCaptureAccess() → true=Granted, false=NotDetermined/Denied
request: CGRequestScreenCaptureAccess() → opens System Settings, re-check status
```
Note: `CGPreflightScreenCaptureAccess` cannot distinguish NotDetermined from Denied. Both return `false`. **Decision:** map `false` to `NotDetermined` as the conservative default — this sets `can_request: true`, encouraging the LLM to try requesting. After the user explicitly denies in System Settings, the next `request()` call will open System Settings again (harmless). No internal state tracking needed.

#### Camera / Microphone (objc2-av-foundation + block2)
```
check: AVCaptureDevice::authorizationStatus(for: mediaType)
  → .authorized=Granted, .denied=Denied, .notDetermined=NotDetermined, .restricted=Restricted
request: AVCaptureDevice::requestAccess(for: mediaType, completionHandler: block)
  → block receives bool (granted or denied)
```

#### SpeechRecognition (objc2-speech + block2)
```
check: SFSpeechRecognizer::authorizationStatus()
  → .authorized=Granted, .denied=Denied, .notDetermined=NotDetermined, .restricted=Restricted
request: SFSpeechRecognizer::requestAuthorization(completionHandler: block)
  → block receives SFSpeechRecognizerAuthorizationStatus
```

#### Accessibility (C FFI)
```
check: AXIsProcessTrusted() → true=Granted, false=Denied/NotDetermined
request: AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})
  → opens System Settings Privacy panel, returns current status
```
Special: Accessibility `request` always opens System Settings even if previously denied.

#### Notifications (objc2-user-notifications + block2)
```
check: UNUserNotificationCenter.getNotificationSettings() → block with UNNotificationSettings
  → .authorizationStatus: .authorized=Granted, .denied=Denied, .notDetermined=NotDetermined
request: UNUserNotificationCenter.requestAuthorization(options: [.alert, .sound]) → block with (Bool, Error?)
```
Note: Requires bundle ID. **Detection strategy:** Before calling any UNUserNotificationCenter API, check `NSBundle::mainBundle().bundleIdentifier()`. If `None` (CLI mode without .app bundle), skip the UNCenter call entirely and return `PermissionInfo { status: Unknown, can_request: false }` with no error. This avoids cryptic ObjC failures. When Aleph ships as a bundled .app, the check will automatically start working.

### block2 Usage Pattern

For APIs requiring completion handlers:
```rust
use block2::RcBlock;
use std::sync::mpsc;

let (tx, rx) = mpsc::channel();
let block = RcBlock::new(move |granted: Bool| {
    let _ = tx.send(granted.as_bool());
});
// Call the API with &block
// Wait with timeout to prevent permanent thread hang if callback never fires
let result = rx.recv_timeout(Duration::from_secs(10))
    .map_err(|_| DesktopError::NotAvailable("permission request timed out".into()))?;
```

This converts async ObjC callbacks to synchronous Rust, then wrapped in `spawn_blocking`.

## Tool Definition

**File:** `src/builtin_tools/permission_tool.rs`

```json
{
  "name": "permission",
  "description": "Check or request macOS system permissions (TCC). Use before accessing camera, microphone, screen recording, etc.",
  "parameters": {
    "action": { "enum": ["check", "check_all", "request"] },
    "permission": { "enum": ["screen_recording", "camera", "microphone", "speech_recognition", "accessibility", "notifications"] }
  }
}
```

- `check` requires `permission` parameter
- `check_all` ignores `permission` parameter
- `request` requires `permission` parameter

### Approval Policy
- `check` / `check_all`: no approval needed (read-only)
- `request`: requires user approval (shows system dialog)

### Tool Response
```json
{
  "permission": "camera",
  "status": "not_determined",
  "can_request": true,
  "message": "Camera permission has not been requested yet. Use action 'request' to prompt the user."
}
```

### LLM Integration

System prompt guidance (not hardcoded dependencies):
> "Before using screen recording, camera, or microphone tools, check permissions with the `permission` tool. If status is `not_determined`, request the permission first."

This follows R8 (LLM Sovereignty) — the LLM decides when to check permissions.

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `crates/desktop/src/permission_types.rs` | TccPermission, PermissionStatus, PermissionInfo |
| Create | `crates/desktop/src/traits/permission.rs` | PermissionCapability trait |
| Create | `crates/desktop-macos/src/permission.rs` | macOS TCC implementation |
| Create | `src/builtin_tools/permission_tool.rs` | LLM-facing tool |
| Modify | `crates/desktop/src/traits/mod.rs` | Export PermissionCapability |
| Modify | `crates/desktop/src/lib.rs` | Export permission_types |
| Modify | `crates/desktop/src/platform.rs` | Add permission() method |
| Modify | `crates/desktop-macos/src/lib.rs` | Wire MacOSPermission into MacOSPlatform |
| Modify | `crates/desktop-linux/src/lib.rs` | Return None for permission() |
| Modify | `crates/desktop-windows/src/lib.rs` | Return None for permission() |
| Modify | `crates/desktop-macos/Cargo.toml` | Add block2, objc2-av-foundation, objc2-speech |
| Modify | `src/executor/builtin_registry/builder.rs` | Register PermissionTool |

## Dependency Changes

### desktop-macos/Cargo.toml additions

```toml
block2 = "0.6"
objc2-av-foundation = { version = "0.3", features = [
    "AVCaptureDevice",
] }
objc2-speech = { version = "0.3", features = [
    "SFSpeechRecognizer",
] }
```

Note: `objc2-user-notifications` already present but needs additional feature `UNNotificationSettings` for `getNotificationSettings()`. Also needs `block2` feature for completion handlers. `core-graphics` already available via desktop crate for CGPreflight/CGRequest FFI.

### C FFI declarations needed

```rust
// Screen Recording (CoreGraphics)
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

// Accessibility (ApplicationServices)
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
}
```

## Error Handling

No new `DesktopError` variants. Map failures to existing variants:
- Permission check failures → `DesktopError::NotAvailable("permission: ...")`
- Platform not supported → `DesktopError::NotImplemented("...")`

## Testing

### Unit tests in permission.rs
- `test_check_notifications` — query notification permission (works in CI, no display needed)
- `test_check_accessibility` — query accessibility status (always works)
- `test_check_all_returns_six` — verify check_all returns exactly 6 PermissionInfo entries
- `test_screen_recording_check` — query screen recording status

### Tool tests in permission_tool.rs
- `test_check_action_requires_permission_param` — missing permission returns error
- `test_check_all_action` — returns all permissions

All tests gated with `#[cfg(target_os = "macos")]` where needed.

## Non-Goals

- Background permission monitoring / polling
- Permission Settings UI
- Command execution approval system
- Permission reset (`tccutil reset`)
- Location permission
- AppleScript automation permission
