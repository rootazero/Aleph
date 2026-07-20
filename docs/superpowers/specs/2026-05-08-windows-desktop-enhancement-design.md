# Windows Desktop Capability Enhancement — Design Spec

> **Date**: 2026-05-08  
> **Scope**: `desktop/windows` crate  
> **Approach**: Core-First (System + Automation + EscapeAbort)  
> **Status**: Draft awaiting review

---

## 1. Background & Problem Statement

The `desktop/windows` crate is currently a minimal stub. It provides only:

- `PowerCapability` — sleep inhibition via `SetThreadExecutionState`
- `ScreenCapability` — via shared `NativeScreen` (already cross-platform)

All other capabilities return `None`:

```rust
fn system(&self) -> Option<&dyn SystemCapability> { None }
fn automation(&self) -> Option<&dyn AutomationCapability> { None }
fn escape_listener(&self) -> Option<&dyn EscapeAbort> { None }
```

By contrast, `desktop/macos` implements **all 8 capability traits** with rich native integration (SwiftBridge, objc2, CoreGraphics). This gap means Windows users cannot:

- Launch/quit applications programmatically
- Read/write system clipboard
- Send native toast notifications
- Execute PowerShell scripts or Shortcuts
- Press Escape to abort AI desktop control

This spec targets the **Core-First** approach: implement `SystemCapability`, `AutomationCapability`, and `EscapeAbort` for Windows, reusing existing shared infrastructure wherever possible.

---

## 2. Scope

### 2.1 In Scope

| Capability | Trait | Windows API / Approach |
|---|---|---|
| **System** | `SystemCapability` | Win32: `ShellExecuteExW`, `EnumWindows`, `GetLastInputInfo`, Windows Runtime (`windows-rs`): `Windows.UI.Notifications` for toasts, `clipboard-win` crate for clipboard |
| **Automation** | `AutomationCapability` | `powershell.exe -Command`, `cmd.exe /C`, `Windows Terminal` fallback. PowerShell as primary script language. |
| **Escape Abort** | `EscapeAbort` | Raw Win32: low-level keyboard hook (`SetWindowsHookExW` + `WH_KEYBOARD_LL`) |
| **Cleanup** | N/A | Fix `WindowsPlatform` test assertions; remove stale comments |

### 2.2 Out of Scope (Phase 2)

| Capability | Reason |
|---|---|
| `MediaCapability` | Requires camera/audio capture APIs (MediaFoundation); large effort, separate PR |
| `PimCapability` | No native Windows PIM store equivalent (Windows Mail/Calendar are UWP apps without public APIs) |
| `PermissionCapability` | Windows permission model differs fundamentally from macOS TCC; needs separate design |
| `AccessibilityCapability` | UI Automation API is large; separate design needed |

### 2.3 Explicitly Not Doing

- **No new heavy dependencies**: Only add lightweight, well-maintained crates (`clipboard-win`, `windows-rs` features already available)
- **No breaking changes**: All existing `DesktopPlatform` trait signatures remain unchanged
- **No SwiftBridge equivalent**: Windows does not need an external helper process; everything is done in-process via Win32/WinRT
- **No registry editing or system modification**: Stay within user-level APIs

---

## 3. Architecture

### 3.1 Design Principles

1. **Reuse before invent**: `NativeScreen` already handles screenshot, mouse, keyboard, OCR. Do not duplicate.
2. **Follow macOS patterns**: Where macOS uses `tokio::task::spawn_blocking` for sync APIs, Windows does the same for Win32 calls.
3. **Minimal surface area**: Each capability is a single struct with thin trait implementations.
4. **Error propagation**: Use `DesktopError` variants; never panic in production code.

### 3.2 Module Structure

```
desktop/windows/src/
├── lib.rs                  # WindowsPlatform aggregator (expanded)
├── sleep_inhibitor.rs      # Existing — unchanged
├── system.rs               # NEW: WindowsSystem (SystemCapability)
├── automation.rs           # NEW: WindowsAutomation (AutomationCapability)
├── escape_listener.rs      # NEW: WindowsEscapeListener (EscapeAbort)
└── tests/                  # NEW: platform-specific tests
```

