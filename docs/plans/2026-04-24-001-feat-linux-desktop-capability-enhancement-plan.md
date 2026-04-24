---
title: Linux Desktop Capability Enhancement
type: feat
status: active
date: 2026-04-24
origin: docs/superpowers/specs/2026-04-24-linux-desktop-capability-enhancement-design.md
---

# Linux Desktop Capability Enhancement — Implementation Plan

## Overview

Transform Aleph's Linux desktop implementation from a 78-line stub (only `ScreenCapability` via `NativeScreen`) into a full-featured platform layer matching macOS capabilities. This plan implements `SystemCapability`, `AutomationCapability`, `PermissionCapability`, and `EscapeAbort` for Linux, following Aleph's existing trait-based architecture.

**Key constraint**: Aleph is a background AI assistant, not a desktop app — no UI elements (menu bar, tray, floating windows).

## Problem Frame

Current `desktop/linux/src/lib.rs` returns `None` for all capabilities except `screen()`:
- `system()` → `None` (no notifications, app management, clipboard, system info)
- `automation()` → `None` (no script execution)
- `permission()` → `None` (no permission checks)
- `escape_listener()` → `None` (no emergency abort)

This makes Linux a second-class citizen compared to macOS where all capabilities are fully implemented.

## Requirements Trace

- R1. `SystemCapability` fully implemented for Linux (notification, app management, clipboard, system info, idle detection)
- R2. `AutomationCapability` implemented (Shell/Python script execution)
- R3. `PermissionCapability` implemented (xdg-desktop-portal integration)
- R4. `EscapeAbort` implemented (evdev/xinput keyboard listener)
- R5. `PimCapability` explicitly returns `None` (Linux PIM too fragmented)
- R6. `MediaCapability` explicitly returns `None` (deferred to future专项)
- R7. OCR works on Linux via tesseract (replace `NotImplemented`)
- R8. Sleep inhibition works on Linux (systemd-inhibit)
- R9. All capabilities use modern APIs (D-Bus/xdg-desktop-portal) with X11 fallbacks
- R10. Wayland compatibility considered for all new implementations
- R11. Old code cleaned up (remove duplicate clipboard in action/input.rs, deprecate ScreenCapability clipboard methods)
- R12. Unit tests for all new modules
- R13. CI passes on Linux (`cargo check`, `cargo test`, `cargo clippy`)

## Scope Boundaries

| Out of Scope | Reason |
|-------------|--------|
| Menu bar / system tray / Halo floating window | Aleph is background-only |
| PIM (Notes/Calendar/Reminders/Contacts) | Linux PIM fragmented (Evolution/KOrganizer/Thunderbird) |
| Media (Camera/Audio/STT) | Complex Linux media stack (V4L2/PipeWire), deferred |
| Full Accessibility API (AX tree) | AT-SPI unstable, Wayland support poor |
| Windows implementation | Separate scope |
| Swift Bridge / JSON-RPC process model | macOS-only architecture |

## Context & Research

### Existing Code Structure

```
desktop/linux/
├── Cargo.toml              # Minimal deps: aleph-desktop, async-trait, tokio
└── src/
    └── lib.rs              # 78-line stub: LinuxPlatform { screen: NativeScreen }

desktop/shared/src/
├── traits/
│   ├── system.rs           # SystemCapability trait (launch_app, quit_app, list_running_apps, send_notification, clipboard_read/write, system_info, user_idle_seconds)
│   ├── automation.rs       # AutomationCapability trait (run_script, list_shortcuts, run_shortcut)
│   ├── permission.rs       # PermissionCapability trait (check, check_all, request)
│   └── screen.rs           # ScreenCapability trait (screenshot, ocr, click, type_text, etc.)
├── system_types.rs         # AppInfo, ClipboardContent, SystemInfo
├── automation_types.rs     # ScriptLanguage, ShortcutInfo
├── permission_types.rs     # TccPermission, PermissionStatus, PermissionInfo
├── native_screen.rs        # NativeScreen: implements ScreenCapability via perception/action modules
├── platform.rs             # DesktopPlatform trait + EscapeAbort trait
└── error.rs                # DesktopError enum

desktop/macos/src/
├── lib.rs                  # MacOSPlatform: all capabilities return Some
├── system/
│   ├── mod.rs              # MacOSSystem
│   ├── notification.rs     # NSUserNotification
│   ├── app_management.rs   # NSWorkspace
│   ├── clipboard.rs        # NSPasteboard
│   ├── sysinfo.rs          # NSProcessInfo
│   └── workspace.rs        # Additional workspace ops
├── automation.rs           # MacOSAutomation (AppleScript/JXA)
├── permission.rs           # MacOSPermission (TCC)
├── escape_listener.rs      # EscapeListener (NSEvent)
└── ...
```

