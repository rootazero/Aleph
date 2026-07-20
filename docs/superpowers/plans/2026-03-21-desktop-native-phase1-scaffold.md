# Desktop Native Capabilities — Phase 1: Architecture Scaffold

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish the trait hierarchy, per-platform crate skeletons, Swift CLI skeleton, and two new builtin tools (system, automation) — without breaking existing desktop/pim functionality.

**Architecture:** New capability traits (Screen, Pim, System, Automation) defined in `crates/desktop/`, aggregated by `DesktopPlatform`. Three platform crates (`desktop-macos`, `desktop-linux`, `desktop-windows`) provide stub implementations. Existing `DesktopTool` and `PimTool` stay untouched in Phase 1 — they'll be migrated in Phase 2/3.

**Tech Stack:** Rust (async-trait, serde, schemars, tokio), Swift (Package.swift + ArgumentParser)

**Spec:** `docs/superpowers/specs/2026-03-21-desktop-native-capabilities-design.md`

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `crates/desktop/src/traits/mod.rs` | Re-export all capability traits |
| `crates/desktop/src/traits/screen.rs` | `ScreenCapability` trait |
| `crates/desktop/src/traits/pim.rs` | `PimCapability` trait |
| `crates/desktop/src/traits/system.rs` | `SystemCapability` trait |
| `crates/desktop/src/traits/automation.rs` | `AutomationCapability` trait |
| `crates/desktop/src/platform.rs` | `DesktopPlatform` aggregator trait |
| `crates/desktop/src/pim_types.rs` | PIM shared types (NoteInfo, CalendarEvent, Reminder, Contact, etc.) |
| `crates/desktop/src/system_types.rs` | System shared types (AppInfo, ClipboardContent, SystemInfo, etc.) |
| `crates/desktop/src/automation_types.rs` | Automation shared types (ScriptLanguage, ShortcutInfo) |
| `crates/desktop/src/bridge.rs` | `SwiftBridge` utility for spawning Swift CLI |
| `crates/desktop-macos/Cargo.toml` | macOS crate manifest |
| `crates/desktop-macos/src/lib.rs` | `MacOSPlatform` stub implementing `DesktopPlatform` |
| `crates/desktop-linux/Cargo.toml` | Linux crate manifest |
| `crates/desktop-linux/src/lib.rs` | `LinuxPlatform` stub implementing `DesktopPlatform` |
| `crates/desktop-windows/Cargo.toml` | Windows crate manifest |
| `crates/desktop-windows/src/lib.rs` | `WindowsPlatform` stub implementing `DesktopPlatform` |
| `src/builtin_tools/system_tool.rs` | `SystemTool` builtin (empty shell) |
| `src/builtin_tools/automation_tool.rs` | `AutomationTool` builtin (empty shell) |
| `apps/macos-bridge/Package.swift` | Swift package manifest |
| `apps/macos-bridge/Sources/AlephBridge/main.swift` | Swift CLI entry point skeleton |

### Modified Files

| File | Change |
|------|--------|
| `crates/desktop/src/lib.rs` | Add `pub mod traits`, `pub mod platform`, `pub mod pim_types`, etc. |
| `crates/desktop/Cargo.toml` | Add `chrono` dependency for PIM date types |
| `Cargo.toml` (workspace) | Add 3 new crate members |
| `Cargo.toml` | Add conditional platform crate deps |
| `src/builtin_tools/mod.rs` | Add `pub mod system_tool`, `pub mod automation_tool` |
| `src/executor/builtin_registry/registry.rs` | Add `system_tool`, `automation_tool` fields |
| `src/executor/builtin_registry/builder.rs` | Construct platform, new tools, register metadata |

---

## Task 1: Define Capability Traits

**Files:**
- Create: `crates/desktop/src/traits/mod.rs`
- Create: `crates/desktop/src/traits/screen.rs`
- Create: `crates/desktop/src/traits/pim.rs`
- Create: `crates/desktop/src/traits/system.rs`
- Create: `crates/desktop/src/traits/automation.rs`
- Create: `crates/desktop/src/platform.rs`
- Modify: `crates/desktop/src/lib.rs`

- [ ] **Step 1: Create `traits/screen.rs`**

Re-use existing types from `lib.rs` (ScreenRegion, Screenshot, OcrResult, MouseButton, WindowInfo).
This trait is intentionally identical to the existing `DesktopCapability` — it's the new name.

```rust
// crates/desktop/src/traits/screen.rs
use async_trait::async_trait;
use crate::{MouseButton, OcrResult, Result, ScreenRegion, Screenshot, WindowInfo};

/// Screen control capability — screenshot, OCR, click, type, scroll, window management.
///
/// This replaces the legacy `DesktopCapability` trait with the same methods.
/// Platform crates implement this for their native screen control APIs.
#[async_trait]
pub trait ScreenCapability: Send + Sync {
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;
    async fn ocr(&self, image_png: Option<&[u8]>) -> Result<OcrResult>;
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()>;
    async fn scroll(&self, direction: &str, amount: i32) -> Result<()>;
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;
    async fn focus_window(&self, window_id: u64) -> Result<()>;
    async fn launch_app(&self, app_name: &str) -> Result<()>;
}
```

- [ ] **Step 2: Create PIM types in `crates/desktop/src/pim_types.rs`**

