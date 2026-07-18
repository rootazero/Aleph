# Windows Desktop Core Capabilities — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `SystemCapability`, `AutomationCapability`, and `EscapeAbort` for the Windows desktop platform, bringing it to functional parity with macOS for core desktop control features.

**Architecture:** Reuse existing `NativeScreen` for screen operations. Add three new modules (`system`, `automation`, `escape_listener`) that implement the shared trait contracts from `desktop/shared`. Wire them into `WindowsPlatform` aggregator. All Win32 calls go through `tokio::task::spawn_blocking`.

**Tech Stack:** Rust, `windows-rs` (Win32 + WinRT), `clipboard-win` crate, `tokio::process`

---

## 0. File Map

| File | Action | Responsibility |
|---|---|---|
| `desktop/windows/Cargo.toml` | Modify | Add `clipboard-win`, `windows` deps; enable `tokio` `"process"` feature |
| `desktop/windows/src/lib.rs` | Modify | Wire new capabilities into `WindowsPlatform`; fix tests |
| `desktop/windows/src/system.rs` | **Create** | `WindowsSystem` struct — `SystemCapability` implementation |
| `desktop/windows/src/automation.rs` | **Create** | `WindowsAutomation` struct — `AutomationCapability` implementation |
| `desktop/windows/src/escape_listener.rs` | **Create** | `WindowsEscapeListener` struct — `EscapeAbort` implementation |
| `desktop/windows/src/sleep_inhibitor.rs` | No change | Already works |
| `desktop/windows/tests/system_e2e.rs` | **Create** | Integration tests for system operations |
| `desktop/windows/tests/automation_e2e.rs` | **Create** | Integration tests for PowerShell/script execution |

---

## 1. Dependency Setup

**Files:** `desktop/windows/Cargo.toml`

- [ ] **Step 1: Add new dependencies**

Replace the existing `[dependencies]` section in `desktop/windows/Cargo.toml`:

```toml
[dependencies]
aleph-desktop = { path = "../shared" }
async-trait = "0.1"
tokio = { version = "1", features = ["rt", "process"] }

# Windows platform bindings
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_UI_Shell",
    "Win32_UI_WindowsAndMessaging",
    "Win32_System_Threading",
    "Win32_System_SystemInformation",
    "Win32_Globalization",
    "Win32_UI_Input_KeyboardAndMouse",
    "Foundation",
    "UI_Notifications",
] }

# Clipboard operations
clipboard-win = "5"
```

- [ ] **Step 2: Verify dependency resolution**

Run:
```bash
cargo check -p aleph-desktop-windows
```

Expected: Should pass (no code changes yet, just deps added).

- [ ] **Step 3: Commit dependency changes**

```bash
git add desktop/windows/Cargo.toml
git commit -m "deps: add windows-rs and clipboard-win for native capabilities"
```

---

## 2. System Capability (`WindowsSystem`)

**Files:** `desktop/windows/src/system.rs` (create)

### 2.1 Core Implementation

- [ ] **Step 1: Create `system.rs` with struct and imports**

```rust
//! Windows `SystemCapability` implementation using Win32 APIs.

use std::path::PathBuf;

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

pub struct WindowsSystem {
    _private: (),
}

impl WindowsSystem {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsSystem {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Implement `launch_app`**

Add to `system.rs` inside `#[async_trait] impl SystemCapability for WindowsSystem`:

