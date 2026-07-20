# Desktop Native Capabilities — Design Spec

**Date:** 2026-03-21
**Status:** Approved
**Scope:** Replace Tauri bridge with native platform implementations, add macOS PIM & system capabilities

---

## Overview

Aleph's desktop capabilities ("hands and feet") currently rely on a Tauri bridge process for screen control. This design replaces Tauri with native platform implementations compiled directly into `aleph-server`, and adds deep macOS system integration (Notes, Calendar, Reminders, Contacts, etc.). Linux/Windows get a framework skeleton for community plugin extension.

**This is NOT a desktop client.** No windows, menu bars, or dock icons. Pure backend capability layer.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Tauri replacement | Compile into aleph-server | Zero IPC overhead, simpler architecture |
| Platform isolation | Per-platform crate (`desktop-macos`, `desktop-linux`, `desktop-windows`) | Physical isolation, `cfg(target_os)` at crate level |
| macOS API calls | Hybrid: Rust crate (screen) + Swift CLI (PIM) + osascript (fallback) | Each tool best suited for its domain |
| Tool registration | Builtin tool (AlephTool trait) | Performance first, zero plugin overhead |
| macOS scope | Full built-in capabilities | Primary platform |
| Linux/Windows scope | Framework stub only, capabilities via plugins | Community-driven |

## §1: Trait Hierarchy

Four capability traits in `crates/desktop/`, one aggregator:

```rust
// crates/desktop/src/lib.rs

/// Screen control — screenshot, OCR, click, type, scroll, window management
#[async_trait]
pub trait ScreenCapability: Send + Sync {
    async fn screenshot(&self, region: Option<ScreenRegion>) -> Result<Screenshot>;
    async fn ocr(&self, image: Option<&[u8]>) -> Result<OcrResult>;
    async fn click(&self, x: f64, y: f64, button: MouseButton) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn key_combo(&self, modifiers: &[String], key: &str) -> Result<()>;
    async fn scroll(&self, direction: ScrollDirection, amount: i32) -> Result<()>;
    async fn window_list(&self) -> Result<Vec<WindowInfo>>;
    async fn focus_window(&self, window_id: u64) -> Result<()>;
}

/// PIM — Notes, Calendar, Reminders, Contacts
#[async_trait]
pub trait PimCapability: Send + Sync {
    // Notes
    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>>;
    async fn notes_read(&self, note_id: &str) -> Result<NoteContent>;
    async fn notes_create(&self, title: &str, body: &str, folder: Option<&str>) -> Result<NoteInfo>;
    async fn notes_update(&self, note_id: &str, title: Option<&str>, body: Option<&str>) -> Result<()>;
    async fn notes_delete(&self, note_id: &str) -> Result<()>;
    // Calendar
    async fn calendar_list_events(&self, from: DateTime, to: DateTime) -> Result<Vec<CalendarEvent>>;
    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<CalendarEvent>;
    async fn calendar_delete_event(&self, event_id: &str) -> Result<()>;
    // Reminders
    async fn reminders_list(&self, list: Option<&str>) -> Result<Vec<Reminder>>;
    async fn reminders_create(&self, reminder: NewReminder) -> Result<Reminder>;
    async fn reminders_complete(&self, reminder_id: &str) -> Result<()>;
    // Contacts
    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>>;
    async fn contacts_read(&self, contact_id: &str) -> Result<ContactDetail>;
}

/// System — app management, notifications, clipboard, system info
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

/// Automation — AppleScript, Shortcuts
#[async_trait]
pub trait AutomationCapability: Send + Sync {
    async fn run_script(&self, script: &str, language: ScriptLanguage) -> Result<String>;
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>>;
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String>;
}

/// Platform aggregator — each platform crate implements this
pub trait DesktopPlatform: Send + Sync {
    fn screen(&self) -> Option<&dyn ScreenCapability>;
    fn pim(&self) -> Option<&dyn PimCapability>;
    fn system(&self) -> Option<&dyn SystemCapability>;
    fn automation(&self) -> Option<&dyn AutomationCapability>;
    fn platform_name(&self) -> &str;
}
```

Each capability returns `Option` from `DesktopPlatform` — unimplemented capabilities return `None`.

## §2: Crate Structure & Conditional Compilation

