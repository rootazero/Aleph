//! Window listing and focus management (platform-specific).

use tracing::info;

use crate::error::{DesktopError, Result};
use crate::WindowInfo;

/// List all visible on-screen windows.
///
/// - **macOS**: CoreGraphics `CGWindowListCopyWindowInfo`.
/// - **Linux**: `wmctrl -l -p`.
/// - **Windows**: `EnumWindows` over visible top-level windows; `WindowInfo.id`
///   carries the `HWND` so [`focus_window`] can round-trip it.
///
/// # Errors
///
/// - [`DesktopError::WindowFailed`] if the platform command fails.
/// - [`DesktopError::NotImplemented`] on platforms without an implementation.
pub fn window_list() -> Result<Vec<WindowInfo>> {
    #[cfg(target_os = "macos")]
    {
        macos_window_list()
    }

    #[cfg(target_os = "linux")]
    {
        linux_window_list()
    }

    #[cfg(target_os = "windows")]
    {
        windows_window_list()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(DesktopError::NotImplemented(
            "window_list not implemented on this platform".into(),
        ))
    }
}

/// Bring the specified window to the foreground.
///
/// - **macOS**: Activates the owning app via `NSRunningApplication`.
/// - **Linux**: Uses `wmctrl -i -a <hex_id>` to activate the window.
/// - **Windows**: Resolves `window_id` as an `HWND`, un-minimizes it if needed,
///   then calls `SetForegroundWindow`.
///
/// # Errors
///
/// - [`DesktopError::WindowFailed`] if the window is not found or the platform
///   command fails.
/// - [`DesktopError::NotImplemented`] on platforms without an implementation.
pub fn focus_window(window_id: u64) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_focus_window(window_id)
    }

    #[cfg(target_os = "linux")]
    {
        linux_focus_window(window_id)
    }

    #[cfg(target_os = "windows")]
    {
        windows_focus_window(window_id)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = window_id;
        Err(DesktopError::NotImplemented(
            "focus_window not implemented on this platform".into(),
        ))
    }
}

/// Move a window's top-left corner to `(x, y)` in global screen coordinates.
///
/// - **macOS**: `System Events` Accessibility API via `osascript` (requires
///   the Accessibility TCC permission, same as input automation).
/// - **Linux**: `wmctrl -i -r <id> -e 0,x,y,-1,-1` (preserves size).
/// - **Windows**: `SetWindowPos` with `SWP_NOSIZE` (resolves `window_id` as an
///   `HWND`; preserves the current size).
///
/// # Errors
///
/// - [`DesktopError::WindowFailed`] if the window is not found or the platform
///   command fails.
/// - [`DesktopError::NotImplemented`] on platforms without an implementation.
pub fn move_window(window_id: u64, x: i32, y: i32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_set_window_bounds(window_id, Some((x, y)), None)
    }

    #[cfg(target_os = "linux")]
    {
        linux_move_window(window_id, x, y)
    }

    #[cfg(target_os = "windows")]
    {
        windows_move_window(window_id, x, y)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (window_id, x, y);
        Err(DesktopError::NotImplemented(
            "move_window not implemented on this platform".into(),
        ))
    }
}

/// Resize a window to `width` × `height` pixels.
///
/// - **macOS**: `System Events` Accessibility API via `osascript` (requires
///   the Accessibility TCC permission, same as input automation).
/// - **Linux**: `wmctrl -i -r <id> -e 0,-1,-1,w,h` (preserves position).
/// - **Windows**: `SetWindowPos` with `SWP_NOMOVE` (resolves `window_id` as an
///   `HWND`; preserves the current top-left position).
///
/// # Errors
///
/// - [`DesktopError::WindowFailed`] if the window is not found or the platform
///   command fails.
/// - [`DesktopError::NotImplemented`] on platforms without an implementation.
pub fn resize_window(window_id: u64, width: u32, height: u32) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos_set_window_bounds(window_id, None, Some((width as i32, height as i32)))
    }

    #[cfg(target_os = "linux")]
    {
        linux_resize_window(window_id, width, height)
    }

    #[cfg(target_os = "windows")]
    {
        windows_resize_window(window_id, width, height)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (window_id, width, height);
        Err(DesktopError::NotImplemented(
            "resize_window not implemented on this platform".into(),
        ))
    }
}

// ── Windows window management helpers ─────────────────────────────