```rust
// crates/desktop/src/pim_types.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteInfo {
    pub id: String,
    pub title: String,
    pub folder: Option<String>,
    pub snippet: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteContent {
    pub id: String,
    pub title: String,
    pub body: String,
    pub folder: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub start: String,
    pub end: String,
    pub calendar_id: Option<String>,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub all_day: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCalendarEvent {
    pub title: String,
    pub start: String,
    pub end: String,
    pub calendar_id: Option<String>,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub all_day: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarInfo {
    pub id: String,
    pub title: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub title: String,
    pub list_id: Option<String>,
    pub due_date: Option<String>,
    pub priority: Option<i32>,
    pub completed: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewReminder {
    pub title: String,
    pub list_id: Option<String>,
    pub due_date: Option<String>,
    pub priority: Option<i32>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReminderList {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub organization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactDetail {
    pub id: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub organization: Option<String>,
    pub notes: Option<String>,
    pub phone_numbers: Vec<String>,
    pub emails: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactGroup {
    pub id: String,
    pub name: String,
}
```

- [ ] **Step 3: Create `traits/pim.rs`**

```rust
// crates/desktop/src/traits/pim.rs
use async_trait::async_trait;
use crate::pim_types::*;
use crate::Result;

/// PIM capability — Notes, Calendar, Reminders, Contacts.
///
/// Platform crates implement this using native APIs (EventKit, Contacts
/// framework on macOS; via Swift CLI bridge).
#[async_trait]
pub trait PimCapability: Send + Sync {
    // Notes
    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>>;
    async fn notes_read(&self, note_id: &str) -> Result<NoteContent>;
    async fn notes_create(&self, title: &str, body: &str, folder: Option<&str>) -> Result<NoteInfo>;
    async fn notes_update(&self, note_id: &str, title: Option<&str>, body: Option<&str>) -> Result<()>;
    async fn notes_delete(&self, note_id: &str) -> Result<()>;
    async fn notes_folders(&self) -> Result<Vec<String>>;

    // Calendar
    async fn calendar_list_events(&self, from: &str, to: &str, calendar_id: Option<&str>) -> Result<Vec<CalendarEvent>>;
    async fn calendar_get_event(&self, event_id: &str) -> Result<CalendarEvent>;
    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<CalendarEvent>;
    async fn calendar_update_event(&self, event_id: &str, title: Option<&str>, start: Option<&str>, end: Option<&str>, location: Option<&str>, notes: Option<&str>) -> Result<()>;
    async fn calendar_delete_event(&self, event_id: &str) -> Result<()>;
    async fn calendar_calendars(&self) -> Result<Vec<CalendarInfo>>;

    // Reminders
    async fn reminders_list(&self, list_id: Option<&str>, include_completed: bool) -> Result<Vec<Reminder>>;
    async fn reminders_get(&self, reminder_id: &str) -> Result<Reminder>;
    async fn reminders_create(&self, reminder: NewReminder) -> Result<Reminder>;
    async fn reminders_complete(&self, reminder_id: &str, completed: bool) -> Result<()>;
    async fn reminders_delete(&self, reminder_id: &str) -> Result<()>;
    async fn reminders_lists(&self) -> Result<Vec<ReminderList>>;

    // Contacts
    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>>;
    async fn contacts_get(&self, contact_id: &str) -> Result<ContactDetail>;
    async fn contacts_groups(&self) -> Result<Vec<ContactGroup>>;
}
```

- [ ] **Step 4: Create system types in `crates/desktop/src/system_types.rs`**

```rust
// crates/desktop/src/system_types.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: Option<u64>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum ClipboardContent {
    Text(String),
    Image(String), // base64 PNG
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub hostname: String,
    pub username: String,
    pub battery_level: Option<f64>,
    pub battery_charging: Option<bool>,
}
```

- [ ] **Step 5: Create `traits/system.rs`**

```rust
// crates/desktop/src/traits/system.rs
use async_trait::async_trait;
use crate::system_types::*;
use crate::Result;

/// System capability — app management, notifications, clipboard, system info.
#[async_trait]
pub trait SystemCapability: Send + Sync {
    async fn launch_app(&self, app_name: &str) -> Result<()>;
    async fn quit_app(&self, app_name: &str) -> Result<()>;
    async fn list_running_apps(&self) -> Result<Vec<AppInfo>>;
    async fn send_notification(&self, title: &str, body: &str) -> Result<()>;
    async fn clipboard_read(&self) -> Result<ClipboardContent>;
    async fn clipboard_write(&self, content: ClipboardContent) -> Result<()>;
    async fn system_info(&self) -> Result<SystemInfo>;
}
```

- [ ] **Step 6: Create automation types in `crates/desktop/src/automation_types.rs`**

```rust
// crates/desktop/src/automation_types.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    AppleScript,
    JavaScript, // JXA (JavaScript for Automation)
    Shell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutInfo {
    pub name: String,
    pub description: Option<String>,
}
```

- [ ] **Step 7: Create `traits/automation.rs`**

```rust
// crates/desktop/src/traits/automation.rs
use async_trait::async_trait;
use crate::automation_types::*;
use crate::Result;

/// Automation capability — run scripts, invoke Shortcuts.
#[async_trait]
pub trait AutomationCapability: Send + Sync {
    async fn run_script(&self, script: &str, language: ScriptLanguage) -> Result<String>;
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>>;
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String>;
}
```

- [ ] **Step 8: Create `traits/mod.rs`**

```rust
// crates/desktop/src/traits/mod.rs
mod screen;
mod pim;
mod system;
mod automation;

pub use screen::ScreenCapability;
pub use pim::PimCapability;
pub use system::SystemCapability;
pub use automation::AutomationCapability;
```

- [ ] **Step 9: Create `platform.rs`**

