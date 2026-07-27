//! Application launch (platform-specific).

// Only the macOS and Windows arms log inline; the Linux arms delegate to
// `crate::linux::app` / `crate::action::window_linux`, which own their own logging.
#[cfg(not(target_os = "linux"))]
use tracing::info;

#[cfg(not(target_os = "linux"))]
use crate::error::DesktopError;
use crate::error::Result;

/// Launch an application by name or bundle ID.
///
/// - **macOS**: `open -b <bundle_id>` (or `open -a <app_name>` if not a bundle ID)
/// - **Linux**: Resolves a `.desktop` entry (by id, `Name=`, or `Exec=`), else a
///   `PATH` executable, else `xdg-open` for URLs and real paths — see
///   [`crate::linux::app::launch`].
/// - **Windows**: `ShellExecuteW` with verb `"open"` (no cmd.exe)
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the application cannot be launched.
/// - [`DesktopError::NotImplemented`] on unsupported platforms.
pub fn launch_app(app_name: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::{NSString, NSURL};

        let ws = NSWorkspace::sharedWorkspace();
        let ns_name = NSString::from_str(app_name);

        #[allow(deprecated)]
        let url = if app_name.contains('.') {
            ws.URLForApplicationWithBundleIdentifier(&ns_name)
        } else {
            ws.fullPathForApplication(&ns_name)
                .map(|p| NSURL::fileURLWithPath(&p))
        };

        let url = url.ok_or_else(|| {
            DesktopError::InputFailed(format!("Application '{app_name}' not found"))
        })?;

        if !ws.openURL(&url) {
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{app_name}'"
            )));
        }

        info!(app_name, "App launched (macOS)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        // Desktop-entry resolution → PATH executable → xdg-open, in that order.
        // This arm used to hand a bare application name to `xdg-open`, which
        // opens *files and URLs*; see `crate::linux::app`.
        crate::linux::app::launch(app_name)
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let verb: Vec<u16> = std::ffi::OsStr::new("open")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let app: Vec<u16> = std::ffi::OsStr::new(app_name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // ShellExecuteW resolves the app via file association / App Paths / PATH
        // without invoking cmd.exe, closing the metacharacter-injection vector.
        // SAFETY: `verb`/`app` are valid NUL-terminated wide buffers living past
        // the call; other pointers are null as documented.
        let hinst = unsafe {
            ShellExecuteW(
                HWND(std::ptr::null_mut()),
                PCWSTR(verb.as_ptr()),
                PCWSTR(app.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if hinst.0 as isize <= 32 {
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{app_name}' (ShellExecuteW code {})",
                hinst.0 as isize
            )));
        }
        info!(app_name, "App launched (Windows)");
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app_name;
        Err(DesktopError::NotImplemented(
            "launch_app not supported on this platform".into(),
        ))
    }
}

/// Quit/close an application by name or bundle ID.
///
/// - **macOS**: Uses `NSRunningApplication` to find and terminate the app by bundle ID.
/// - **Linux**: Asks the window manager to close the app's windows, then falls back
///   to `SIGTERM` on processes whose executable name matches exactly — never a
///   command-line match. See [`crate::linux::app::quit`].
/// - **Windows**: Matches running processes by executable name (not window
///   title) and posts `WM_CLOSE` to each of their visible windows.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the application cannot be found or terminated.
/// - [`DesktopError::NotImplemented`] on unsupported platforms.
pub fn quit_app(app_name: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSRunningApplication;
        use objc2_foundation::NSString;

        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str(app_name),
        );
        if apps.is_empty() {
            return Err(DesktopError::InputFailed(format!(
                "No running application found with identifier '{app_name}'"
            )));
        }
        for app in &apps {
            app.terminate();
        }
        info!(app_name, "App quit requested (macOS)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        // Ask the windows to close first, then SIGTERM the exactly-matching
        // processes — never a command-line match; see `crate::linux::app`.
        crate::linux::app::quit(app_name)
    }

    #[cfg(target_os = "windows")]
    {
        windows_quit_app(app_name)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = app_name;
        Err(DesktopError::NotImplemented(
            "quit_app not implemented on this platform".into(),
        ))
    }
}