### Key Reference Files

- `desktop/macos/src/lib.rs` — Platform aggregation pattern (follow exactly)
- `desktop/macos/src/system/mod.rs` — SystemCapability implementation pattern
- `desktop/shared/src/traits/system.rs` — Trait contract to implement
- `src/builtin_tools/desktop/mod.rs` — How DesktopTool uses capabilities

### Current Linux Dependencies

```toml
[dependencies]
aleph-desktop = { path = "../shared" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt"] }
```

## Key Technical Decisions

- **Pure Rust implementation**: No Swift/ObjC bridge, all capabilities in Rust
- **Modern API priority**: D-Bus / xdg-desktop-portal first, X11 fallback second
- **Wayland-aware**: All implementations must work on Wayland (even if degraded)
- **Zero UI**: No windows, menus, or tray icons — all interaction via LLM conversation
- **Cleanup-first**: Remove old code as we go, avoid accumulation

## Implementation Units

### Unit 1: LinuxSystem — Core Structure

**Goal:** Create `LinuxSystem` struct and implement basic `SystemCapability` methods

**Requirements:** R1

**Dependencies:** None

**Files:**
- Create: `desktop/linux/src/system/mod.rs`
- Modify: `desktop/linux/src/lib.rs`

**Approach:**
- Create `LinuxSystem` struct (empty for now, will hold state later)
- Implement `SystemCapability` trait for `LinuxSystem`
- Start with `system_info()` using `sysinfo` crate
- Stub other methods with `NotImplemented` errors
- Update `LinuxPlatform` to return `Some(&self.system)`

**Patterns to follow:**
- `desktop/macos/src/system/mod.rs` — MacOSSystem structure
- `desktop/macos/src/lib.rs` — Platform aggregation

**Test scenarios:**
- Happy path: `LinuxSystem::new()` creates successfully
- Happy path: `system_info()` returns correct OS name "Linux"
- Edge case: `system_info()` handles missing hostname gracefully

**Verification:**
- `cargo check -p aleph-desktop-linux` passes
- Unit tests pass

---

### Unit 2: Notifications

**Goal:** Implement `send_notification()` using `notify-rust`

**Requirements:** R1

**Dependencies:** Unit 1

**Files:**
- Create: `desktop/linux/src/system/notification.rs`
- Modify: `desktop/linux/src/system/mod.rs`

**Approach:**
- Add `notify-rust = "4"` to `desktop/linux/Cargo.toml`
- Implement `send_notification(title, body)` using D-Bus Notification protocol
- Fallback to `notify-send` CLI if D-Bus unavailable
- Use app name "Aleph"

**Patterns to follow:**
- `desktop/macos/src/system/notification.rs` — Notification structure

**Test scenarios:**
- Happy path: Notification sends successfully (mock D-Bus)
- Error path: D-Bus unavailable returns graceful error
- Edge case: Empty title/body handled

**Verification:**
- `cargo check` passes with new dependency
- Unit tests pass (mock D-Bus)

---

### Unit 3: App Management

**Goal:** Implement `launch_app()`, `quit_app()`, `list_running_apps()`

**Requirements:** R1

**Dependencies:** Unit 1