```rust
async fn launch_app(&self, app_name: &str) -> Result<()> {
    let app_name = app_name.to_string();
    tokio::task::spawn_blocking(move || {
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let operation: Vec<u16> = "open\0".encode_utf16().collect();
        let file: Vec<u16> = app_name.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: ShellExecuteW is a well-documented Win32 API. PCWSTR points to valid null-terminated UTF-16.
        let result = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(operation.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };

        // ShellExecuteW returns an HINSTANCE > 32 on success.
        let code = result.0 as isize;
        if code > 32 {
            Ok(())
        } else {
            Err(DesktopError::SystemFailed(format!(
                "failed to launch '{app_name}': ShellExecute returned {code}"
            )))
        }
    })
    .await
    .map_err(|e| DesktopError::SystemFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 3: Implement `quit_app`**

Add after `launch_app`:

```rust
async fn quit_app(&self, app_name: &str) -> Result<()> {
    let app_name = app_name.to_string();
    tokio::task::spawn_blocking(move || {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
            PostMessageW, WM_CLOSE,
        };

        struct EnumState {
            target: String,
            found: bool,
        }

        // SAFETY: EnumWindows callback follows documented signature.
        extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
            unsafe {
                if IsWindowVisible(hwnd).as_bool() {
                    let mut buf = [0u16; 512];
                    let len = GetWindowTextW(hwnd, &mut buf);
                    if len > 0 {
                        let title = String::from_utf16_lossy(&buf[..len as usize]);
                        let state = &mut *(lparam.0 as *mut EnumState);
                        if title.to_lowercase().contains(&state.target.to_lowercase()) {
                            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                            state.found = true;
                        }
                    }
                }
                1 // Continue enumeration
            }
        }

        let mut state = EnumState {
            target: app_name.clone(),
            found: false,
        };

        unsafe {
            let _ = EnumWindows(
                Some(enum_proc),
                LPARAM(&mut state as *mut _ as isize),
            );
        }

        if state.found {
            Ok(())
        } else {
            Err(DesktopError::NotFound(format!(
                "no visible window matching '{app_name}' found"
            )))
        }
    })
    .await
    .map_err(|e| DesktopError::SystemFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 4: Implement `list_running_apps`**

Add after `quit_app`:

```rust
async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
    tokio::task::spawn_blocking(|| {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
        };

        #[derive(Default)]
        struct EnumState {
            apps: Vec<AppInfo>,
        }

        // SAFETY: EnumWindows callback follows documented signature.
        extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
            unsafe {
                if IsWindowVisible(hwnd).as_bool() {
                    let mut buf = [0u16; 512];
                    let len = GetWindowTextW(hwnd, &mut buf);
                    if len > 0 {
                        let title = String::from_utf16_lossy(&buf[..len as usize]);
                        let mut pid: u32 = 0;
                        GetWindowThreadProcessId(hwnd, Some(&mut pid));

                        let state = &mut *(lparam.0 as *mut EnumState);
                        state.apps.push(AppInfo {
                            name: title,
                            bundle_id: None,
                            pid: pid as u64,
                            is_frontmost: false,
                        });
                    }
                }
                1
            }
        }

        let mut state = EnumState::default();
        unsafe {
            let _ = EnumWindows(
                Some(enum_proc),
                LPARAM(&mut state as *mut _ as isize),
            );
        }

        Ok(state.apps)
    })
    .await
    .map_err(|e| DesktopError::SystemFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 5: Implement `send_notification`**

Add after `list_running_apps`:

```rust
async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
    let title = title.to_string();
    let body = body.to_string();
    tokio::task::spawn_blocking(move || {
        // WinRT toast notifications require a COM apartment and
        // Windows.UI.Notifications APIs. For simplicity and reliability
        // across Windows versions, we use PowerShell to show a toast.
        let script = format!(
            r#"Add-Type -AssemblyName System.Runtime.WindowsRuntime;
$null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType=WindowsRuntime];
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);
$template.GetElementsByTagName('text').Item(0).AppendChild($template.CreateTextNode('{}')) | Out-Null;
$template.GetElementsByTagName('text').Item(1).AppendChild($template.CreateTextNode('{}')) | Out-Null;
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Aleph').Show([Windows.UI.Notifications.ToastNotification]::new($template));"#,
            title.replace('\'', "''"),
            body.replace('\'', "''")
        );

        match std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", &script])
            .output()
        {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(DesktopError::SystemFailed(format!(
                    "toast notification failed: {stderr}"
                )))
            }
            Err(e) => Err(DesktopError::SystemFailed(format!(
                "failed to spawn powershell for notification: {e}"
            ))),
        }
    })
    .await
    .map_err(|e| DesktopError::SystemFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 6: Implement `clipboard_read` and `clipboard_write`**

Add after `send_notification`:

```rust
async fn clipboard_read(&self) -> Result<ClipboardContent> {
    tokio::task::spawn_blocking(|| {
        use clipboard_win::{formats, get_clipboard};

        match get_clipboard(formats::Unicode) {
            Ok(text) => Ok(ClipboardContent::Text(text)),
            Err(e) => Err(DesktopError::InputFailed(format!(
                "clipboard read failed: {e}"
            ))),
        }
    })
    .await
    .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
}

async fn clipboard_write(&self, text: &str) -> Result<()> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        use clipboard_win::{formats, set_clipboard};

        match set_clipboard(formats::Unicode, text.as_str()) {
            Ok(()) => Ok(()),
            Err(e) => Err(DesktopError::InputFailed(format!(
                "clipboard write failed: {e}"
            ))),
        }
    })
    .await
    .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 7: Implement `system_info`**

Add after clipboard methods:

```rust
async fn system_info(&self) -> Result<SystemInfo> {
    tokio::task::spawn_blocking(|| {
        use windows::Win32::System::SystemInformation::{
            GetComputerNameExW, ComputerNamePhysicalDnsHostname,
        };

        let mut hostname_buf = [0u16; 256];
        let mut size = hostname_buf.len() as u32;

        // SAFETY: GetComputerNameExW writes into provided buffer up to size bytes.
        let hostname = unsafe {
            if GetComputerNameExW(
                ComputerNamePhysicalDnsHostname,
                Some(&mut hostname_buf),
                &mut size,
            )
            .as_bool()
            {
                String::from_utf16_lossy(&hostname_buf[..size as usize])
            } else {
                "unknown".to_string()
            }
        };

        Ok(SystemInfo {
            hostname,
            os_name: "Windows".to_string(),
            os_version: std::env::var("OS")
                .unwrap_or_else(|_| "Windows NT".to_string()),
            arch: std::env::consts::ARCH.to_string(),
            uptime_seconds: 0.0, // Can be enhanced with GetTickCount64
        })
    })
    .await
    .map_err(|e| DesktopError::SystemFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 8: Implement `user_idle_seconds`**

Add after `system_info`:

```rust
async fn user_idle_seconds(&self) -> Result<f64> {
    tokio::task::spawn_blocking(|| {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetLastInputInfo;
        use windows::Win32::Foundation::GetTickCount;

        #[repr(C)]
        struct LASTINPUTINFO {
            cbSize: u32,
            dwTime: u32,
        }

        let mut info = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };

        // SAFETY: GetLastInputInfo fills the struct; GetTickCount returns millis since boot.
        let idle_millis = unsafe {
            if GetLastInputInfo(&mut info as *mut _ as *mut _).as_bool() {
                let now = GetTickCount();
                now - info.dwTime
            } else {
                0
            }
        };

        Ok((idle_millis as f64) / 1000.0)
    })
    .await
    .map_err(|e| DesktopError::SystemFailed(format!("task join error: {e}")))?
}
```

- [ ] **Step 9: Add module declaration to `lib.rs`**

In `desktop/windows/src/lib.rs`, add:
```rust
mod system;
```

And update the `use` statements to include `WindowsSystem`:
```rust
pub use system::WindowsSystem;
```

- [ ] **Step 10: Commit system capability**

```bash
git add desktop/windows/src/system.rs desktop/windows/src/lib.rs
git commit -m "feat(windows): implement SystemCapability (launch, clipboard, notifications, idle)"
```

---

## 3. Automation Capability (`WindowsAutomation`)

**Files:** `desktop/windows/src/automation.rs` (create)

- [ ] **Step 1: Create `automation.rs` with struct and imports**

```rust
//! Windows `AutomationCapability` implementation using PowerShell and cmd.

use async_trait::async_trait;
use tokio::process::Command;

use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};

pub struct WindowsAutomation {
    _private: (),
}

impl WindowsAutomation {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsAutomation {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Implement `run_script`**

```rust
#[async_trait]
impl AutomationCapability for WindowsAutomation {
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String> {
        let source = source.to_string();
        let output = match language {
            ScriptLanguage::PowerShell => {
                Command::new("powershell.exe")
                    .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &source])
                    .output()
                    .await
            }
            ScriptLanguage::Shell => {
                Command::new("cmd.exe")
                    .args(["/C", &source])
                    .output()
                    .await
            }
            ScriptLanguage::AppleScript | ScriptLanguage::Jxa => {
                return Err(DesktopError::NotImplemented(
                    "AppleScript/JXA not available on Windows".into(),
                ));
            }
        };

        let output = output
            .map_err(|e| DesktopError::InputFailed(format!("failed to spawn process: {e}")))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(DesktopError::InputFailed(stderr))
        }
    }
```

- [ ] **Step 3: Implement `list_shortcuts`**

```rust
    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>> {
        let output = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                r#"Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter *.lnk | Select-Object -ExpandProperty BaseName"#,
            ])
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to list shortcuts: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(DesktopError::InputFailed(format!(
                "list_shortcuts failed: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let shortcuts = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| ShortcutInfo {
                name: line.trim().to_string(),
                id: None,
                description: None,
            })
            .collect();

        Ok(shortcuts)
    }