/// Returns true when `exe_path` (a full executable path) names the same
/// application as `target` — compared on the file-name stem, case-insensitively,
/// with an optional `.exe` suffix on either side.
///
/// Split out as a pure function so the Windows `quit_app` matching rule can be
/// unit-tested on any platform.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn exe_name_matches(exe_path: &str, target: &str) -> bool {
    fn stem(name: &str) -> String {
        let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
        let lower = base.to_ascii_lowercase();
        lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
    }
    let target_stem = stem(target);
    !target_stem.is_empty() && stem(exe_path) == target_stem
}

#[cfg(target_os = "windows")]
fn windows_quit_app(app_name: &str) -> Result<()> {
    use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    };

    struct EnumState {
        target: String,
        closed: u32,
    }

    extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `EnumWindows` guarantees `hwnd` is valid for this callback,
        // and `lparam` carries the `&mut EnumState` pointer passed below.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return BOOL(1);
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, Some(std::ptr::addr_of_mut!(pid)));
            if pid == 0 {
                return BOOL(1);
            }
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid) else {
                return BOOL(1);
            };
            let mut buf = [0u16; 512];
            let mut size = 512u32;
            let exe = if QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                std::ptr::addr_of_mut!(size),
            )
            .is_ok()
            {
                String::from_utf16_lossy(&buf[..size as usize])
            } else {
                String::new()
            };
            let _ = CloseHandle(handle);

            let state = &mut *(lparam.0 as *mut EnumState);
            if exe_name_matches(&exe, &state.target) {
                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                state.closed += 1;
            }
        }
        BOOL(1) // continue — close every window of every matching process
    }

    let mut state = EnumState {
        target: app_name.to_string(),
        closed: 0,
    };
    // SAFETY: `enum_proc` matches the `WNDENUMPROC` signature; `state` lives
    // until `EnumWindows` returns.
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(std::ptr::addr_of_mut!(state) as isize),
        );
    }

    if state.closed == 0 {
        return Err(DesktopError::InputFailed(format!(
            "No running application matching '{app_name}' found"
        )));
    }
    info!(
        app_name,
        closed = state.closed,
        "App quit requested (Windows)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::exe_name_matches;

    #[test]
    fn exe_name_matches_basename_case_insensitive() {
        assert!(exe_name_matches(
            r"C:\Windows\System32\notepad.exe",
            "notepad"
        ));
        assert!(exe_name_matches(
            r"C:\Windows\System32\notepad.exe",
            "Notepad"
        ));
        assert!(exe_name_matches(
            r"C:\Windows\System32\notepad.exe",
            "NOTEPAD.EXE"
        ));
        assert!(exe_name_matches(
            r"C:\Program Files\Chrome\chrome.exe",
            "chrome.exe"
        ));
    }

    #[test]
    fn exe_name_matches_forward_slash_paths() {
        assert!(exe_name_matches("/usr/bin/code", "code"));
        assert!(exe_name_matches("code.exe", "code"));
    }

    #[test]
    fn exe_name_matches_rejects_partial_and_unrelated() {
        // The old buggy behaviour was a title substring match — "Word" must NOT
        // match "Password Manager" / "winword"-adjacent names.
        assert!(!exe_name_matches(r"C:\apps\PasswordManager.exe", "word"));
        assert!(!exe_name_matches(r"C:\Office\winword.exe", "word"));
        assert!(!exe_name_matches(r"C:\Windows\notepad.exe", "note"));
    }

    #[test]
    fn exe_name_matches_rejects_empty_target() {
        assert!(!exe_name_matches(r"C:\Windows\notepad.exe", ""));
        assert!(!exe_name_matches("", ""));
    }
}