**Files:**
- Create: `desktop/linux/src/system/app_management.rs`
- Modify: `desktop/linux/src/system/mod.rs`

**Approach:**
- **Launch**: Parse `.desktop` files from `/usr/share/applications/` and `~/.local/share/applications/`
  - Use `gtk-launch <desktop-file-id>` if available
  - Fallback: extract `Exec=` line and execute directly
- **Quit**: Use `killall <name>` (safer than `pkill -f`)
- **List running**: Parse `/proc/*/status` to find processes matching .desktop entries

**Technical design:**
```rust
// .desktop file parser
struct DesktopEntry {
    name: String,
    exec: String,
    icon: Option<String>,
    categories: Vec<String>,
}

fn parse_desktop_file(path: &Path) -> Result<DesktopEntry>
fn find_desktop_file(app_name: &str) -> Result<PathBuf>
```

**Patterns to follow:**
- `desktop/macos/src/system/app_management.rs` — App management structure

**Test scenarios:**
- Happy path: Parse valid .desktop file
- Happy path: `launch_app("firefox")` executes gtk-launch
- Error path: App not found returns error
- Edge case: .desktop file with `%u` placeholder handled
- Edge case: Multiple .desktop files with same name

**Verification:**
- Unit tests for .desktop parsing
- Integration test for launch/quit (marked `#[ignore]`)

---

### Unit 4: Clipboard

**Goal:** Implement `clipboard_read()` and `clipboard_write()` using `arboard`

**Requirements:** R1, R11

**Dependencies:** Unit 1

**Files:**
- Create: `desktop/linux/src/system/clipboard.rs`
- Modify: `desktop/linux/src/system/mod.rs`
- Modify: `desktop/shared/src/traits/screen.rs` (deprecate clipboard methods)
- Modify: `desktop/shared/src/action/input.rs` (remove Linux clipboard code)

**Approach:**
- Add `arboard = "3"` to `desktop/linux/Cargo.toml`
- Implement clipboard methods using arboard (auto-detects X11/Wayland)
- **Cleanup**: Mark `ScreenCapability::clipboard_read/write` as `#[deprecated]`
- **Cleanup**: Remove duplicate clipboard code from `action/input.rs`

**Patterns to follow:**
- `desktop/macos/src/system/clipboard.rs` — Clipboard structure

**Test scenarios:**
- Happy path: Read/write text clipboard
- Edge case: Clipboard empty returns `None` text
- Edge case: Non-UTF8 content handled gracefully

**Verification:**
- `cargo check` passes
- Unit tests pass
- `action/input.rs` no longer has clipboard code

---

### Unit 5: System Info & Idle Detection

**Goal:** Implement `system_info()` and `user_idle_seconds()`

**Requirements:** R1, R9

**Dependencies:** Unit 1

**Files:**
- Create: `desktop/linux/src/system/sysinfo.rs`
- Create: `desktop/linux/src/system/idle_detection.rs`
- Modify: `desktop/linux/src/system/mod.rs`

**Approach:**
- **System info**: Use `sysinfo` crate (already used elsewhere in Aleph)
  - `sysinfo::System::new_all()` for OS version, hostname
  - `std::env::consts::ARCH` for architecture
  - `std::env::var("USER")` for username
- **Idle detection**: Multi-backend fallback
  1. X11: `xprintidle` CLI (returns milliseconds)
  2. Wayland + GNOME: D-Bus `org.gnome.Mutter.IdleMonitor`
  3. Wayland + KDE: D-Bus `org.kde.KWin` (if available)
  4. Generic: `logind` D-Bus interface
  5. Return `NotImplemented` if none available

**Patterns to follow:**
- `desktop/macos/src/system/sysinfo.rs` — System info structure

**Test scenarios:**
- Happy path: `system_info()` returns valid data
- Happy path: `user_idle_seconds()` returns value with xprintidle
- Error path: No idle detection backend returns `NotImplemented`
- Edge case: xprintidle returns invalid output

**Verification:**
- Unit tests for system info
- Integration test for idle detection (marked `#[ignore]`)