### 3.3 Platform Comparison

| macOS | Windows | Notes |
|---|---|---|
| `MacOSSystem` | `WindowsSystem` | Both use `spawn_blocking` for sync native APIs |
| `MacOSAutomation` (AppleScript/Shell) | `WindowsAutomation` (PowerShell/Shell) | PowerShell replaces AppleScript as primary script language |
| `EscapeListener` (CGEvent) | `WindowsEscapeListener` (`WH_KEYBOARD_LL`) | Low-level hook on Windows vs. CoreGraphics event tap on macOS |

---

## 4. Component Design

### 4.1 `WindowsSystem` — System Capability

```rust
pub struct WindowsSystem {
    _private: (),
}

#[async_trait]
impl SystemCapability for WindowsSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()>;
    async fn quit_app(&self, app_name: &str) -> Result<()>;
    async fn list_running_apps(&self) -> Result<Vec<AppInfo>>;
    async fn send_notification(&self, title: &str, body: &str) -> Result<()>;
    async fn clipboard_read(&self) -> Result<ClipboardContent>;
    async fn clipboard_write(&self, text: &str) -> Result<()>;
    async fn system_info(&self) -> Result<SystemInfo>;
    async fn user_idle_seconds(&self) -> Result<f64>;
}
```

**Implementation details:**

| Method | Windows API | Crate / FFI |
|---|---|---|
| `launch_app` | `ShellExecuteExW` with `"open"` verb | `windows-rs` (`Win32::UI::Shell`) |
| `quit_app` | Post `WM_CLOSE` to target window via `EnumWindows` + `GetWindowTextW` | Raw Win32 FFI |
| `list_running_apps` | `EnumWindows` + filter visible windows + `GetWindowThreadProcessId` → `OpenProcess` → `QueryFullProcessImageNameW` | Raw Win32 FFI |
| `send_notification` | `Windows.UI.Notifications.ToastNotificationManager` (WinRT) | `windows-rs` ( `"Foundation"`, `"UI_Notifications"` ) |
| `clipboard_read` | `GetClipboardData` + `GlobalLock` | `clipboard-win` crate (lightweight, well-tested) |
| `clipboard_write` | `SetClipboardData` | `clipboard-win` crate |
| `system_info` | `GetComputerNameExW`, `GetVersionExW` (or `RtlGetVersion` for Win10+), `GlobalMemoryStatusEx`, `GetDiskFreeSpaceExW` | `windows-rs` + `sysinfo` crate (if needed) |
| `user_idle_seconds` | `GetLastInputInfo` → subtract from `GetTickCount` | Raw Win32 FFI |

**Error handling:**
- Win32 errors → `DesktopError::SystemFailed(format!("Win32 error {code}: {msg}"))`
- Clipboard errors → `DesktopError::InputFailed(...)`
- Process not found → `DesktopError::NotFound(...)`

### 4.2 `WindowsAutomation` — Automation Capability

```rust
pub struct WindowsAutomation {
    _private: (),
}

#[async_trait]
impl AutomationCapability for WindowsAutomation {
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String>;
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>>;
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String>;
}
```

**Implementation details:**

| Method | Implementation |
|---|---|
| `run_script` | PowerShell: `powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "{source}"`  <br>Shell: `cmd.exe /C {source}`  <br>AppleScript/JXA: return `NotImplemented` |
| `list_shortcuts` | PowerShell: `Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs"` — list `.lnk` files  <br>Return simplified `ShortcutInfo { name, id: None, description: None }` |
| `run_shortcut` | Resolve `.lnk` path → `ShellExecuteExW` with the `.lnk` target  <br>OR use `WScript.Shell` COM object to execute shortcut |

**Security note:**
- `ExecutionPolicy Bypass` is required for unsigned scripts; this is acceptable because the caller (Aleph agent) is trusted.
- Shell execution is marked with `// SECURITY:` comment referencing the macOS equivalent.