```rust
// crates/desktop/src/platform.rs
use crate::traits::{AutomationCapability, PimCapability, ScreenCapability, SystemCapability};

/// Aggregator trait for all desktop capabilities on a platform.
///
/// Each platform crate implements this. Capabilities that aren't available
/// on a given platform return `None`.
pub trait DesktopPlatform: Send + Sync {
    fn screen(&self) -> Option<&dyn ScreenCapability>;
    fn pim(&self) -> Option<&dyn PimCapability>;
    fn system(&self) -> Option<&dyn SystemCapability>;
    fn automation(&self) -> Option<&dyn AutomationCapability>;
    fn platform_name(&self) -> &str;
}
```

- [ ] **Step 10: Update `crates/desktop/src/lib.rs`**

Add new module declarations alongside existing code. The existing `DesktopCapability` trait and `NativeDesktop` stay untouched.

Add after the existing module declarations (line 23-26):

```rust
pub mod traits;
pub mod platform;
pub mod pim_types;
pub mod system_types;
pub mod automation_types;
pub mod bridge;
```

Add re-exports after existing re-exports:

```rust
pub use traits::{ScreenCapability, PimCapability, SystemCapability, AutomationCapability};
pub use platform::DesktopPlatform;
```

- [ ] **Step 11: Verify it compiles**

Run: `cargo check -p aleph-desktop`
Expected: compiles with no errors

- [ ] **Step 12: Commit**

```bash
git add crates/desktop/src/traits/ crates/desktop/src/platform.rs \
    crates/desktop/src/pim_types.rs crates/desktop/src/system_types.rs \
    crates/desktop/src/automation_types.rs crates/desktop/src/lib.rs
git commit -m "desktop: add capability trait hierarchy and shared types"
```

---

## Task 2: Add SwiftBridge Utility

**Files:**
- Create: `crates/desktop/src/bridge.rs`
- Modify: `crates/desktop/Cargo.toml` (if `tokio/process` not already included)

- [ ] **Step 1: Write test for SwiftBridge argument building**

```rust
// at bottom of crates/desktop/src/bridge.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_args() {
        let bridge = SwiftBridge::new(PathBuf::from("/usr/local/bin/aleph-bridge"));
        // Verify the binary path is stored correctly
        assert_eq!(bridge.binary_path, PathBuf::from("/usr/local/bin/aleph-bridge"));
    }

    #[test]
    fn test_default_binary_path() {
        let bridge = SwiftBridge::default();
        // Should resolve to a path containing "aleph-bridge"
        let path_str = bridge.binary_path.to_string_lossy();
        assert!(path_str.contains("aleph-bridge"), "default path should contain 'aleph-bridge': {path_str}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-desktop --lib bridge`