---

### Unit 6: LinuxAutomation

**Goal:** Implement `AutomationCapability` for Linux

**Requirements:** R2

**Dependencies:** None (parallel to Unit 1-5)

**Files:**
- Create: `desktop/linux/src/automation.rs`
- Modify: `desktop/linux/src/lib.rs`

**Approach:**
- Create `LinuxAutomation` struct
- Implement `run_script()`:
  - `ScriptLanguage::Shell` → `bash -c "script"`
  - `ScriptLanguage::Python` → `python3 -c "script"`
  - Others → `NotImplemented`
- `list_shortcuts()` → return empty vec (Linux has no Shortcuts equivalent)
- `run_shortcut()` → `NotImplemented`
- Update `LinuxPlatform` to return `Some(&self.automation)`

**Patterns to follow:**
- `desktop/macos/src/automation.rs` — MacOSAutomation structure

**Test scenarios:**
- Happy path: Execute shell script successfully
- Happy path: Execute Python script successfully
- Error path: Script returns non-zero exit code
- Error path: Unsupported language returns error
- Edge case: Script with stderr captured

**Verification:**
- Unit tests with mock Command
- Integration tests marked `#[ignore]`

---

### Unit 7: LinuxPermission

**Goal:** Implement `PermissionCapability` for Linux

**Requirements:** R3

**Dependencies:** None (parallel)

**Files:**
- Create: `desktop/linux/src/permission.rs`
- Modify: `desktop/linux/src/lib.rs`

**Approach:**
- Create `LinuxPermission` struct
- `check()`:
  - `ScreenRecording` → `Unknown` (Linux has no persistent permission state; portal handles it on-demand)
  - Others → `Unknown` with `can_request: false`
- `check_all()` → return vec of all permissions with `Unknown`
- `request()` → same as `check()` (Linux permissions are operation-triggered, not explicitly requested)
- Update `LinuxPlatform` to return `Some(&self.permission)`

**Patterns to follow:**
- `desktop/macos/src/permission.rs` — MacOSPermission structure

**Test scenarios:**
- Happy path: `check(ScreenRecording)` returns `Unknown` with `can_request: true`
- Happy path: `check(Camera)` returns `Unknown` with `can_request: false`
- Happy path: `check_all()` returns all 6 permissions

**Verification:**
- Unit tests pass

---

### Unit 8: EscapeAbort Listener

**Goal:** Implement `EscapeAbort` for Linux

**Requirements:** R4

**Dependencies:** None (parallel)

**Files:**
- Create: `desktop/linux/src/escape_listener.rs`
- Modify: `desktop/linux/src/lib.rs`

**Approach:**
- Create `LinuxEscapeListener` struct with `AtomicBool` for abort state
- Implement `EscapeAbort` trait:
  - `start()`: Spawn thread that listens for Escape key
  - `is_aborted()`: Check atomic flag
  - `reset()`: Clear atomic flag
  - `stop()`: Stop listener thread
- **Backend**: Use `evdev` crate to read keyboard events from `/dev/input/event*`
  - Requires user to be in `input` group or run as root
  - Fallback: Return `NotImplemented` with helpful error message

**Patterns to follow:**
- `desktop/macos/src/escape_listener.rs` — EscapeListener structure

**Test scenarios:**
- Happy path: Listener starts and stops
- Happy path: `is_aborted()` returns false initially
- Error path: No evdev permissions returns graceful error
- Edge case: Multiple start() calls handled

**Verification:**
- Unit tests for state management
- Integration test marked `#[ignore]` (requires keyboard)

---

### Unit 9: OCR (Tesseract)

**Goal:** Implement OCR for Linux using tesseract

**Requirements:** R7

**Dependencies:** None (modifies shared perception module)

**Files:**
- Create: `desktop/shared/src/perception/ocr_linux.rs`
- Modify: `desktop/shared/src/perception/mod.rs`

