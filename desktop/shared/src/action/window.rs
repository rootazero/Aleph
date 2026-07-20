//! Window listing and focus management (platform-specific).

use tracing::info;

use crate::error::{DesktopError, Result};
use crate::WindowInfo;

/// List all visible on-screen windows.
///
/// - **macOS**: CoreGraphics `CGWindowListCopyWindowInfo` (on-screen only).
/// - **Linux**: `wmctrl -l -p -G`.
/// - **Windows**: `EnumWindows` over visible top-level windows; `WindowInfo.id`
///   carries the `HWND` so [`focus_window`] can round-trip it.
///
/// Every arm fills in [`WindowInfo::bounds`] where its platform query yields a
/// rectangle: the window frame in the global screen space clicks are issued in.
/// That geometry is what lets a caller capture (and reason about) one window
/// instead of the whole display. A field the platform does not report stays
/// `None` — unknown is never flattened to zero.
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
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible,
    };

    use crate::BoundingBox;

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

                    // A minimized window keeps WS_VISIBLE, so `IsWindowVisible`
                    // alone would report it as on screen; `IsIconic` is the only
                    // thing that says otherwise. Windows also parks a minimized
                    // window's rect at (-32000, -32000) — a sentinel, not a
                    // position — so its geometry is reported as unknown rather
                    // than as somewhere nothing can be clicked.
                    let iconic = IsIconic(hwnd).as_bool();
                    let mut rect = RECT::default();
                    let bounds = if iconic {
                        None
                    } else {
                        // Screen coordinates, top-left origin — the same space
                        // clicks are issued in.
                        GetWindowRect(hwnd, &mut rect).ok().map(|()| BoundingBox {
                            x: f64::from(rect.left),
                            y: f64::from(rect.top),
                            w: f64::from(rect.right - rect.left),
                            h: f64::from(rect.bottom - rect.top),
                        })
                    };

                    let state = &mut *(lparam.0 as *mut EnumState);
                    state.windows.push(WindowInfo {
                        id: hwnd.0 as usize as u64,
                        title,
                        owner: String::new(),
                        pid: u64::from(pid),
                        bounds,
                        on_screen: Some(!iconic),
                        // Windows has no window-level concept comparable to
                        // macOS' `kCGWindowLayer`: not told, not zero.
                        ..Default::default()
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
            LPARAM(std::ptr::addr_of_mut!(state) as isize),
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
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    };

    use crate::BoundingBox;

    // On-screen-only is deliberate, not inherited by accident.
    //
    // Dropping it would list minimized and other-Space windows — but those have
    // no live backing surface, so `screenshot_window` returns a blank or stale
    // frame for them, and macOS' focus path activates the owning app without
    // un-minimizing the window. The list exists to feed capture and targeting,
    // so widening it would hand the model window ids that silently do not work,
    // which is worse than not offering them. Listing them is a separate feature
    // (it needs a restore-then-capture step), not a flag change.
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let list = match copy_window_info(options, kCGNullWindowID) {
        Some(l) => l,
        None => return Ok(Vec::new()),
    };

    let mut windows = Vec::new();
    let values = list.get_all_values();

    for ptr in &values {
        // SAFETY: `*ptr` is a `CFDictionaryRef` element of the CFArray returned
        // by `copy_window_info`; `wrap_under_get_rule` retains it, so it stays
        // valid for the lifetime of `entry`.
        let entry: core_foundation::dictionary::CFDictionary<
            CFString,
            core_foundation::base::CFType,
        > = unsafe {
            TCFType::wrap_under_get_rule(*ptr as core_foundation::dictionary::CFDictionaryRef)
        };

        let get_str = |key: core_foundation::string::CFStringRef| -> String {
            // SAFETY: `key` is one of the `kCGWindow*` framework constant
            // `CFStringRef`s, valid for the process lifetime; `wrap_under_get_rule`
            // retains it correctly.
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
            // SAFETY: `key` is one of the `kCGWindow*` framework constant
            // `CFStringRef`s, valid for the process lifetime; `wrap_under_get_rule`
            // retains it correctly.
            unsafe {
                let key_cf = CFString::wrap_under_get_rule(key);
                entry
                    .find(&key_cf)
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|n| n.to_i64())
                    .unwrap_or(0)
            }
        };

        // `kCGWindowBounds` is a CGRect dictionary representation — CFString
        // keys ("X"/"Y"/"Width"/"Height") mapping to CFNumbers — in the global
        // screen POINT space with a top-left origin, i.e. the same space clicks
        // are issued in. Absent (or malformed) stays `None`: "not told" must not
        // be reported as a rectangle at the origin.
        let get_bounds = || -> Option<BoundingBox> {
            // SAFETY: `kCGWindowBounds` is a framework constant `CFStringRef`,
            // valid for the process lifetime; `wrap_under_get_rule` retains it.
            let key_cf = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
            // `downcast` type-checks the value against `CFDictionaryGetTypeID`,
            // so the re-wrap below is only reached for a real CFDictionary; the
            // K/V types then say how to read its entries.
            let untyped = entry.find(&key_cf)?.downcast::<CFDictionary>()?;
            // SAFETY: `untyped` is a live CFDictionary (checked above) and
            // `wrap_under_get_rule` retains the same reference.
            let rect: CFDictionary<CFString, core_foundation::base::CFType> =
                unsafe { CFDictionary::wrap_under_get_rule(untyped.as_concrete_TypeRef()) };
            let num = |k: &str| -> Option<f64> {
                rect.find(CFString::new(k))
                    .and_then(|v| v.downcast::<CFNumber>())
                    .and_then(|n| n.to_f64())
            };
            Some(BoundingBox {
                x: num("X")?,
                y: num("Y")?,
                w: num("Width")?,
                h: num("Height")?,
            })
        };

        // SAFETY: `kCGWindowName` is a framework constant valid for the
        // process lifetime; `get_str` retains it safely.
        let title = unsafe { get_str(kCGWindowName) };
        // SAFETY: `kCGWindowLayer` is a framework constant valid for the
        // process lifetime; `get_i64` retains it safely.
        let layer = unsafe { get_i64(kCGWindowLayer) };

        // Skip windows with empty title and non-zero layer (menu extras, etc.)
        if title.is_empty() && layer != 0 {
            continue;
        }

        // SAFETY: `kCGWindowNumber` is a framework constant valid for the
        // process lifetime; `get_i64` retains it safely.
        let id = unsafe { get_i64(kCGWindowNumber) } as u64;
        // SAFETY: `kCGWindowOwnerName` is a framework constant valid for the
        // process lifetime; `get_str` retains it safely.
        let owner = unsafe { get_str(kCGWindowOwnerName) };
        // SAFETY: `kCGWindowOwnerPID` is a framework constant valid for the
        // process lifetime; `get_i64` retains it safely.
        let pid = unsafe { get_i64(kCGWindowOwnerPID) } as u64;

        windows.push(WindowInfo {
            id,
            title,
            owner,
            pid,
            bounds: get_bounds(),
            // Both come free from the query we already ran: `layer` is read
            // above, and the option set is on-screen-only.
            layer: i32::try_from(layer).ok(),
            on_screen: Some(true),
        });
    }

    info!(count = windows.len(), "Window list retrieved (macOS)");
    Ok(windows)
}