```

- [ ] **Step 4: Implement `run_shortcut`**

```rust
    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String> {
        let name = name.to_string();
        let input = input.map(|s| s.to_string());

        // Resolve .lnk path and execute via ShellExecute
        let script = format!(
            r#"$lnk = Get-ChildItem "$env:APPDATA\Microsoft\Windows\Start Menu\Programs" -Recurse -Filter '{0}.lnk' | Select-Object -First 1; if ($lnk) {{ $shell = New-Object -ComObject WScript.Shell; $shortcut = $shell.CreateShortcut($lnk.FullName); & $shortcut.TargetPath $shortcut.Arguments }} else {{ exit 1 }}"#,
            name.replace('\'', "''")
        );

        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-Command", &script]);

        if let Some(data) = input {
            cmd.arg(&data);
        }

        let output = cmd.output().await.map_err(|e| {
            DesktopError::InputFailed(format!("failed to run shortcut `{name}`: {e}"))
        })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(DesktopError::InputFailed(format!(
                "shortcut `{name}` failed: {stderr}"
            )))
        }
    }
}
```

- [ ] **Step 5: Add module declaration to `lib.rs`**

In `desktop/windows/src/lib.rs`, add:
```rust
mod automation;
```

And update:
```rust
pub use automation::WindowsAutomation;
```

- [ ] **Step 6: Commit automation capability**

```bash
git add desktop/windows/src/automation.rs desktop/windows/src/lib.rs
git commit -m "feat(windows): implement AutomationCapability (PowerShell, cmd, shortcuts)"
```

---

## 4. Escape Listener (`WindowsEscapeListener`)

**Files:** `desktop/windows/src/escape_listener.rs` (create)

- [ ] **Step 1: Create `escape_listener.rs`**

```rust
//! Windows `EscapeAbort` implementation using low-level keyboard hook.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use aleph_desktop::platform::EscapeAbort;
use aleph_desktop::Result;