**Approach:**
- Create `ocr_linux.rs` with `perform_ocr(png_bytes: &[u8]) -> Result<OcrResult>`
- Write PNG to temp file
- Call `tesseract <tmp> stdout -l chi_sim+eng`
- Parse stdout for text
- Parse TSV output for bounding boxes (optional, Phase 1 can skip)
- Update `perception/mod.rs` to use Linux OCR on Linux instead of `NotImplemented`

**Patterns to follow:**
- `desktop/shared/src/perception/ocr_macos.rs` — macOS OCR implementation

**Test scenarios:**
- Happy path: OCR returns text from test image
- Error path: tesseract not installed returns graceful error
- Edge case: Empty image handled

**Verification:**
- Unit test with mock tesseract
- Integration test marked `#[ignore]`

---

### Unit 10: Sleep Inhibitor

**Goal:** Implement sleep inhibition for Linux

**Requirements:** R8

**Dependencies:** None (parallel)

**Files:**
- Create: `desktop/linux/src/sleep_inhibitor.rs`
- Modify: `desktop/linux/src/lib.rs`

**Approach:**
- Create `LinuxSleepInhibitor` struct
- `acquire()`:
  1. Try `systemd-inhibit --what=sleep --why="Aleph is working" --mode=block sleep infinity`
  2. Fallback: `gnome-session-inhibit --inhibit suspend --reason "Aleph"`
  3. Log warning if neither available
- `release()`: Kill child process
- Implement `Drop` to auto-release
- Note: This is not a capability trait, just a helper. Integrate into agent loop similar to macOS.

**Patterns to follow:**
- Codex sleep inhibition implementation

**Test scenarios:**
- Happy path: Inhibitor acquires and releases
- Error path: No inhibition backend logs warning
- Edge case: Double-release handled

**Verification:**
- Unit tests for state management
- Integration test marked `#[ignore]`

---

### Unit 11: Update LinuxPlatform & Dependencies

**Goal:** Wire all new capabilities into `LinuxPlatform`

**Requirements:** R1-R8

**Dependencies:** Units 1-10

**Files:**
- Modify: `desktop/linux/src/lib.rs`
- Modify: `desktop/linux/Cargo.toml`

**Approach:**
- Update `LinuxPlatform` struct to hold all new capability instances:
  ```rust
  pub struct LinuxPlatform {
      screen: NativeScreen,
      system: LinuxSystem,
      automation: LinuxAutomation,
      permission: LinuxPermission,
      escape: LinuxEscapeListener,
  }
  ```
- Update `DesktopPlatform` impl to return `Some` for all capabilities
- Add all new dependencies to `Cargo.toml`:
  ```toml
  notify-rust = "4"
  arboard = "3"
  sysinfo = "0.30"
  zbus = "4"
  evdev = "0.12"
  ```

**Test scenarios:**
- Happy path: All `LinuxPlatform` capabilities return `Some`
- Happy path: `platform_name()` returns "Linux"

**Verification:**
- `cargo check -p aleph-desktop-linux` passes
- `cargo test -p aleph-desktop-linux` passes

---

### Unit 12: Cleanup & Deprecation

**Goal:** Remove old/duplicate code

**Requirements:** R11

**Dependencies:** Unit 4 (clipboard migration)

**Files:**
- Modify: `desktop/shared/src/traits/screen.rs`
- Modify: `desktop/shared/src/action/input.rs`
- Modify: `desktop/shared/src/native_screen.rs`

**Approach:**
1. **Deprecate `ScreenCapability` clipboard methods**:
   ```rust
   #[deprecated(since = "2026.04.24", note = "Use SystemCapability::clipboard_read instead")]
   async fn clipboard_read(&self) -> Result<String>;
   ```
2. **Remove duplicate clipboard from `action/input.rs`**:
   - Delete `clipboard_read()` and `clipboard_write()` functions
   - These were Linux-specific stubs that never worked properly
3. **Update `native_screen.rs`**:
   - Keep `clipboard_read/write` implementations but mark internal methods deprecated
   - Or redirect to SystemCapability if available (decision needed)

**Patterns to follow:**
- Rust deprecation conventions