Expected: FAIL — module `bridge` not found (or struct doesn't exist)

- [ ] **Step 3: Implement SwiftBridge**

```rust
// crates/desktop/src/bridge.rs
//! Swift CLI bridge — spawns the `aleph-bridge` binary and communicates via JSON.
//!
//! Used by macOS platform crate to invoke Swift code that calls Apple frameworks
//! (EventKit, Contacts, etc.) which have no Rust bindings.

use std::path::PathBuf;
use std::process::Stdio;

use serde::de::DeserializeOwned;

use crate::{DesktopError, Result};

/// Bridge to the `aleph-bridge` Swift CLI binary.
///
/// Invokes subcommands like `aleph-bridge notes list --folder "个人"`
/// and parses JSON output from stdout.
pub struct SwiftBridge {
    pub(crate) binary_path: PathBuf,
}

impl SwiftBridge {
    /// Create a bridge pointing to a specific binary path.
    pub fn new(binary_path: PathBuf) -> Self {
        Self { binary_path }
    }

    /// Execute a bridge command and deserialize the JSON result.
    ///
    /// # Arguments
    /// * `domain` — subcommand domain: "notes", "calendar", "reminders", "contacts", "system"
    /// * `action` — action within domain: "list", "create", "get", etc.
    /// * `args` — key-value pairs passed as `--key value` CLI arguments
    pub async fn call<T: DeserializeOwned>(
        &self,
        domain: &str,
        action: &str,
        args: &[(&str, &str)],
    ) -> Result<T> {
        let mut cmd = tokio::process::Command::new(&self.binary_path);
        cmd.arg(domain).arg(action);

        for (key, value) in args {
            cmd.arg(format!("--{key}"));
            cmd.arg(value);
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd.output().await.map_err(|e| {
            DesktopError::BridgeFailed(format!(
                "Failed to spawn aleph-bridge at {}: {e}",
                self.binary_path.display()
            ))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::BridgeFailed(format!(
                "aleph-bridge {domain} {action} failed (exit {}): {stderr}",
                output.status.code().unwrap_or(-1)
            )));
        }

        serde_json::from_slice(&output.stdout).map_err(|e| {
            let raw = String::from_utf8_lossy(&output.stdout);
            DesktopError::BridgeFailed(format!(
                "Failed to parse bridge JSON for {domain} {action}: {e}\nRaw output: {raw}"
            ))
        })
    }

    /// Check if the bridge binary exists and is executable.
    pub fn is_available(&self) -> bool {
        self.binary_path.exists()
    }
}

impl Default for SwiftBridge {
    /// Default bridge looks for `aleph-bridge` next to the current executable,
    /// then falls back to `~/.aleph/bin/aleph-bridge`.
    fn default() -> Self {
        let next_to_exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("aleph-bridge")))
            .filter(|p| p.exists());

        let in_aleph_bin = dirs::home_dir()
            .map(|h| h.join(".aleph").join("bin").join("aleph-bridge"))
            .filter(|p| p.exists());

        let binary_path = next_to_exe
            .or(in_aleph_bin)
            .unwrap_or_else(|| PathBuf::from("aleph-bridge"));

        Self { binary_path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_command_args() {
        let bridge = SwiftBridge::new(PathBuf::from("/usr/local/bin/aleph-bridge"));
        assert_eq!(bridge.binary_path, PathBuf::from("/usr/local/bin/aleph-bridge"));
    }

    #[test]
    fn test_default_binary_path() {
        let bridge = SwiftBridge::default();
        let path_str = bridge.binary_path.to_string_lossy();
        assert!(path_str.contains("aleph-bridge"), "default path should contain 'aleph-bridge': {path_str}");
    }
}
```

- [ ] **Step 4: Add `BridgeFailed` variant to error type**

Check `crates/desktop/src/error.rs` — add `BridgeFailed(String)` variant if not present.

- [ ] **Step 5: Run tests**

Run: `cargo test -p aleph-desktop --lib bridge`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/desktop/src/bridge.rs crates/desktop/src/error.rs
git commit -m "desktop: add SwiftBridge utility for macOS native API calls"
```

---

## Task 3: Create Platform Crate Skeletons

**Files:**
- Create: `crates/desktop-macos/Cargo.toml`
- Create: `crates/desktop-macos/src/lib.rs`
- Create: `crates/desktop-linux/Cargo.toml`
- Create: `crates/desktop-linux/src/lib.rs`
- Create: `crates/desktop-windows/Cargo.toml`
- Create: `crates/desktop-windows/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create `crates/desktop-macos/Cargo.toml`**

```toml
[package]
name = "aleph-desktop-macos"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
aleph-desktop = { path = "../desktop" }
async-trait = "0.1"
tokio = { version = "1", features = ["process"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
```

- [ ] **Step 2: Create `crates/desktop-macos/src/lib.rs`**

```rust
//! macOS desktop platform — full native implementation.
//!
//! Provides all four capabilities: Screen (xcap + enigo), PIM (Swift CLI bridge),
//! System (partial Rust + Swift), Automation (osascript + Shortcuts CLI).
//!
//! Phase 1: Stub implementation. Phase 2/3 will add real implementations.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// macOS desktop platform with full native capabilities.
pub struct MacOSPlatform {
    _private: (),
}

impl MacOSPlatform {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MacOSPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for MacOSPlatform {
    fn screen(&self) -> Option<&dyn ScreenCapability> {
        // Phase 2: will return the ScreenCapability implementation
        None
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        // Phase 3: will return the PimCapability implementation via SwiftBridge
        None
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        // Phase 3: will return the SystemCapability implementation
        None
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        // Phase 3: will return the AutomationCapability implementation
        None
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
    fn test_macos_platform_stubs() {
        let platform = MacOSPlatform::default();
        // Phase 1: all capabilities are None (stub)
        assert!(platform.screen().is_none());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
```

- [ ] **Step 3: Create `crates/desktop-linux/Cargo.toml`**

```toml
[package]
name = "aleph-desktop-linux"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
aleph-desktop = { path = "../desktop" }
async-trait = "0.1"
```

- [ ] **Step 4: Create `crates/desktop-linux/src/lib.rs`**

```rust
//! Linux desktop platform — framework stub.
//!
//! Provides skeleton for community plugin extension.
//! Screen capability will be added in Phase 2 (xcap + enigo).
//! PIM, System, and Automation capabilities are left to community plugins.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// Linux desktop platform — framework stub for community extension.
pub struct LinuxPlatform {
    _private: (),
}

impl LinuxPlatform {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for LinuxPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for LinuxPlatform {
    fn screen(&self) -> Option<&dyn ScreenCapability> {
        // Phase 2: will add basic xcap + enigo screen capability
        None
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        None // Community plugin territory
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        None // Community plugin territory
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        None // Community plugin territory
    }

    fn platform_name(&self) -> &str {
        "Linux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linux_platform_creation() {
        let platform = LinuxPlatform::new();
        assert_eq!(platform.platform_name(), "Linux");
        assert!(platform.screen().is_none());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
```

- [ ] **Step 5: Create `crates/desktop-windows/Cargo.toml`**

```toml
[package]
name = "aleph-desktop-windows"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[dependencies]
aleph-desktop = { path = "../desktop" }
async-trait = "0.1"
```

- [ ] **Step 6: Create `crates/desktop-windows/src/lib.rs`**

```rust
//! Windows desktop platform — framework stub.
//!
//! Provides skeleton for community plugin extension.
//! Screen capability will be added in Phase 2 (xcap + enigo).
//! PIM, System, and Automation capabilities are left to community plugins.

use aleph_desktop::traits::{
    AutomationCapability, PimCapability, ScreenCapability, SystemCapability,
};
use aleph_desktop::DesktopPlatform;

/// Windows desktop platform — framework stub for community extension.
pub struct WindowsPlatform {
    _private: (),
}

impl WindowsPlatform {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for WindowsPlatform {
    fn screen(&self) -> Option<&dyn ScreenCapability> {
        // Phase 2: will add basic xcap + enigo screen capability
        None
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        None // Community plugin territory
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        None // Community plugin territory
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        None // Community plugin territory
    }

    fn platform_name(&self) -> &str {
        "Windows"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_windows_platform_creation() {
        let platform = WindowsPlatform::new();
        assert_eq!(platform.platform_name(), "Windows");
        assert!(platform.screen().is_none());
        assert!(platform.pim().is_none());
        assert!(platform.system().is_none());
        assert!(platform.automation().is_none());
    }
}
```

- [ ] **Step 7: Add new crates to workspace `Cargo.toml`**

In root `Cargo.toml`, add to `members` array (after `"crates/desktop"`):

```toml
"crates/desktop-macos",
"crates/desktop-linux",
"crates/desktop-windows",
```

- [ ] **Step 8: Add conditional platform deps to `Cargo.toml`**

Add at the end of `Cargo.toml`:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
aleph-desktop-macos = { path = "../crates/desktop-macos" }

[target.'cfg(target_os = "linux")'.dependencies]
aleph-desktop-linux = { path = "../crates/desktop-linux" }

[target.'cfg(target_os = "windows")'.dependencies]
aleph-desktop-windows = { path = "../crates/desktop-windows" }
```

- [ ] **Step 9: Verify all crates compile**

Run: `cargo check --workspace`
Expected: compiles (may have warnings, no errors)
Note: `--workspace` may fail due to Tauri env deps. If so, check each new crate individually:

```bash
cargo check -p aleph-desktop
cargo check -p aleph-desktop-macos
cargo check -p alephcore
```

- [ ] **Step 10: Run tests for new crates**

Run: `cargo test -p aleph-desktop-macos --lib && cargo test -p aleph-desktop --lib bridge`
Expected: all tests PASS

- [ ] **Step 11: Commit**

```bash
git add crates/desktop-macos/ crates/desktop-linux/ crates/desktop-windows/ \
    Cargo.toml Cargo.toml
git commit -m "desktop: add per-platform crate skeletons (macos, linux, windows)"
```

---

## Task 4: Create SystemTool and AutomationTool Builtin Tools

**Files:**
- Create: `src/builtin_tools/system_tool.rs`
- Create: `src/builtin_tools/automation_tool.rs`
- Modify: `src/builtin_tools/mod.rs`

- [ ] **Step 1: Create `src/builtin_tools/system_tool.rs`**

Follow the same pattern as existing tools. Uses `DesktopPlatform` instead of `DesktopBridgeClient`.

```rust
//! System tool — app management, notifications, clipboard, system info.
//!
//! Delegates to the platform's SystemCapability implementation.
//! On platforms without SystemCapability, returns a friendly error message.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sync_primitives::Arc;
use crate::error::Result;
use crate::tools::AlephTool;

/// System tool — app management, notifications, clipboard, system info.
#[derive(Clone)]
pub struct SystemTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl SystemTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

/// Arguments for the system tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SystemArgs {
    /// The system operation to perform.
    ///
    /// App management: "launch_app", "quit_app", "list_running_apps"
    /// Notifications:  "send_notification"
    /// Clipboard:      "clipboard_read", "clipboard_write"
    /// System:         "system_info"
    pub action: String,

    /// App name or bundle ID (for launch_app, quit_app).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,

    /// Notification title (for send_notification).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Notification body or clipboard text (for send_notification, clipboard_write).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// Output from system operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[async_trait]
impl AlephTool for SystemTool {
    const NAME: &'static str = "system";
    const DESCRIPTION: &'static str = r#"System operations: app management, notifications, clipboard, system info.

- launch_app: Launch an app by name. Required: app_name
- quit_app: Quit an app by name. Required: app_name
- list_running_apps: List all running applications
- send_notification: Send a system notification. Required: title. Optional: body
- clipboard_read: Read clipboard contents
- clipboard_write: Write text to clipboard. Required: body
- system_info: Get system information (OS, hostname, battery, etc.)"#;

    type Args = SystemArgs;
    type Output = SystemOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let sys = match self.platform.system() {
            Some(s) => s,
            None => {
                return Ok(SystemOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "System capabilities are not available on {}. \
                         Install a community plugin to add this support.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        let result: std::result::Result<Value, String> = match args.action.as_str() {
            "launch_app" => {
                let name = args.app_name.as_deref().unwrap_or("");
                if name.is_empty() {
                    return Ok(SystemOutput {
                        success: false, data: None,
                        message: Some("app_name is required for launch_app".into()),
                    });
                }
                sys.launch_app(name).await
                    .map(|_| serde_json::json!({"launched": name}))
                    .map_err(|e| e.to_string())
            }
            "quit_app" => {
                let name = args.app_name.as_deref().unwrap_or("");
                if name.is_empty() {
                    return Ok(SystemOutput {
                        success: false, data: None,
                        message: Some("app_name is required for quit_app".into()),
                    });
                }
                sys.quit_app(name).await
                    .map(|_| serde_json::json!({"quit": name}))
                    .map_err(|e| e.to_string())
            }
            "list_running_apps" => {
                sys.list_running_apps().await
                    .map(|apps| serde_json::to_value(apps).unwrap_or_default())
                    .map_err(|e| e.to_string())
            }
            "send_notification" => {
                let title = args.title.as_deref().unwrap_or("");
                if title.is_empty() {
                    return Ok(SystemOutput {
                        success: false, data: None,
                        message: Some("title is required for send_notification".into()),
                    });
                }
                let body = args.body.as_deref().unwrap_or("");
                sys.send_notification(title, body).await
                    .map(|_| serde_json::json!({"sent": true}))
                    .map_err(|e| e.to_string())
            }
            "clipboard_read" => {
                sys.clipboard_read().await
                    .map(|content| serde_json::to_value(content).unwrap_or_default())
                    .map_err(|e| e.to_string())
            }
            "clipboard_write" => {
                let text = args.body.as_deref().unwrap_or("");
                if text.is_empty() {
                    return Ok(SystemOutput {
                        success: false, data: None,
                        message: Some("body is required for clipboard_write".into()),
                    });
                }
                use aleph_desktop::system_types::ClipboardContent;
                sys.clipboard_write(ClipboardContent::Text(text.to_string())).await
                    .map(|_| serde_json::json!({"written": true}))
                    .map_err(|e| e.to_string())
            }
            "system_info" => {
                sys.system_info().await
                    .map(|info| serde_json::to_value(info).unwrap_or_default())
                    .map_err(|e| e.to_string())
            }
            other => {
                return Ok(SystemOutput {
                    success: false, data: None,
                    message: Some(format!("Unknown system action: {other}. Valid actions: launch_app, quit_app, list_running_apps, send_notification, clipboard_read, clipboard_write, system_info")),
                });
            }
        };

        match result {
            Ok(data) => Ok(SystemOutput { success: true, data: Some(data), message: None }),
            Err(msg) => Ok(SystemOutput { success: false, data: None, message: Some(msg) }),
        }
    }
}
```

- [ ] **Step 2: Create `src/builtin_tools/automation_tool.rs`**

```rust
//! Automation tool — run scripts (AppleScript/JXA/Shell) and invoke Shortcuts.
//!
//! Delegates to the platform's AutomationCapability implementation.
//! On platforms without AutomationCapability, returns a friendly error message.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sync_primitives::Arc;
use crate::error::Result;
use crate::tools::AlephTool;

/// Automation tool — scripts and Shortcuts.
#[derive(Clone)]
pub struct AutomationTool {
    platform: Arc<dyn aleph_desktop::DesktopPlatform>,
}

impl AutomationTool {
    pub fn new(platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
        Self { platform }
    }
}

/// Arguments for the automation tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AutomationArgs {
    /// The automation action to perform.
    ///
    /// Scripts:   "run_script"
    /// Shortcuts: "list_shortcuts", "run_shortcut"
    pub action: String,

    /// Script source code (for run_script).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,

    /// Script language: "applescript", "javascript", "shell" (for run_script).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Shortcut name (for run_shortcut).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Input data for shortcut (for run_shortcut).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
}

/// Output from automation operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[async_trait]
impl AlephTool for AutomationTool {
    const NAME: &'static str = "automation";
    const DESCRIPTION: &'static str = r#"Desktop automation: run scripts and invoke Shortcuts.

- run_script: Execute a script. Required: script, language. Language options: "applescript", "javascript" (JXA), "shell"
- list_shortcuts: List available Shortcuts (macOS Shortcuts app)
- run_shortcut: Run a named Shortcut. Required: name. Optional: input"#;

    type Args = AutomationArgs;
    type Output = AutomationOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let auto = match self.platform.automation() {
            Some(a) => a,
            None => {
                return Ok(AutomationOutput {
                    success: false,
                    data: None,
                    message: Some(format!(
                        "Automation capabilities are not available on {}. \
                         Install a community plugin to add this support.",
                        self.platform.platform_name()
                    )),
                });
            }
        };

        let result: std::result::Result<Value, String> = match args.action.as_str() {
            "run_script" => {
                let script = args.script.as_deref().unwrap_or("");
                if script.is_empty() {
                    return Ok(AutomationOutput {
                        success: false, data: None,
                        message: Some("script is required for run_script".into()),
                    });
                }
                let lang_str = args.language.as_deref().unwrap_or("applescript");
                let language = match lang_str {
                    "applescript" => aleph_desktop::automation_types::ScriptLanguage::AppleScript,
                    "javascript" | "jxa" => aleph_desktop::automation_types::ScriptLanguage::JavaScript,
                    "shell" | "bash" => aleph_desktop::automation_types::ScriptLanguage::Shell,
                    other => {
                        return Ok(AutomationOutput {
                            success: false, data: None,
                            message: Some(format!("Unknown script language: {other}. Valid: applescript, javascript, shell")),
                        });
                    }
                };
                auto.run_script(script, language).await
                    .map(|output| serde_json::json!({"output": output}))
                    .map_err(|e| e.to_string())
            }
            "list_shortcuts" => {
                auto.list_shortcuts().await
                    .map(|shortcuts| serde_json::to_value(shortcuts).unwrap_or_default())
                    .map_err(|e| e.to_string())
            }
            "run_shortcut" => {
                let name = args.name.as_deref().unwrap_or("");
                if name.is_empty() {
                    return Ok(AutomationOutput {
                        success: false, data: None,
                        message: Some("name is required for run_shortcut".into()),
                    });
                }
                auto.run_shortcut(name, args.input.as_deref()).await
                    .map(|output| serde_json::json!({"output": output}))
                    .map_err(|e| e.to_string())
            }
            other => {
                return Ok(AutomationOutput {
                    success: false, data: None,
                    message: Some(format!("Unknown automation action: {other}. Valid actions: run_script, list_shortcuts, run_shortcut")),
                });
            }
        };

        match result {
            Ok(data) => Ok(AutomationOutput { success: true, data: Some(data), message: None }),
            Err(msg) => Ok(AutomationOutput { success: false, data: None, message: Some(msg) }),
        }
    }
}
```

- [ ] **Step 3: Update `src/builtin_tools/mod.rs`**

Add module declarations (after `pub mod desktop;` at line 56):

```rust
pub mod system_tool;
pub mod automation_tool;
```

Add re-exports (after the desktop re-exports around line 101):

```rust
pub use system_tool::{SystemArgs, SystemOutput, SystemTool};
pub use automation_tool::{AutomationArgs, AutomationOutput, AutomationTool};
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: compiles (new tools aren't wired into registry yet, but module compiles)

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/system_tool.rs src/builtin_tools/automation_tool.rs \
    src/builtin_tools/mod.rs
git commit -m "core: add SystemTool and AutomationTool builtin tools"
```

---

## Task 5: Wire Up DesktopPlatform and Register New Tools

**Files:**
- Modify: `src/executor/builtin_registry/registry.rs`
- Modify: `src/executor/builtin_registry/builder.rs`

- [ ] **Step 1: Add new tool fields to `BuiltinToolRegistry` in `registry.rs`**

After the existing `pim_tool` field (line 50):

```rust
    /// System tool instance (app management, notifications, clipboard, system info)
    pub(crate) system_tool: crate::builtin_tools::SystemTool,
    /// Automation tool instance (scripts, Shortcuts)
    pub(crate) automation_tool: crate::builtin_tools::AutomationTool,
    /// Desktop platform reference (shared with new tools)
    pub(crate) desktop_platform: crate::sync_primitives::Arc<dyn aleph_desktop::DesktopPlatform>,
```

- [ ] **Step 2: Add tool dispatch cases in `registry.rs`**

Find the `"desktop"` dispatch case (around line 278) and add nearby:

```rust
"system" => Box::pin(async move { self.system_tool.call_json(arguments).await }),
"automation" => Box::pin(async move { self.automation_tool.call_json(arguments).await }),
```

- [ ] **Step 3: Construct platform and new tools in `builder.rs`**

Add platform construction after the existing desktop_tool construction (after line 69):

```rust
        // Build platform-specific DesktopPlatform
        let desktop_platform: Arc<dyn aleph_desktop::DesktopPlatform> = {
            #[cfg(target_os = "macos")]
            { Arc::new(aleph_desktop_macos::MacOSPlatform::new()) }

            #[cfg(target_os = "linux")]
            { Arc::new(aleph_desktop_linux::LinuxPlatform::new()) }

            #[cfg(target_os = "windows")]
            { Arc::new(aleph_desktop_windows::WindowsPlatform::new()) }

            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            { compile_error!("Unsupported platform — Aleph requires macOS, Linux, or Windows") }
        };

        // New capability-based tools (Phase 1: shell only, delegate to DesktopPlatform)
        let system_tool = crate::builtin_tools::SystemTool::new(Arc::clone(&desktop_platform));
        let automation_tool = crate::builtin_tools::AutomationTool::new(Arc::clone(&desktop_platform));
```

- [ ] **Step 4: Register new tool metadata in `register_core_tools()`**

Add in `register_core_tools()` (after the `"pim"` registration around line 390):

```rust
        reg(tools, "system", crate::builtin_tools::SystemTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::system_tool::SystemArgs)).unwrap_or_default());
        reg(tools, "automation", crate::builtin_tools::AutomationTool::DESCRIPTION,
            serde_json::to_value(schema_for!(crate::builtin_tools::automation_tool::AutomationArgs)).unwrap_or_default());