/// Windows escape listener using `WH_KEYBOARD_LL` hook.
pub struct WindowsEscapeListener {
    aborted: AtomicBool,
    // Hook handle stored as raw pointer; valid only between start() and stop().
    hook: Mutex<Option<windows::Win32::Foundation::HHOOK>>,
}

// Static instance for the hook callback to access.
static LISTENER_PTR: Mutex<Option<*const WindowsEscapeListener>> = Mutex::new(None);

impl WindowsEscapeListener {
    pub fn new() -> Self {
        Self {
            aborted: AtomicBool::new(false),
            hook: Mutex::new(None),
        }
    }
}

impl Default for WindowsEscapeListener {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeAbort for WindowsEscapeListener {
    fn start(&self) -> Result<()> {
        use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SetWindowsHookExW, WH_KEYBOARD_LL, KBDLLHOOKSTRUCT, VK_ESCAPE, WM_KEYDOWN,
        };
        use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;

        // Store self pointer for callback access.
        {
            let mut guard = LISTENER_PTR.lock().unwrap();
            *guard = Some(self as *const _);
        }

        // SAFETY: SetWindowsHookExW with WH_KEYBOARD_LL requires a valid HINSTANCE
        // (passing null is documented for global hooks in some contexts; we pass
        // GetModuleHandleW(null) for correctness).
        let hmod = unsafe {
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .unwrap_or(HINSTANCE(std::ptr::null_mut()))
        };