### 4.3 `WindowsEscapeListener` — Escape Abort

```rust
pub struct WindowsEscapeListener {
    aborted: AtomicBool,
    hook_handle: Option<AtomicPtr<HHOOK__>>, // or use AtomicIsize for HHOOK
}

impl EscapeAbort for WindowsEscapeListener {
    fn start(&self) -> Result<()>;
    fn stop(&self);
    fn is_aborted(&self) -> bool;
    fn reset(&self);
}
```

**Implementation details:**

- `start()`: Call `SetWindowsHookExW(WH_KEYBOARD_LL, hook_proc, module_handle, 0)`.
- `hook_proc`: Check `wParam == WM_KEYDOWN` and `vkCode == VK_ESCAPE`. If so, set `aborted = true` and do **not** forward to next hook (consume the keypress).
- `stop()`: Call `UnhookWindowsHookEx`.
- Thread safety: `AtomicBool` for `aborted`; `Send + Sync` via atomic types.
- **Critical**: The hook procedure must be in a DLL or use a static function with proper calling convention (`extern "system"`). Since `desktop-windows` is a DLL (crate-type includes `cdylib`/`dylib`), this is feasible.

---

## 5. Data Flow

### 5.1 Application Launch

```
Agent Tool Call
    → WindowsPlatform::system()
    → WindowsSystem::launch_app("notepad")
        → tokio::task::spawn_blocking
            → ShellExecuteExW(lpFile="notepad", lpOperation="open")
        → Result::Ok(())
```

### 5.2 PowerShell Script Execution

```
Agent Tool Call
    → WindowsPlatform::automation()
    → WindowsAutomation::run_script(PowerShell, "Get-Date")
        → tokio::process::Command("powershell.exe")
            → stdout captured
        → Result::Ok("Friday, May 8, 2026 ...")
```

### 5.3 Escape Key Detection

```
User presses Escape
    → OS dispatches to WH_KEYBOARD_LL hook
    → WindowsEscapeListener::hook_proc
        → vkCode == VK_ESCAPE?
            → aborted.store(true, SeqCst)
            → return 1 (consume keypress)
    → Agent checks is_aborted() between actions
        → true → abort sequence, call reset()
```

---

## 6. Dependencies

### 6.1 New Dependencies (`desktop/windows/Cargo.toml`)

```toml
[dependencies]
# Existing
aleph-desktop = { path = "../shared" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt", "process"] }  # Add "process" feature

# New — lightweight, well-maintained
clipboard-win = "5"        # Clipboard read/write
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_UI_Shell",
    "Win32_UI_WindowsAndMessaging",
    "Win32_System_Threading",
    "Win32_System_SystemInformation",
    "Win32_Globalization",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_System_DataExchange",      # Clipboard APIs if not using clipboard-win
    "Win32_System_Com",
    "Win32_System_Ole",
    "Foundation",
    "UI_Notifications",
] }
```

### 6.2 Dependency Justification

| Crate | Size | Purpose | Alternative Considered |
|---|---|---|---|
| `clipboard-win` | ~50KB | Clipboard operations | Raw Win32 FFI — rejected because `clipboard-win` handles all edge cases (Unicode, format negotiation) |
| `windows` (Microsoft) | ~20MB (build-time only) | Win32/WinRT FFI bindings | `winapi` — rejected because `windows-rs` is the officially supported modern binding |

**No `sysinfo` crate**: `system_info()` will use raw Win32 APIs to avoid adding another dependency. If complexity grows, `sysinfo` can be added in Phase 2.

---

## 7. Error Handling

### 7.1 New Error Variants (if needed)

No new `DesktopError` variants are required. Existing variants suffice:

- `DesktopError::SystemFailed(String)` — for Win32 API failures
- `DesktopError::InputFailed(String)` — for clipboard/script failures
- `DesktopError::NotImplemented(String)` — for AppleScript/JXA on Windows
- `DesktopError::NotFound(String)` — for app/window not found

### 7.2 Win32 Error Conversion