#[cfg(target_os = "windows")]
fn windows_window_list() -> Result<Vec<WindowInfo>> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    struct EnumState {
        windows: Vec<WindowInfo>,
    }

    extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `EnumWindows` guarantees `hwnd` is valid for this callback,
        // and `lparam` carries the `&mut EnumState` pointer passed below, which
        // outlives the synchronous enumeration.
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let mut buf = [0u16; 512];
                let len = GetWindowTextW(hwnd, &mut buf);
                if len > 0 {
                    let title = String::from_utf16_lossy(&buf[..len as usize]);
                    let mut pid: u32 = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    let state = &mut *(lparam.0 as *mut EnumState);
                    state.windows.push(WindowInfo {
                        id: hwnd.0 as usize as u64,
                        title,
                        owner: String::new(),
                        pid: pid as u64,
                    });
                }
            }
        }
        BOOL(1) // continue enumeration
    }

    let mut state = EnumState {
        windows: Vec::new(),
    };
    // SAFETY: `enum_proc` matches the `WNDENUMPROC` signature; `state` lives
    // until `EnumWindows` returns.
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut state as *mut EnumState as isize),
        );
    }

    info!(
        count = state.windows.len(),
        "Window list retrieved (Windows)"
    );
    Ok(state.windows)
}

#[cfg(target_os = "windows")]
fn windows_focus_window(window_id: u64) -> Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let hwnd = HWND(window_id as usize as *mut core::ffi::c_void);

    // SAFETY: the handle is validated by `IsWindow` before any state-changing
    // call; all calls are documented Win32 APIs.
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return Err(DesktopError::WindowFailed(format!(
                "No window found with id {window_id}"
            )));
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // `SetForegroundWindow` may return false under Windows' foreground-lock
        // rules even when the window is raised; that is not a hard failure.
        let _ = SetForegroundWindow(hwnd);
    }

    info!(window_id, "Window focused (Windows)");
    Ok(())
}

/// Reposition and/or resize a window via Win32 `SetWindowPos`.
///
/// `SWP_NOZORDER | SWP_NOACTIVATE` are always added so the call never changes
/// the Z-order or steals focus; callers pass `SWP_NOSIZE` (move only) or
/// `SWP_NOMOVE` (resize only) to pin the dimension they want preserved.
#[cfg(target_os = "windows")]
fn windows_set_window_pos(
    window_id: u64,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS,
) -> Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    };

    let hwnd = HWND(window_id as usize as *mut core::ffi::c_void);

    // SAFETY: the handle is validated by `IsWindow` before the state-changing
    // call; `SetWindowPos` is a documented Win32 API and the insert-after
    // handle is ignored because `SWP_NOZORDER` is always set.
    unsafe {
        if !IsWindow(hwnd).as_bool() {
            return Err(DesktopError::WindowFailed(format!(
                "No window found with id {window_id}"
            )));
        }
        SetWindowPos(
            hwnd,
            HWND::default(),
            x,
            y,
            cx,
            cy,
            flags | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(|e| DesktopError::WindowFailed(format!("SetWindowPos failed: {e}")))?;
    }

    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_move_window(window_id: u64, x: i32, y: i32) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE;

    windows_set_window_pos(window_id, x, y, 0, 0, SWP_NOSIZE)?;
    info!(window_id, x, y, "Window moved (Windows)");
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_resize_window(window_id: u64, width: u32, height: u32) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE;

    // Win32 `SetWindowPos` takes signed dimensions; reject values past `i32::MAX`
    // rather than letting an `as` cast silently wrap to a negative size.
    let cx = i32::try_from(width)
        .map_err(|_| DesktopError::WindowFailed(format!("width {width} exceeds i32 range")))?;
    let cy = i32::try_from(height)
        .map_err(|_| DesktopError::WindowFailed(format!("height {height} exceeds i32 range")))?;

    windows_set_window_pos(window_id, 0, 0, cx, cy, SWP_NOMOVE)?;
    info!(window_id, width, height, "Window resized (Windows)");
    Ok(())
}

// ── macOS window management helpers ──────────────────────────────

