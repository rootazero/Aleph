//! Windows `SystemCapability` implementation using Win32 APIs.

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;

/// Where Windows records its own build identity.
#[cfg_attr(not(windows), allow(dead_code))]
const CURRENT_VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

/// Assemble a human-meaningful OS version string, e.g.
/// `"Windows 11 Pro 24H2 (build 26100.2894)"`.
///
/// The previous implementation returned `std::env::var("OS")`, which is the
/// literal string `"Windows_NT"` on every Windows machine ever shipped — the
/// same seven characters for Windows 7 and Windows 11, carrying no version at
/// all. `system_info` is what a model reads before deciding whether a feature
/// exists on this machine, so a constant there is worse than an empty string.
///
/// Read from the registry rather than `GetVersionExW`, which lies to
/// unmanifested processes by design (it reports 6.2 unless the executable
/// declares Windows 10 compatibility in its manifest — and a Rust binary ships
/// no manifest). Each field degrades independently: an unreadable value is
/// dropped from the string, never guessed at.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_version() -> String {
    #[cfg(windows)]
    {
        use aleph_desktop::win_registry::{read_string, read_u32, Hive};

        let get = |name: &str| read_string(Hive::LocalMachine, CURRENT_VERSION_KEY, name);

        // `CurrentBuild` is a REG_SZ, not a DWORD — the only DWORDs in this key
        // are `UBR` and the `Current{Major,Minor}VersionNumber` pair.
        let build_number: Option<u32> = get("CurrentBuild").and_then(|b| b.parse().ok());

        // Microsoft froze `ProductName` at "Windows 10 …" on Windows 11 machines;
        // the build number is the field that actually tells them apart (11 ships
        // 22000+). Reporting "Windows 10" on a Windows 11 box would send a model
        // looking for the wrong UI.
        let product = get("ProductName").map(|p| match build_number {
            Some(build) if build >= 22_000 => p.replace("Windows 10", "Windows 11"),
            _ => p,
        });
        // `DisplayVersion` ("24H2") replaced `ReleaseId` ("2009") in 20H2.
        let release = get("DisplayVersion").or_else(|| get("ReleaseId"));
        // `UBR` is the update-revision suffix: 26100 + 2894 → "build 26100.2894",
        // which is the form Winver and every support page use.
        let build =
            build_number.map(
                |b| match read_u32(Hive::LocalMachine, CURRENT_VERSION_KEY, "UBR") {
                    Some(ubr) => format!("build {b}.{ubr}"),
                    None => format!("build {b}"),
                },
            );

        let mut parts: Vec<String> = Vec::new();
        parts.extend(product);
        parts.extend(release);
        let head = parts.join(" ");
        match (head.is_empty(), build) {
            (true, Some(b)) => b,
            (true, None) => "Windows".to_string(),
            (false, Some(b)) => format!("{head} ({b})"),
            (false, None) => head,
        }
    }
    #[cfg(not(windows))]
    {
        "Windows".to_string()
    }
}

pub struct WindowsSystem {
    _private: (),
}

impl WindowsSystem {
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for WindowsSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemCapability for WindowsSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        #[cfg(windows)]
        {
            let app_name = app_name.to_string();
            tokio::task::spawn_blocking(move || {
                use windows::core::PCWSTR;
                use windows::Win32::System::Com::{
                    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
                };
                use windows::Win32::UI::Shell::ShellExecuteW;
                use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

                // `ShellExecuteW` delegates to Shell extension handlers, and the
                // documentation is explicit that COM must be initialized first —
                // single-threaded apartment, because some handlers require it.
                // A bare `spawn_blocking` thread has no apartment, so launching
                // anything whose association goes through such a handler failed
                // with an opaque code. `CoUninitialize` balances it on the way
                // out; the thread is returned to the pool clean.
                //
                // SAFETY: paired init/uninit on this thread only, around the one
                // call that needs the apartment.
                let com_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
                struct ComExit(bool);
                impl Drop for ComExit {
                    fn drop(&mut self) {
                        if self.0 {
                            // SAFETY: balances the `CoInitializeEx` above.
                            unsafe { CoUninitialize() };
                        }
                    }
                }
                let _com = ComExit(com_ok);

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
                    Err(DesktopError::PlatformError(format!(
                        "failed to launch '{app_name}': ShellExecute returned {code}"
                    )))
                }
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            let _ = app_name;
            Err(DesktopError::NotImplemented(
                "launch_app requires Windows".into(),
            ))
        }
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        // Delegate to the shared cross-platform implementation, which matches by
        // process executable name. The previous local implementation matched by
        // window-title substring, so `quit_app("Word")` could also close
        // unrelated apps such as "Password Manager".
        let app_name = app_name.to_string();
        tokio::task::spawn_blocking(move || aleph_desktop::action::quit_app(&app_name))
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
    }