```

- [ ] **Step 5: Add new fields to the `Self { ... }` constructor return**

In the `Self { ... }` block in `with_config()` (around line 284-347), add:

```rust
            system_tool,
            automation_tool,
            desktop_platform,
```

- [ ] **Step 6: Add imports for platform crates in `builder.rs`**

The platform crates are conditionally compiled via `Cargo.toml`, so they're available when the target matches. Add the import for `SystemTool` and `AutomationTool` to the existing import line at line 12:

Update the existing import from `crate::builtin_tools`:

```rust
use crate::builtin_tools::{..., SystemTool, AutomationTool};
```

- [ ] **Step 7: Verify it compiles**

Run: `cargo check -p alephcore`
Expected: compiles with no errors

- [ ] **Step 8: Commit**

```bash
git add src/executor/builtin_registry/registry.rs \
    src/executor/builtin_registry/builder.rs
git commit -m "core: wire up DesktopPlatform and register system/automation tools"
```

---

## Task 6: Create Swift CLI Skeleton

**Files:**
- Create: `apps/macos-bridge/Package.swift`
- Create: `apps/macos-bridge/Sources/AlephBridge/main.swift`

- [ ] **Step 1: Create `apps/macos-bridge/Package.swift`**

```swift
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "AlephBridge",
    platforms: [.macOS(.v13)],
    dependencies: [
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.3.0"),
    ],
    targets: [
        .executableTarget(
            name: "AlephBridge",
            dependencies: [
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
            ],
            path: "Sources/AlephBridge"
        ),
    ]
)
```

- [ ] **Step 2: Create `apps/macos-bridge/Sources/AlephBridge/main.swift`**

```swift
import ArgumentParser
import Foundation