```rust
fn win32_err<T>(result: windows::core::Result<T>, context: &str) -> Result<T> {
    result.map_err(|e| DesktopError::SystemFailed(format!("{context}: {e}")))
}
```

---

## 8. Testing Strategy

### 8.1 Unit Tests (in-module)

| Test | Description |
|---|---|
| `system_info_returns_valid` | Verify `WindowsSystem::system_info()` returns non-empty strings |
| `clipboard_roundtrip` | Write → Read → assert_eq |
| `user_idle_seconds_non_negative` | Verify idle time >= 0 |
| `escape_listener_lifecycle` | start → is_aborted (false) → stop |
| `automation_powershell_echo` | `run_script(PowerShell, "Write-Output hello")` == "hello" |
| `automation_shell_echo` | `run_script(Shell, "echo hello")` == "hello" |
| `automation_applescript_not_implemented` | Returns `NotImplemented` |

### 8.2 Integration Tests

- `desktop/windows/tests/system_e2e.rs` — Launch notepad, verify it appears in `list_running_apps`, then quit it.
- `desktop/windows/tests/automation_e2e.rs` — Run a PowerShell script that creates a temp file, verify file exists.

### 8.3 CI Considerations

- Windows tests run on `windows-latest` GitHub Actions runner.
- `send_notification` test may fail headlessly; mark with `#[ignore]` or check `CI` env var.
- `launch_app` tests use `notepad.exe` (guaranteed present on Windows).

---

## 9. Cleanup Work

### 9.1 `WindowsPlatform` Test Fixes

Current test in `desktop/windows/src/lib.rs`:

```rust
#[test]
fn screen_is_some() {
    let platform = WindowsPlatform::new();
    assert!(platform.screen().is_some());
    assert!(platform.pim().is_none());      // Still true
    assert!(platform.system().is_none());   // MUST CHANGE to is_some()
    assert!(platform.automation().is_none()); // MUST CHANGE to is_some()
}
```

After implementation:
- `system()` → `is_some()`
- `automation()` → `is_some()`
- Add `assert!(platform.escape_listener().is_some())`

### 9.2 Code Quality

- Remove any `TODO` or `FIXME` comments in existing code.
- Ensure all `unsafe` blocks have `// SAFETY:` comments.
- Run `cargo fmt` and `cargo clippy` before final commit.

---

## 10. Migration Plan

| Step | Action | Files Changed |
|---|---|---|
| 1 | Add dependencies to `Cargo.toml` | `desktop/windows/Cargo.toml` |
| 2 | Implement `WindowsSystem` | New: `desktop/windows/src/system.rs` |
| 3 | Implement `WindowsAutomation` | New: `desktop/windows/src/automation.rs` |
| 4 | Implement `WindowsEscapeListener` | New: `desktop/windows/src/escape_listener.rs` |
| 5 | Wire into `WindowsPlatform` | `desktop/windows/src/lib.rs` |
| 6 | Update tests | `desktop/windows/src/lib.rs` |
| 7 | Add e2e tests | New: `desktop/windows/tests/*.rs` |
| 8 | Run `cargo check`, `cargo clippy`, `cargo test` | All |

---

## 11. Open Questions

1. **Toast notifications on headless CI**: Should `send_notification` silently succeed (no-op) when no GUI session exists, or return an error?
2. **PowerShell execution policy**: Is `Bypass` acceptable, or should we use `RemoteSigned`?
3. **Escape key consumption**: Should the Escape key be consumed (not forwarded to the active app) when abort is triggered, or should it pass through?

---

## 12. Risks & Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| `windows-rs` compile time increase | Medium | Use minimal feature set; compile only in `desktop-windows` crate |
| WH_KEYBOARD_LL hook requires message pump | High | Document that `EscapeAbort` requires a running message loop; return clear error if not in GUI thread |
| WinRT toast notifications fail on older Windows | Low | Gracefully degrade to `DesktopError::SystemFailed` with helpful message |

---

*End of spec. Ready for implementation plan.*