    /// One entry per **process** that owns a visible top-level window.
    ///
    /// This used to enumerate *windows* and report each window's title as an app
    /// name, which broke three separate things that all read this list:
    ///
    /// * `app:` targeting matched titles, so `app: "explorer"` missed (File
    ///   Explorer titles itself after the open folder) while two browser windows
    ///   made `app: "chrome"` ambiguous and the tool refused rather than
    ///   resolving the one process behind both.
    /// * `is_active` was hard-coded `false`, so the frontmost-app lookup that
    ///   arms the credential-manager hard block (`check_blocked_app`) never found
    ///   a target and the block was inert on Windows.
    /// * `observe`'s post-action state reported no frontmost app at all.
    ///
    /// `name` is now the executable's `FileDescription` ("Google Chrome") with
    /// the file stem as fallback, and `bundle_id` is the full executable path —
    /// Windows' closest analogue to a bundle id, and what makes `app:
    /// "chrome.exe"` resolvable. See [`aleph_desktop::win_window`].
    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                Ok(aleph_desktop::win_window::running_apps()
                    .into_iter()
                    .map(|app| AppInfo {
                        name: app.name,
                        bundle_id: app.executable,
                        pid: Some(u64::from(app.pid)),
                        is_active: app.is_active,
                    })
                    .collect())
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "list_running_apps requires Windows".into(),
            ))
        }
    }

    /// Post a toast via the WinRT notification manager, driven from PowerShell.
    ///
    /// Title and body travel as **environment variables**, so the script text is
    /// a constant. Interpolating them doubled `'` and nothing else, which left
    /// every other PowerShell metacharacter — and, more commonly, the newlines
    /// an R5 summary is full of — free to break the string literal and fail the
    /// whole notification. (The same class of bug was fixed on the macOS
    /// AppleScript path in 2026-07; the Windows path was not.)
    ///
    /// # Why the script now fails loudly
    ///
    /// It had no `$ErrorActionPreference` and no `try`/`catch`, so every step
    /// could fail as a *non-terminating* error and PowerShell still exited `0`.
    /// A machine where the WinRT projection will not load (Server Core, an N
    /// edition, a locked-down policy) therefore posted no notification and
    /// reported delivery — the worst possible answer for the one capability
    /// whose entire purpose is to tell the user something happened. R5 says
    /// Aleph comes to the user; silently not arriving is not an option it gets.
    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        #[cfg(windows)]
        {
            /// Constant script: reads every caller value from the environment,
            /// so nothing the caller controls is parsed as PowerShell.
            ///
            /// The app id decides which name and icon the toast carries. The
            /// desktop shell registers `ai.aleph.desktop` (its Tauri bundle
            /// identifier) and the Panel-only build registers `ai.aleph.panel`;
            /// when one of those is installed the toast looks like it came from
            /// Aleph. The bare `Aleph` fallback still posts — Windows 10 1607
            /// and later accept an unregistered app id and create the settings
            /// entry on the fly — it just shows without the app's identity.
            const TOAST_SCRIPT: &str = r"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType=WindowsRuntime]
$appId = 'Aleph'
foreach ($candidate in @('ai.aleph.desktop', 'ai.aleph.panel')) {
    if (Test-Path -LiteralPath ('HKCU:\Software\Classes\AppUserModelId\' + $candidate)) { $appId = $candidate; break }
}
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$nodes = $template.GetElementsByTagName('text')
$null = $nodes.Item(0).AppendChild($template.CreateTextNode($env:ALEPH_TOAST_TITLE))
$null = $nodes.Item(1).AppendChild($template.CreateTextNode($env:ALEPH_TOAST_BODY))
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($appId).Show([Windows.UI.Notifications.ToastNotification]::new($template))
";

            let title = title.to_string();
            let body = body.to_string();
            // `hidden_std_command`: a console child spawned by the windowless
            // daemon pops a black window on the user's screen — for a
            // *notification*, which is the one thing that must not startle.
            let output = tokio::task::spawn_blocking(move || {
                aleph_desktop::script_exec::hidden_std_command("powershell.exe")
                    .args(["-NoProfile", "-NonInteractive", "-Command", TOAST_SCRIPT])
                    .env("ALEPH_TOAST_TITLE", title)
                    .env("ALEPH_TOAST_BODY", body)
                    .output()
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?;

            match output {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(DesktopError::PlatformError(format!(
                        "toast notification failed: {}",
                        stderr.trim()
                    )))
                }
                Err(e) => Err(DesktopError::PlatformError(format!(
                    "failed to spawn powershell for notification: {e}"
                ))),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (title, body);
            Err(DesktopError::NotImplemented(
                "send_notification requires Windows".into(),
            ))
        }
    }

    /// Read the clipboard, distinguishing "nothing there" from "the read broke".
    ///
    /// Both used to be an `InputFailed`: `get_clipboard::<String>` errors when
    /// no `CF_UNICODETEXT` is on the board, which is the ordinary state of an
    /// empty clipboard, of one holding a picture, and of one holding a copied
    /// file. So the tool answered "clipboard read failed" for three situations
    /// where nothing had failed at all, and the model had no way to tell that
    /// from a genuine error.
    ///
    /// `has_image` was also hard-coded `false` while the wire field exists and
    /// macOS fills it, so a screenshot on the clipboard was invisible to the
    /// model. The bitmap itself is still not transported — that needs a DIB→PNG
    /// conversion this capability has no consumer for — but *knowing it is
    /// there* is what lets the model paste it instead of asking again.
    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use clipboard_win::{formats, get_clipboard, is_format_avail};

                // `CF_DIB` is what a screenshot or a copied image lands as;
                // `CF_BITMAP` covers the older producers.
                let has_image = is_format_avail(formats::CF_DIB)
                    || is_format_avail(formats::CF_DIBV5)
                    || is_format_avail(formats::CF_BITMAP);

                if !is_format_avail(formats::CF_UNICODETEXT) {
                    return Ok(ClipboardContent {
                        text: None,
                        has_image,
                        image_base64: None,
                    });
                }

                match get_clipboard::<String, _>(formats::Unicode) {
                    Ok(text) => Ok(ClipboardContent {
                        text: Some(text),
                        has_image,
                        image_base64: None,
                    }),
                    // The format is on the board but the read failed — another
                    // process holds the clipboard open, or a delayed-render
                    // owner died. That *is* a failure.
                    Err(e) => Err(DesktopError::InputFailed(format!(
                        "clipboard read failed: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| DesktopError::InputFailed(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "clipboard_read requires Windows".into(),
            ))
        }
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        #[cfg(windows)]
        {
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
        #[cfg(not(windows))]
        {
            let _ = text;
            Err(DesktopError::NotImplemented(
                "clipboard_write requires Windows".into(),
            ))
        }
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use windows::core::PWSTR;
                use windows::Win32::System::SystemInformation::{
                    ComputerNamePhysicalDnsHostname, GetComputerNameExW,
                };

                let mut hostname_buf = [0u16; 256];
                let mut size = 256u32;

                // SAFETY: GetComputerNameExW writes up to `size` UTF-16 units
                // into the buffer and updates `size` with the count written.
                let hostname = unsafe {
                    if GetComputerNameExW(
                        ComputerNamePhysicalDnsHostname,
                        PWSTR(hostname_buf.as_mut_ptr()),
                        std::ptr::addr_of_mut!(size),
                    )
                    .is_ok()
                    {
                        String::from_utf16_lossy(&hostname_buf[..size as usize])
                    } else {
                        "unknown".to_string()
                    }
                };

                let username = std::env::var("USERNAME").unwrap_or_else(|_| "unknown".to_string());

                Ok(SystemInfo {
                    os_name: "Windows".to_string(),
                    os_version: windows_version(),
                    hostname,
                    arch: std::env::consts::ARCH.to_string(),
                    username,
                })
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "system_info requires Windows".into(),
            ))
        }
    }

    async fn user_idle_seconds(&self) -> Result<f64> {
        #[cfg(windows)]
        {
            tokio::task::spawn_blocking(|| {
                use windows::Win32::System::SystemInformation::GetTickCount64;
                use windows::Win32::UI::Input::KeyboardAndMouse::GetLastInputInfo;

                #[repr(C)]
                #[allow(non_snake_case, clippy::upper_case_acronyms, reason = "Win32 ABI struct — name & field casing must match the Win32 header; renaming would diverge from the C declaration and break the bind contract")]
                struct LASTINPUTINFO {
                    cbSize: u32,
                    dwTime: u32,
                }

                let mut info = LASTINPUTINFO {
                    cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
                    dwTime: 0,
                };

                // SAFETY: GetLastInputInfo writes into the struct on success;
                // GetTickCount64 returns u64 millis since boot (no 49.7-day wrap).
                let (ok, now_ms, last_ms) = unsafe {
                    let ok = GetLastInputInfo(std::ptr::addr_of_mut!(info) as *mut _).as_bool();
                    (ok, GetTickCount64(), info.dwTime)
                };

                if !ok {
                    return Err(DesktopError::PlatformError(
                        "GetLastInputInfo failed".into(),
                    ));
                }

                // dwTime is the low 32 bits of the boot tick counter at the moment
                // of the last input event. GetTickCount64 advances past 2^32 ms
                // after ~49.7 days; reconstruct the full 64-bit timestamp by
                // combining the high 32 bits of `now_ms` with `last_ms`, then
                // rolling back one wrap if the result is in the future.
                let high = now_ms & 0xFFFF_FFFF_0000_0000;
                let mut last_full = high | (last_ms as u64);
                if last_full > now_ms {
                    last_full = last_full.wrapping_sub(1u64 << 32);
                }
                let idle_millis = now_ms.saturating_sub(last_full);

                Ok((idle_millis as f64) / 1000.0)
            })
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
        }
        #[cfg(not(windows))]
        {
            Err(DesktopError::NotImplemented(
                "user_idle_seconds requires Windows".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _sys = WindowsSystem::default();
    }
}