```
crates/
├── desktop/                    — Platform-agnostic base crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs              — re-export traits + DesktopPlatform
│       ├── traits/
│       │   ├── mod.rs
│       │   ├── screen.rs       — ScreenCapability
│       │   ├── pim.rs          — PimCapability
│       │   ├── system.rs       — SystemCapability
│       │   └── automation.rs   — AutomationCapability
│       ├── types.rs            — Shared types (Screenshot, NoteInfo, CalendarEvent...)
│       └── bridge.rs           — SwiftBridge utility (spawn Swift CLI, JSON communication)
│
├── desktop-macos/              — Full macOS implementation
│   ├── Cargo.toml              — deps: desktop, serde_json, tokio
│   └── src/
│       ├── lib.rs              — MacOSPlatform: impl DesktopPlatform
│       ├── screen.rs           — impl ScreenCapability (xcap + enigo)
│       ├── pim.rs              — impl PimCapability (Swift CLI → EventKit/Contacts)
│       ├── system.rs           — impl SystemCapability (partial Rust, partial Swift)
│       └── automation.rs       — impl AutomationCapability (osascript + Shortcuts CLI)
│
├── desktop-linux/              — Linux framework stub
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs              — LinuxPlatform: impl DesktopPlatform (most return None)
│       └── screen.rs           — impl ScreenCapability (xcap + enigo, basic)
│
└── desktop-windows/            — Windows framework stub
    ├── Cargo.toml
    └── src/
        ├── lib.rs              — WindowsPlatform: impl DesktopPlatform (most return None)
        └── screen.rs           — impl ScreenCapability (xcap + enigo, basic)
```

**aleph-server Cargo.toml:**

```toml
[dependencies]
aleph-desktop = { path = "../crates/desktop" }

[target.'cfg(target_os = "macos")'.dependencies]
aleph-desktop-macos = { path = "../crates/desktop-macos" }

[target.'cfg(target_os = "linux")'.dependencies]
aleph-desktop-linux = { path = "../crates/desktop-linux" }

[target.'cfg(target_os = "windows")'.dependencies]
aleph-desktop-windows = { path = "../crates/desktop-windows" }
```

**Platform construction at server startup:**

```rust
fn build_platform() -> Box<dyn DesktopPlatform> {
    #[cfg(target_os = "macos")]
    { Box::new(aleph_desktop_macos::MacOSPlatform::new()) }

    #[cfg(target_os = "linux")]
    { Box::new(aleph_desktop_linux::LinuxPlatform::new()) }

    #[cfg(target_os = "windows")]
    { Box::new(aleph_desktop_windows::WindowsPlatform::new()) }
}
```

## §3: Swift CLI Bridge Architecture

A single Swift CLI binary with subcommands per domain.

**Directory:**

```
apps/macos-bridge/
├── Package.swift
├── Sources/
│   └── AlephBridge/
│       ├── main.swift              — Entry point, subcommand dispatch
│       ├── NotesCommand.swift      — Notes.app CRUD
│       ├── CalendarCommand.swift   — EventKit calendar operations
│       ├── RemindersCommand.swift  — EventKit reminders operations
│       ├── ContactsCommand.swift   — Contacts.framework
│       ├── SystemCommand.swift     — System info, notifications, app management
│       └── Models/
│           └── Codable structs     — JSON models matching Rust types.rs
```

**Invocation protocol:**

```bash
aleph-bridge notes list --folder "个人"
aleph-bridge calendar events --from "2026-03-21" --to "2026-03-28"
aleph-bridge reminders create --title "买咖啡" --list "生活"
aleph-bridge contacts search --query "张三"
```

- Input: CLI arguments
- Output: stdout JSON
- Errors: stderr + non-zero exit code

**Rust-side SwiftBridge:**

```rust
pub struct SwiftBridge {
    binary_path: PathBuf,
}

impl SwiftBridge {
    pub async fn call<T: DeserializeOwned>(
        &self,
        domain: &str,
        action: &str,
        args: &[(&str, &str)]
    ) -> Result<T> {
        let output = tokio::process::Command::new(&self.binary_path)
            .arg(domain)
            .arg(action)
            .args(args.iter().flat_map(|(k, v)| [format!("--{}", k), v.to_string()]))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("bridge error: {}", err));
        }

        serde_json::from_slice(&output.stdout)
            .map_err(|e| anyhow!("bridge parse error: {}", e))
    }
}
```