#[cfg(target_os = "macos")]
fn macos_focus_window(window_id: u64) -> Result<()> {
    // Find the PID (and bounds) for this window by scanning the window list.
    let windows = macos_window_list()?;
    let window = windows.iter().find(|w| w.id == window_id).ok_or_else(|| {
        DesktopError::WindowFailed(format!("No window found with id {window_id}"))
    })?;

    let pid = window.pid as i32;

    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return Err(DesktopError::WindowFailed(format!(
            "No application found with PID {pid}"
        )));
    };
    #[allow(deprecated)]
    // ActivateIgnoringOtherApps deprecated in macOS 14 but still functional.
    app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);

    // Bring the *specific* window forward, not just whatever the app had
    // frontmost. Best-effort: without a match / Accessibility permission we have
    // still activated the app (the prior behavior), so never hard-fail here.
    match window.bounds.as_ref() {
        Some(bounds) => match super::window_ax::raise_window(window.pid, bounds) {
            Ok(true) => info!(window_id, pid, "Specific window raised via AX (macOS)"),
            Ok(false) => info!(
                window_id,
                pid, "No AX window match; app activated only (macOS)"
            ),
            Err(e) => tracing::warn!(window_id, "AX raise error: {e}; app activated only"),
        },
        None => info!(
            window_id,
            pid, "No window bounds; app activated only (macOS)"
        ),
    }
    Ok(())
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
    let ((a, b), setter) = match (position, size) {
        (Some((a, b)), None) => ((a, b), "set position of tw to {a, b}"),
        (None, Some((a, b))) => ((a, b), "set size of tw to {a, b}"),
        // The public `move_window` / `resize_window` entry points only ever
        // pass exactly one of the two; reject anything else defensively.
        _ => {
            return Err(DesktopError::WindowFailed(
                "macos_set_window_bounds requires exactly one of position or size".into(),
            ))
        }
    };

    // Precise path: resolve the exact CGWindowID to its AX window (by geometry)
    // and set position/size there. Falls through to the osascript-by-title path
    // below when AX can't resolve/authorize the window (no match / no perm).
    if let Some(bounds) = window.bounds.as_ref() {
        match super::window_ax::set_window_geometry(pid, bounds, position, size) {
            Ok(true) => {
                info!(window_id, pid, "Window bounds updated via AX (macOS)");
                return Ok(());
            }
            Ok(false) => { /* no AX match — fall through to osascript */ }
            Err(e) => {
                tracing::warn!(
                    window_id,
                    "AX set-bounds error: {e}; falling back to osascript"
                );
            }
        }
    }

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
    // `-G` adds the geometry columns; without them nothing can crop a capture
    // to a window or map its pixels back to click coordinates.
    let output = std::process::Command::new("wmctrl")
        .args(["-l", "-p", "-G"])
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
    let windows: Vec<WindowInfo> = stdout.lines().filter_map(parse_wmctrl_line).collect();

    info!(count = windows.len(), "Window list retrieved (Linux)");
    Ok(windows)
}