#[cfg(target_os = "macos")]
fn macos_window_list() -> Result<Vec<WindowInfo>> {
    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowLayer, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionOnScreenOnly, kCGWindowName, kCGWindowNumber, kCGWindowOwnerName,
        kCGWindowOwnerPID,
    };

    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let list = match copy_window_info(options, kCGNullWindowID) {
        Some(l) => l,
        None => return Ok(Vec::new()),
    };

    let mut windows = Vec::new();
    let values = list.get_all_values();

    for ptr in &values {
        let entry: core_foundation::dictionary::CFDictionary<
            CFString,
            core_foundation::base::CFType,
        > = unsafe {
            TCFType::wrap_under_get_rule(*ptr as core_foundation::dictionary::CFDictionaryRef)
        };

        let get_str = |key: core_foundation::string::CFStringRef| -> String {
            unsafe {
                let key_cf = CFString::wrap_under_get_rule(key);
                entry
                    .find(&key_cf)
                    .and_then(|v| v.downcast::<CFString>())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            }
        };

        let get_i64 = |key: core_foundation::string::CFStringRef| -> i64 {
            unsafe {
                let key_cf = CFString::wrap_under_get_rule(key);
                entry
                    .find(&key_cf)
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|n| n.to_i64())
                    .unwrap_or(0)
            }
        };

        let title = unsafe { get_str(kCGWindowName) };
        let layer = unsafe { get_i64(kCGWindowLayer) };

        // Skip windows with empty title and non-zero layer (menu extras, etc.)
        if title.is_empty() && layer != 0 {
            continue;
        }

        let id = unsafe { get_i64(kCGWindowNumber) } as u64;
        let owner = unsafe { get_str(kCGWindowOwnerName) };
        let pid = unsafe { get_i64(kCGWindowOwnerPID) } as u64;

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

#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u64) -> Result<()> {
    // Find the PID for this window by scanning the window list
    let windows = macos_window_list()?;
    let window = windows.iter().find(|w| w.id == window_id).ok_or_else(|| {
        DesktopError::WindowFailed(format!("No window found with id {window_id}"))
    })?;

    let pid = window.pid as i32;

    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
    match app {
        Some(app) => {
            #[allow(deprecated)]
            // ActivateIgnoringOtherApps deprecated in macOS 14 but still functional
            app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
            info!(window_id, pid, "Window focused (macOS)");
            Ok(())
        }
        None => Err(DesktopError::WindowFailed(format!(
            "No application found with PID {pid}"
        ))),
    }
}

/// Set a window's position and/or size via the `System Events` Accessibility
/// API (driven by `osascript`).
///
/// `window_id` is resolved to its owning process (`unix id`) and window title
/// by scanning [`macos_window_list`], mirroring [`macos_focus_window`]. The
/// `AppleScript` matches the target window by title within that process and
/// falls back to `window 1` when no title matches (e.g. untitled windows).
///
/// All dynamic values are passed through `AppleScript`'s `argv` rather than
/// interpolated into the script body, so titles containing quotes or other
/// metacharacters cannot break out of the string literal.
#[cfg(target_os = "macos")]
fn macos_set_window_bounds(
    window_id: u64,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
) -> Result<()> {
    let windows = macos_window_list()?;
    let window = windows.iter().find(|w| w.id == window_id).ok_or_else(|| {
        DesktopError::WindowFailed(format!("No window found with id {window_id}"))
    })?;
    let pid = window.pid;
    let title = window.title.clone();

    // The two operations share the same window-resolution preamble; the
    // trailing setter line differs. `{0}` is replaced with the AX setter.
    let setter = match (position, size) {
        (Some(_), None) => "set position of tw to {a, b}",
        (None, Some(_)) => "set size of tw to {a, b}",
        // The public `move_window` / `resize_window` entry points only ever
        // pass exactly one of the two; reject anything else defensively.
        _ => {
            return Err(DesktopError::WindowFailed(
                "macos_set_window_bounds requires exactly one of position or size".into(),
            ))
        }
    };
    let (a, b) = position
        .or(size)
        .expect("exactly one of position/size is Some");

    let script = format!(
        r#"on run argv
set pid to (item 1 of argv) as integer
set t to item 2 of argv
set a to (item 3 of argv) as integer
set b to (item 4 of argv) as integer
tell application "System Events"
set proc to first process whose unix id is pid
tell proc
set tw to missing value
repeat with w in windows
if name of w is t then
set tw to w
exit repeat
end if
end repeat
if tw is missing value then
if (count of windows) is 0 then error "process has no windows"
set tw to window 1
end if
{setter}
end tell
end tell
end run"#
    );

    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .arg(pid.to_string())
        .arg(title)
        .arg(a.to_string())
        .arg(b.to_string())
        .output()
        .map_err(|e| DesktopError::WindowFailed(format!("Failed to run osascript: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "osascript failed (is Accessibility permission granted?): {}",
            stderr.trim()
        )));
    }

    info!(window_id, pid, "Window bounds updated (macOS)");
    Ok(())
}