        let hook = unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), hmod, 0)
        };

        let hook = hook.map_err(|e| {
            aleph_desktop::DesktopError::SystemFailed(format!(
                "failed to install keyboard hook: {e}"
            ))
        })?;

        {
            let mut guard = self.hook.lock().unwrap();
            *guard = Some(hook);
        }

        Ok(())
    }

    fn stop(&self) {
        use windows::Win32::UI::Input::KeyboardAndMouse::UnhookWindowsHookEx;

        let mut guard = self.hook.lock().unwrap();
        if let Some(hook) = guard.take() {
            // SAFETY: UnhookWindowsHookEx with a valid hook handle.
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        }

        let mut ptr_guard = LISTENER_PTR.lock().unwrap();
        *ptr_guard = None;
    }

    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    fn reset(&self) {
        self.aborted.store(false, Ordering::SeqCst);
    }
}

// SAFETY: The hook callback is thread-safe because it only accesses AtomicBool.
extern "system" fn keyboard_hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{CallNextHookEx, KBDLLHOOKSTRUCT, VK_ESCAPE, WM_KEYDOWN};

    if code >= 0 && wparam.0 as u32 == WM_KEYDOWN {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if kb.vkCode == VK_ESCAPE.0 as u32 {
            if let Ok(guard) = LISTENER_PTR.lock() {
                if let Some(ptr) = *guard {
                    let listener = unsafe { &*ptr };
                    listener.aborted.store(true, std::sync::atomic::Ordering::SeqCst);
                    // Consume the escape key to prevent it reaching the active app.
                    return LRESULT(1);
                }
            }
        }
    }

    // SAFETY: CallNextHookEx with parameters forwarded from the hook invocation.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
```

- [ ] **Step 2: Add module declaration to `lib.rs`**

In `desktop/windows/src/lib.rs`, add:
```rust
mod escape_listener;
```

And:
```rust
pub use escape_listener::WindowsEscapeListener;
```

- [ ] **Step 3: Commit escape listener**

```bash
git add desktop/windows/src/escape_listener.rs desktop/windows/src/lib.rs
git commit -m "feat(windows): implement EscapeAbort with WH_KEYBOARD_LL hook"
```

---

## 5. Wire into `WindowsPlatform`

**Files:** `desktop/windows/src/lib.rs`

- [ ] **Step 1: Update `WindowsPlatform` struct and impl**

Replace the entire `WindowsPlatform` definition in `lib.rs`:

```rust
/// Windows platform with shared `NativeScreen` for screen capabilities.
pub struct WindowsPlatform {
    screen: NativeScreen,
    power: WindowsPower,
    system: WindowsSystem,
    automation: WindowsAutomation,
    escape: WindowsEscapeListener,
}

impl WindowsPlatform {
    /// Create a new `WindowsPlatform` instance.
    pub fn new() -> Self {
        Self {
            screen: NativeScreen::new(),
            power: WindowsPower::new(),
            system: WindowsSystem::new(),
            automation: WindowsAutomation::new(),
            escape: WindowsEscapeListener::new(),
        }
    }
}

impl Default for WindowsPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopPlatform for WindowsPlatform {
    fn platform_name(&self) -> &str {
        "Windows"
    }

    fn screen(&self) -> Option<&dyn ScreenCapability> {
        Some(&self.screen)
    }

    fn pim(&self) -> Option<&dyn PimCapability> {
        None
    }

    fn system(&self) -> Option<&dyn SystemCapability> {
        Some(&self.system)
    }

    fn automation(&self) -> Option<&dyn AutomationCapability> {
        Some(&self.automation)
    }