/// Aleph Bridge — Swift CLI for macOS native API access.
///
/// Provides subcommands for PIM (Notes, Calendar, Reminders, Contacts)
/// and system operations. Called by aleph-server via SwiftBridge.
///
/// All output is JSON on stdout. Errors go to stderr with non-zero exit.
@main
struct AlephBridge: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "aleph-bridge",
        abstract: "Aleph Desktop Bridge — macOS native API access via CLI",
        version: "0.1.0",
        subcommands: [
            Notes.self,
            Calendar.self,
            Reminders.self,
            Contacts.self,
            System.self,
        ]
    )
}

// MARK: - Notes

struct Notes: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Notes.app operations",
        subcommands: [List.self, Get.self, Create.self, Update.self, Delete.self, Folders.self]
    )

    struct List: ParsableCommand {
        @Option(help: "Filter by folder name")
        var folder: String?

        func run() throws {
            // Phase 3: Implement via AppleScript or ScriptingBridge
            printJSON(["notes": [Any](), "message": "Not yet implemented"])
        }
    }

    struct Get: ParsableCommand {
        @Argument(help: "Note ID")
        var id: String

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Create: ParsableCommand {
        @Option(help: "Note title")
        var title: String

        @Option(help: "Note body")
        var body: String?

        @Option(help: "Target folder")
        var folder: String?

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Update: ParsableCommand {
        @Argument(help: "Note ID")
        var id: String

        @Option(help: "New title")
        var title: String?

        @Option(help: "New body")
        var body: String?

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Delete: ParsableCommand {
        @Argument(help: "Note ID")
        var id: String

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Folders: ParsableCommand {
        func run() throws {
            printJSON(["folders": [String](), "message": "Not yet implemented"])
        }
    }
}

// MARK: - Calendar

struct Calendar: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Calendar operations (EventKit)",
        subcommands: [Events.self, Get.self, Create.self, Update.self, Delete.self, Calendars.self]
    )

    struct Events: ParsableCommand {
        @Option(help: "Start date (ISO 8601)")
        var from: String

        @Option(help: "End date (ISO 8601)")
        var to: String

        @Option(help: "Calendar ID filter")
        var calendarId: String?

        func run() throws {
            printJSON(["events": [Any](), "message": "Not yet implemented"])
        }
    }

    struct Get: ParsableCommand {
        @Argument(help: "Event ID")
        var id: String

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Create: ParsableCommand {
        @Option var title: String
        @Option var start: String
        @Option var end: String
        @Option var calendarId: String?
        @Option var location: String?
        @Option var notes: String?
        @Flag var allDay: Bool = false

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Update: ParsableCommand {
        @Argument var id: String
        @Option var title: String?
        @Option var start: String?
        @Option var end: String?
        @Option var location: String?
        @Option var notes: String?

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Delete: ParsableCommand {
        @Argument var id: String

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Calendars: ParsableCommand {
        func run() throws {
            printJSON(["calendars": [Any](), "message": "Not yet implemented"])
        }
    }
}

// MARK: - Reminders

struct Reminders: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Reminders operations (EventKit)",
        subcommands: [List.self, Get.self, Create.self, Complete.self, Delete.self, Lists.self]
    )

    struct List: ParsableCommand {
        @Option var listId: String?
        @Flag var includeCompleted: Bool = false

        func run() throws {
            printJSON(["reminders": [Any](), "message": "Not yet implemented"])
        }
    }

    struct Get: ParsableCommand {
        @Argument var id: String
        func run() throws { printJSON(["error": "Not yet implemented"]) }
    }

    struct Create: ParsableCommand {
        @Option var title: String
        @Option var listId: String?
        @Option var dueDate: String?
        @Option var priority: Int?
        @Option var notes: String?

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Complete: ParsableCommand {
        @Argument var id: String
        @Flag var completed: Bool = true

        func run() throws {
            printJSON(["error": "Not yet implemented"])
        }
    }

    struct Delete: ParsableCommand {
        @Argument var id: String
        func run() throws { printJSON(["error": "Not yet implemented"]) }
    }

    struct Lists: ParsableCommand {
        func run() throws {
            printJSON(["lists": [Any](), "message": "Not yet implemented"])
        }
    }
}

// MARK: - Contacts

struct Contacts: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Contacts operations",
        subcommands: [Search.self, Get.self, Groups.self]
    )

    struct Search: ParsableCommand {
        @Option var query: String
        func run() throws {
            printJSON(["contacts": [Any](), "message": "Not yet implemented"])
        }
    }

    struct Get: ParsableCommand {
        @Argument var id: String
        func run() throws { printJSON(["error": "Not yet implemented"]) }
    }

    struct Groups: ParsableCommand {
        func run() throws {
            printJSON(["groups": [Any](), "message": "Not yet implemented"])
        }
    }
}

// MARK: - System

struct System: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "System information and operations",
        subcommands: [Info.self]
    )

    struct Info: ParsableCommand {
        func run() throws {
            let info: [String: Any] = [
                "os_name": "macOS",
                "os_version": ProcessInfo.processInfo.operatingSystemVersionString,
                "hostname": ProcessInfo.processInfo.hostName,
                "username": NSUserName(),
            ]
            printJSON(info)
        }
    }
}