/// Parse one line of `wmctrl -l -p -G`.
///
/// ```text
/// <XID> <desktop> <PID> <x> <y> <w> <h> <machine> <title…>
/// 0x04000007  0 12345 100  50   800  600  hostname Window Title Here
/// ```
///
/// wmctrl pads the numeric columns, so fields are separated by *runs* of
/// whitespace, and the title — everything after the machine name — may itself
/// contain spaces, so it is taken as the untouched remainder of the line.
///
/// Geometry that fails to parse yields `bounds: None` rather than dropping the
/// window: an unlistable window is worse than one whose rectangle is unknown.
#[cfg(target_os = "linux")]
fn parse_wmctrl_line(line: &str) -> Option<WindowInfo> {
    use crate::BoundingBox;

    // XID, desktop, PID, x, y, w, h, machine.
    let mut fields = [""; 8];
    let mut rest = line;
    for slot in &mut fields {
        let start = rest.trim_start();
        let end = start.find(char::is_whitespace).unwrap_or(start.len());
        *slot = &start[..end];
        rest = &start[end..];
    }
    if fields.iter().any(|f| f.is_empty()) {
        return None;
    }

    let id_str = fields[0].trim_start_matches("0x").trim_start_matches("0X");
    let id = u64::from_str_radix(id_str, 16).ok()?;
    let pid: u64 = fields[2].parse().unwrap_or(0);

    // X11 geometry is in device pixels with a top-left origin — the same space
    // clicks are issued in. All four columns or none: half a rectangle is not a
    // rectangle.
    let bounds = match (
        fields[3].parse::<f64>(),
        fields[4].parse::<f64>(),
        fields[5].parse::<f64>(),
        fields[6].parse::<f64>(),
    ) {
        (Ok(x), Ok(y), Ok(w), Ok(h)) => Some(BoundingBox { x, y, w, h }),
        _ => None,
    };

    Some(WindowInfo {
        id,
        title: rest.trim().to_string(),
        owner: String::new(),
        pid,
        bounds,
        // wmctrl reports neither a stacking level nor whether the window is
        // iconified: not told, not zero/false.
        ..Default::default()
    })
}

#[cfg(target_os = "linux")]
fn linux_focus_window(window_id: u64) -> Result<()> {
    // Variable-width hex: a fixed 8-digit width would silently truncate a 64-bit
    // XID parsed by `window_list` (u64::from_str_radix), focusing the wrong window.
    let id_hex = format!("0x{window_id:x}");
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
    // Variable-width hex so a 64-bit XID is not truncated (see linux_focus_window).
    let id_hex = format!("0x{window_id:x}");
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

// ── Linux window-list parsing tests ──────────────────────────────
//
// `parse_wmctrl_line` is the only place the Linux arm can lose geometry or
// mangle a title, and it needs no wmctrl to exercise.
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;

    #[test]
    fn parses_padded_columns_and_a_title_with_spaces() {
        // wmctrl right-aligns the desktop column and left-pads the geometry
        // columns, so fields are separated by runs of spaces.
        let line = "0x04000007  0 12345 100  50   800  600  hostname My Window Title";
        let w = parse_wmctrl_line(line).expect("line parses");
        assert_eq!(w.id, 0x0400_0007);
        assert_eq!(w.pid, 12345);
        assert_eq!(w.title, "My Window Title");
        let b = w.bounds.expect("geometry");
        assert_eq!((b.x, b.y, b.w, b.h), (100.0, 50.0, 800.0, 600.0));
        // wmctrl reports neither of these.
        assert!(w.layer.is_none());
        assert!(w.on_screen.is_none());
    }

    #[test]
    fn keeps_the_window_when_geometry_is_unparseable() {
        // Unknown geometry must not delete the window from the list.
        let line = "0x0400000a  0 999 x y w h hostname Odd";
        let w = parse_wmctrl_line(line).expect("line parses");
        assert_eq!(w.id, 0x0400_000a);
        assert_eq!(w.title, "Odd");
        assert!(w.bounds.is_none());
    }

    #[test]
    fn negative_coordinates_survive() {
        // A window on a display left of the primary one has a negative origin.
        let line = "0x1 0 7 -1920 -100 640 480 host Left";
        let b = parse_wmctrl_line(line)
            .expect("line parses")
            .bounds
            .expect("geometry");
        assert_eq!((b.x, b.y), (-1920.0, -100.0));
    }

    #[test]
    fn rejects_a_truncated_line() {
        assert!(parse_wmctrl_line("0x1 0 7 hostname Title").is_none());
        assert!(parse_wmctrl_line("").is_none());
    }
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