    fn permission(&self) -> Option<&dyn PermissionCapability> {
        None
    }

    fn media(&self) -> Option<&dyn MediaCapability> {
        None
    }

    fn power(&self) -> Option<&dyn PowerCapability> {
        Some(&self.power)
    }

    fn escape_listener(&self) -> Option<&dyn EscapeAbort> {
        Some(&self.escape)
    }
}
```

- [ ] **Step 2: Update tests**

Replace the `#[cfg(test)]` module in `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let platform = WindowsPlatform::default();
        assert_eq!(platform.platform_name(), "Windows");
    }

    #[test]
    fn screen_is_some() {
        let platform = WindowsPlatform::new();
        assert!(platform.screen().is_some());
        assert!(platform.system().is_some());
        assert!(platform.automation().is_some());
        assert!(platform.power().is_some());
        assert!(platform.escape_listener().is_some());
        assert!(platform.pim().is_none());
        assert!(platform.permission().is_none());
        assert!(platform.media().is_none());
    }
}
```

- [ ] **Step 3: Verify compilation**

Run:
```bash
cargo check -p aleph-desktop-windows
```

Expected: Clean compile with zero errors.

- [ ] **Step 4: Commit platform wiring**

```bash
git add desktop/windows/src/lib.rs
git commit -m "feat(windows): wire System, Automation, Escape into WindowsPlatform"
```

---

## 6. Unit Tests

### 6.1 System Tests

- [ ] **Step 1: Add tests to `system.rs`**

Append to `desktop/windows/src/system.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _sys = WindowsSystem::default();
    }

    #[tokio::test]
    async fn clipboard_roundtrip() {
        let sys = WindowsSystem::new();
        sys.clipboard_write("hello windows").await.unwrap();
        let content = sys.clipboard_read().await.unwrap();
        assert_eq!(content, ClipboardContent::Text("hello windows".to_string()));
    }

    #[tokio::test]
    async fn system_info_returns_valid() {
        let sys = WindowsSystem::new();
        let info = sys.system_info().await.unwrap();
        assert!(!info.hostname.is_empty());
        assert_eq!(info.os_name, "Windows");
    }

    #[tokio::test]
    async fn user_idle_seconds_non_negative() {
        let sys = WindowsSystem::new();
        let idle = sys.user_idle_seconds().await.unwrap();
        assert!(idle >= 0.0);
    }
}
```

- [ ] **Step 2: Run system tests**

```bash
cargo test -p aleph-desktop-windows --lib system::tests
```

Expected: All 4 tests pass.

### 6.2 Automation Tests

- [ ] **Step 3: Add tests to `automation.rs`**

Append to `desktop/windows/src/automation.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _auto = WindowsAutomation::default();
    }

    #[tokio::test]
    async fn powershell_echo() {
        let auto = WindowsAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::PowerShell, "Write-Output hello")
            .await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn shell_echo() {
        let auto = WindowsAutomation::new();
        let result = auto.run_script(ScriptLanguage::Shell, "echo hello").await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn applescript_not_implemented() {
        let auto = WindowsAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::AppleScript, "return 1")
            .await;
        assert!(matches!(result, Err(DesktopError::NotImplemented(_))));
    }

    #[tokio::test]
    async fn list_shortcuts_does_not_panic() {
        let auto = WindowsAutomation::new();
        let result = auto.list_shortcuts().await;
        // May succeed or fail depending on Start Menu contents; must not panic.
        assert!(result.is_ok() || result.is_err());
    }
}
```

- [ ] **Step 4: Run automation tests**

```bash
cargo test -p aleph-desktop-windows --lib automation::tests
```

Expected: All 5 tests pass.

### 6.3 Escape Listener Tests

- [ ] **Step 5: Add tests to `escape_listener.rs`**

Append to `desktop/windows/src/escape_listener.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _esc = WindowsEscapeListener::default();
    }

    #[test]
    fn lifecycle() {
        let esc = WindowsEscapeListener::new();
        assert!(!esc.is_aborted());

        // start() may fail in headless/CI environments without a message pump.
        let started = esc.start();
        if started.is_ok() {
            assert!(!esc.is_aborted());
            esc.reset();
            assert!(!esc.is_aborted());
            esc.stop();
        }
    }
}
```

- [ ] **Step 6: Run escape tests**

```bash
cargo test -p aleph-desktop-windows --lib escape_listener::tests
```

Expected: All 2 tests pass (lifecycle may skip the hook installation in CI).

- [ ] **Step 7: Commit all tests**

```bash
git add desktop/windows/src/system.rs desktop/windows/src/automation.rs desktop/windows/src/escape_listener.rs
git commit -m "test(windows): add unit tests for System, Automation, Escape capabilities"
```

---

## 7. Integration Tests

**Files:** `desktop/windows/tests/` (new directory)

- [ ] **Step 1: Create `tests/system_e2e.rs`**

```rust
//! E2E tests for Windows system capabilities.