**Test scenarios:**
- Happy path: Code compiles without deprecated warnings in new code
- Happy path: No duplicate clipboard implementations

**Verification:**
- `cargo check` passes
- `cargo clippy` shows no errors (deprecated warnings expected)

---

### Unit 13: Unit Tests

**Goal:** Comprehensive unit tests for all new modules

**Requirements:** R12

**Dependencies:** Units 1-12

**Files:**
- Create/Modify: `desktop/linux/src/system/tests.rs`
- Create/Modify: `desktop/linux/src/tests.rs`

**Approach:**
- Add `#[cfg(test)]` modules in each source file
- Mock external dependencies (D-Bus, evdev, tesseract) where possible
- Test error paths and edge cases
- Use `rstest` for parameterized tests if already in workspace

**Test scenarios:**
- Happy path: Each capability method succeeds
- Error path: Each capability method fails gracefully
- Edge case: Missing system dependencies handled

**Verification:**
- `cargo test -p aleph-desktop-linux` passes
- Coverage > 80% for new code

---

### Unit 14: CI Verification

**Goal:** Ensure Linux build passes in CI

**Requirements:** R13

**Dependencies:** Units 1-13

**Files:**
- Modify: `.github/workflows/ci.yml` (if exists)

**Approach:**
- Ensure `cargo check -p aleph-desktop-linux` runs on Linux runner
- Ensure `cargo test -p aleph-desktop-linux` runs
- Install system dependencies if needed (libdbus-dev for notify-rust)

**Verification:**
- CI passes on PR

## System-Wide Impact

- **API compatibility**: All new implementations follow existing trait contracts; no breaking changes
- **DesktopTool**: Automatically gains Linux system/automation/permission capabilities
- **Error propagation**: New Linux errors use existing `DesktopError` variants
- **State lifecycle**: `LinuxSleepInhibitor` uses `Drop` for cleanup; `LinuxEscapeListener` has `stop()` method
- **Unchanged invariants**: 
  - `ScreenCapability` behavior unchanged (still uses `NativeScreen`)
  - `PimCapability` still returns `None` on Linux
  - `MediaCapability` still returns `None` on Linux

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Wayland compatibility issues | Medium | High | Use D-Bus/portal first, X11 fallback, graceful degradation |
| System dependencies not installed | High | Medium | Runtime detection, helpful error messages |
| evdev permissions (input group) | Medium | Medium | Detect permissions, return guidance message |
| tesseract not installed | Medium | Medium | Check at runtime, return `NotImplemented` with message |
| Dependency version conflicts | Low | Medium | Pin versions, test with workspace versions |

## Phased Delivery

### Phase 1: Foundation (Units 1-5)
- LinuxSystem structure
- Notifications
- App management
- Clipboard
- System info + idle detection

### Phase 2: Automation & Permissions (Units 6-7)
- LinuxAutomation
- LinuxPermission

### Phase 3: Escape & OCR (Units 8-9)
- LinuxEscapeListener
- Linux OCR (tesseract)

### Phase 4: Integration & Cleanup (Units 10-14)
- Sleep inhibitor
- Wire everything into LinuxPlatform
- Cleanup old code
- Tests & CI

## Documentation Plan

- Update `docs/reference/ARCHITECTURE.md`: Add Linux desktop capabilities section
- Update `docs/reference/DESKTOP_BRIDGE.md`: Note Linux uses pure Rust, no Swift Bridge
- Update `README.md`: Linux feature matrix

## Sources & References

- **Origin document:** `docs/superpowers/specs/2026-04-24-linux-desktop-capability-enhancement-design.md`
- **Reference implementation:** `desktop/macos/src/lib.rs` and submodules
- **Codex reference:** `/Volumes/TBU4/Github/codex/codex-rs/linux-sandbox/src/bwrap.rs` (sandbox only)
- **notify-rust docs:** https://docs.rs/notify-rust
- **arboard docs:** https://docs.rs/arboard
- **evdev docs:** https://docs.rs/evdev