// ── Linux window management helpers ──────────────────────────────

#[cfg(target_os = "linux")]
fn linux_window_list() -> Result<Vec<WindowInfo>> {
    // Use wmctrl -l -p to list windows: <XID> <desktop> <PID> <machine> <title>
    let output = std::process::Command::new("wmctrl")
        .args(["-l", "-p"])
        .output()
        .map_err(|e| {
            DesktopError::WindowFailed(format!(
                "Failed to run wmctrl (is it installed? `sudo apt install wmctrl`): {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "wmctrl failed: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut windows = Vec::new();

    for line in stdout.lines() {
        // Format: 0x04000007  0 12345 hostname Window Title Here
        let parts: Vec<&str> = line.splitn(5, char::is_whitespace).collect();
        if parts.len() < 5 {
            continue;
        }

        let id_str = parts[0].trim_start_matches("0x").trim_start_matches("0X");
        let id = u64::from_str_radix(id_str, 16).unwrap_or(0);
        let pid: u64 = parts[2].trim().parse().unwrap_or(0);
        let title = parts[4].trim().to_string();

        windows.push(WindowInfo {
            id,
            title,
            owner: String::new(),
            pid,
        });
    }

    info!(count = windows.len(), "Window list retrieved (Linux)");
    Ok(windows)
}

#[cfg(target_os = "linux")]
fn linux_focus_window(window_id: u64) -> Result<()> {
    let id_hex = format!("0x{:08x}", window_id);
    let output = std::process::Command::new("wmctrl")
        .args(["-i", "-a", &id_hex])
        .output()
        .map_err(|e| {
            DesktopError::WindowFailed(format!(
                "Failed to run wmctrl (is it installed? `sudo apt install wmctrl`): {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "Failed to focus window {}: {}",
            id_hex,
            stderr.trim()
        )));
    }

    info!(window_id, "Window focused (Linux)");
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_move_window(window_id: u64, x: i32, y: i32) -> Result<()> {
    // `wmctrl -e <gravity>,<x>,<y>,<w>,<h>`; -1 leaves a dimension unchanged.
    let mvarg = format!("0,{x},{y},-1,-1");
    linux_wmctrl_geometry(window_id, &mvarg)
}

#[cfg(target_os = "linux")]
fn linux_resize_window(window_id: u64, width: u32, height: u32) -> Result<()> {
    let szarg = format!("0,-1,-1,{width},{height}");
    linux_wmctrl_geometry(window_id, &szarg)
}

#[cfg(target_os = "linux")]
fn linux_wmctrl_geometry(window_id: u64, geometry: &str) -> Result<()> {
    let id_hex = format!("0x{window_id:08x}");
    let output = std::process::Command::new("wmctrl")
        .args(["-i", "-r", &id_hex, "-e", geometry])
        .output()
        .map_err(|e| {
            DesktopError::WindowFailed(format!(
                "Failed to run wmctrl (is it installed? `sudo apt install wmctrl`): {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::WindowFailed(format!(
            "Failed to set geometry for window {id_hex}: {}",
            stderr.trim()
        )));
    }

    info!(window_id, geometry, "Window geometry updated (Linux)");
    Ok(())
}

// ── Windows window-management tests ──────────────────────────────
//
// These exercise the real Win32 entry points, so they only compile and run on
// Windows. They assert graceful failure on a bogus handle and on out-of-range
// dimensions — both must surface `WindowFailed` rather than panic or wrap.
#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;

    #[test]
    fn move_window_invalid_id_errors() {
        // HWND 1 is never a valid top-level window handle.
        let err = move_window(1, 0, 0).unwrap_err();
        assert!(matches!(err, DesktopError::WindowFailed(_)));
    }

    #[test]
    fn resize_window_invalid_id_errors() {
        let err = resize_window(1, 800, 600).unwrap_err();
        assert!(matches!(err, DesktopError::WindowFailed(_)));
    }

    #[test]
    fn resize_window_dimension_overflow_errors() {
        // u32 values past i32::MAX must be rejected, not wrapped to a negative.
        let err = resize_window(1, u32::MAX, 600).unwrap_err();
        match err {
            DesktopError::WindowFailed(msg) => {
                assert!(msg.contains("exceeds i32 range"), "got: {msg}");
            }
            other => panic!("expected WindowFailed, got {other:?}"),
        }
    }
}