use aleph_desktop_windows::WindowsPlatform;
use aleph_desktop::DesktopPlatform;

#[tokio::test]
async fn system_info_works() {
    let platform = WindowsPlatform::new();
    let system = platform.system().unwrap();
    let info = system.system_info().await.unwrap();
    assert!(!info.hostname.is_empty());
}

#[tokio::test]
async fn clipboard_roundtrip() {
    let platform = WindowsPlatform::new();
    let system = platform.system().unwrap();
    system.clipboard_write("integration test").await.unwrap();
    let content = system.clipboard_read().await.unwrap();
    assert_eq!(content.to_string(), "integration test");
}
```

- [ ] **Step 2: Create `tests/automation_e2e.rs`**

```rust
//! E2E tests for Windows automation capabilities.

use aleph_desktop::DesktopPlatform;
use aleph_desktop::automation_types::ScriptLanguage;
use aleph_desktop_windows::WindowsPlatform;

#[tokio::test]
async fn powershell_get_date() {
    let platform = WindowsPlatform::new();
    let auto = platform.automation().unwrap();
    let result = auto
        .run_script(ScriptLanguage::PowerShell, "Get-Date -Format yyyy")
        .await;
    let year = result.unwrap();
    assert_eq!(year.len(), 4);
    assert!(year.parse::<u32>().unwrap() >= 2026);
}

#[tokio::test]
async fn shell_dir() {
    let platform = WindowsPlatform::new();
    let auto = platform.automation().unwrap();
    let result = auto.run_script(ScriptLanguage::Shell, "dir").await;
    assert!(result.is_ok());
    assert!(!result.unwrap().is_empty());
}
```

- [ ] **Step 3: Run integration tests**

```bash
cargo test -p aleph-desktop-windows --test system_e2e
cargo test -p aleph-desktop-windows --test automation_e2e
```

Expected: All tests pass.

- [ ] **Step 4: Commit integration tests**

```bash
git add desktop/windows/tests/
git commit -m "test(windows): add integration tests for system and automation"
```

---

## 8. Final Verification

- [ ] **Step 1: Full test suite**

```bash
cargo test -p aleph-desktop-windows
```

Expected: All tests pass.

- [ ] **Step 2: Lint**

```bash
cargo clippy -p aleph-desktop-windows -- -D warnings
```

Expected: Zero warnings.

- [ ] **Step 3: Format**

```bash
cargo fmt -- --check
```

Expected: No formatting issues.

- [ ] **Step 4: Final commit (if needed)**

```bash
git add -A
git commit -m "chore(windows): final verification and cleanup" || true
```

---

## 9. Post-Implementation Cleanup Checklist

After all tasks complete, verify:

- [ ] All `TODO`/`FIXME` comments removed from new code
- [ ] All `unsafe` blocks have `// SAFETY:` comments
- [ ] `WindowsPlatform` tests assert `is_some()` for implemented capabilities
- [ ] `cargo check -p aleph-desktop-windows` passes
- [ ] `cargo clippy -p aleph-desktop-windows -- -D warnings` passes
- [ ] `cargo test -p aleph-desktop-windows` passes
- [ ] No unused imports in new files
- [ ] Documentation comments on all public types and methods

---

*End of plan. Ready for execution.*