// MARK: - Helpers

func printJSON(_ value: Any) {
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
       let string = String(data: data, encoding: .utf8) {
        print(string)
    } else {
        fputs("Failed to serialize JSON\n", stderr)
        Foundation.exit(1)
    }
}
```

- [ ] **Step 3: Verify Swift CLI builds (macOS only)**

Run: `cd apps/macos-bridge && swift build`
Expected: builds successfully

- [ ] **Step 4: Test CLI help output**

Run: `cd apps/macos-bridge && swift run AlephBridge --help`
Expected: shows subcommands (notes, calendar, reminders, contacts, system)

- [ ] **Step 5: Commit**

```bash
git add apps/macos-bridge/
git commit -m "apps: add Swift CLI bridge skeleton for macOS native APIs"
```

---

## Task 7: Final Integration Verification

- [ ] **Step 1: Full compilation check**

Run: `cargo check -p alephcore`
Expected: no errors

- [ ] **Step 2: Run all existing tests to verify nothing is broken**

Run: `cargo test -p alephcore --lib`
Expected: same test results as before (no regressions)

- [ ] **Step 3: Run new crate tests**

Run: `cargo test -p aleph-desktop --lib && cargo test -p aleph-desktop-macos --lib`
Expected: all new tests pass

- [ ] **Step 4: Verify existing desktop and PIM tools still work**

The existing `DesktopTool` and `PimTool` are unchanged — they still use `DesktopBridgeClient` as before. This is intentional; they'll be migrated in Phase 2/3.

- [ ] **Step 5: Commit (if any fixes were needed)**

```bash
git commit -m "desktop: Phase 1 scaffold complete — integration verified"
```