**Build & distribution:**
- `aleph-bridge` built alongside `aleph-server`
- `just build` runs `swift build` first, then `cargo build`
- Binary placed in same directory as `aleph-server` or `~/.aleph/bin/`
- First run requires macOS permission grants (Calendar/Contacts/Reminders system dialogs)

## §4: Builtin Tool Registration

Split existing `DesktopTool` into four domain-specific builtin tools:

```
src/builtin_tools/desktop/
├── mod.rs          — Unified re-export
├── screen.rs       — ScreenTool
├── pim.rs          — PimTool
├── system.rs       — SystemTool
└── automation.rs   — AutomationTool
```

Each tool follows the `AlephTool` trait pattern with tagged enum args:

```rust
pub struct PimTool {
    platform: Arc<dyn DesktopPlatform>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "action")]
pub enum PimArgs {
    #[serde(rename = "notes_list")]
    NotesList { folder: Option<String> },
    #[serde(rename = "notes_read")]
    NotesRead { note_id: String },
    #[serde(rename = "notes_create")]
    NotesCreate { title: String, body: String, folder: Option<String> },
    // ... all PIM actions
}

impl AlephTool for PimTool {
    const NAME: &'static str = "pim";
    const DESCRIPTION: &'static str = "Personal information management: notes, calendar, reminders, contacts";
    type Args = PimArgs;
    type Output = serde_json::Value;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let pim = self.platform.pim()
            .ok_or_else(|| anyhow!("PIM not available on {}", self.platform.platform_name()))?;
        match args { /* dispatch to trait methods */ }
    }

    fn requires_confirmation(&self) -> bool { true }
}
```

**Registered in BuiltinToolRegistry:**

```rust
pub(crate) screen_tool: ScreenTool,
pub(crate) pim_tool: PimTool,
pub(crate) system_tool: SystemTool,
pub(crate) automation_tool: AutomationTool,
```

**Approval policy:**
- Read operations (list/read/search) → auto-allow
- Write operations (create/update/delete/click/type) → require confirmation
- Uses existing `ApprovalPolicy` trait, distinguished by action variant

**Unavailable platform behavior:**
- `platform.pim()` returns `None` → tool still registered, returns clear error: `"PIM capabilities are not available on Linux. Install a community plugin to add this support."`

## §5: Migration & Cleanup Path

Progressive replacement, not big-bang deletion:

1. **Phase 2 completes** → all Tauri bridge callers switched to native `ScreenCapability`
2. **Remove bridge call path** — delete `DesktopBridgeClient` fallback logic from tools
3. **Remove Tauri process management** — delete server code that spawns Tauri subprocess
4. **Remove UDS protocol** — delete `bridge.sock` related code
5. **Phase 4 final cleanup:**
   - Delete `apps/tauri/` entirely
   - Remove tauri members from workspace `Cargo.toml`
   - Delete Tauri-only code in `apps/shared/`
   - Clean CI/CD Tauri build steps
   - Update ARCHITECTURE.md and other docs

**Verification criteria:**
- `grep -r "tauri" --include="*.rs" --include="*.toml"` returns zero results (excluding docs)
- `grep -r "bridge.sock"` returns zero results
- All three platforms compile: `cargo check` on macOS, cross-check configs for Linux/Windows

## §6: Phased Delivery

| Phase | Content | Deliverables | Dependencies |
|-------|---------|-------------|--------------|
| **1** | Architecture scaffold | `desktop/` trait hierarchy + `desktop-macos/` `desktop-linux/` `desktop-windows/` crate skeletons + `apps/macos-bridge/` Swift CLI skeleton + 4 builtin tools registered (empty shells) | None |
| **2** | Screen control native | `ScreenCapability` macOS/Linux/Windows implementation (migrated from Tauri), `ScreenTool` fully functional, bridge fallback removed | Phase 1 |
| **3** | macOS PIM & system | Swift CLI subcommands implemented, `PimTool` `SystemTool` `AutomationTool` fully functional, build integration (`just build` includes swift build) | Phase 1 |
| **4** | Tauri removal | Delete `apps/tauri/`, clean all Tauri references, update docs | Phase 2 |

**Phase 2 and Phase 3 can run in parallel** — both depend only on Phase 1's trait skeleton.

**Out of scope:**
- Linux/Windows capability implementations (community plugins)
- Plugin API for supplementing platform capabilities (existing plugin system sufficient)
- Halo/UI changes (this design is pure backend)
