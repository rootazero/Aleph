//! Window listing and focus management (platform-specific).

// Only the macOS and Windows arms log inline; the Linux arms delegate to
// `crate::linux::app` / `crate::action::window_linux`, which own their own logging.
#[cfg(not(target_os = "linux"))]
use tracing::info;

#[cfg(not(target_os = "linux"))]
use crate::error::DesktopError;
use crate::error::Result;
use crate::WindowInfo;

/// List all visible on-screen windows.
///
/// - **macOS**: CoreGraphics `CGWindowListCopyWindowInfo` (on-screen only).
/// - **Linux**: native EWMH `_NET_CLIENT_LIST` on X11, or the compositor IPC on
///   sway / Hyprland — see [`super::window_linux`].
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
        super::window_linux::window_list()
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
/// - **Linux**: EWMH `_NET_ACTIVE_WINDOW` on X11, or the compositor IPC — see
///   [`super::window_linux`].
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
        super::window_linux::focus_window(window_id)
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
/// - **Linux**: EWMH `_NET_MOVERESIZE_WINDOW` on X11, or the compositor IPC — see
///   [`super::window_linux`] (preserves size).
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
        super::window_linux::move_window(window_id, x, y)
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
/// - **Linux**: EWMH `_NET_MOVERESIZE_WINDOW` on X11, or the compositor IPC — see
///   [`super::window_linux`] (preserves position).
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
        super::window_linux::resize_window(window_id, width, height)
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

/// Windows window list, projected from the shared enumeration in
/// [`crate::win_window`].
///
/// The enumeration rules — visibility, DWM cloaking, tool windows, and the
/// visible-frame bounds — live there because the running-app list and the UI
/// Automation root resolver need exactly the same answers, and three private
/// copies of `EnumWindows` is how they came to disagree.
///
/// `owner` is filled from the owning process's executable, which is a handle a
/// caller can pass back as `app:`. A window title is not one: File Explorer
/// titles itself after the open folder, so `app: "explorer"` never matched.
#[cfg(target_os = "windows")]
fn windows_window_list() -> Result<Vec<WindowInfo>> {
    use std::collections::HashMap;

    // One version-resource read per *process*, not per window.
    let mut owners: HashMap<u32, String> = HashMap::new();

    let windows: Vec<WindowInfo> = crate::win_window::enumerate_top_level()
        .into_iter()
        // An untitled window cannot be named or chosen from a list; the
        // enumeration keeps them because the pid→window resolver needs them.
        .filter(|w| !w.title.is_empty())
        .map(|w| {
            let owner = owners
                .entry(w.pid)
                .or_insert_with(|| {
                    crate::win_window::process_image_path(w.pid)
                        .map(|exe| crate::win_window::friendly_app_name(&exe))
                        .unwrap_or_default()
                })
                .clone();
            WindowInfo {
                id: w.id,
                title: w.title,
                owner,
                pid: u64::from(w.pid),
                bounds: w.bounds,
                on_screen: Some(!w.minimized),
                // Windows has no window-level concept comparable to macOS'
                // `kCGWindowLayer`: not told, not zero.
                ..Default::default()
            }
        })
        .collect();

    info!(count = windows.len(), "Window list retrieved (Windows)");
    Ok(windows)
}

/// How long to wait for the window server to actually make a window foreground.
///
/// `SetForegroundWindow` is asynchronous *and* refusable: Windows' foreground
/// lock denies the change outright when the calling process did not receive the
/// last input, and the API still returns without saying so usefully. Polling
/// `GetForegroundWindow` for the real outcome is the only way to know; orca's
/// Windows runtime lands on the same 500 ms ceiling (`Wait-OrcaWindowFocused`).
///
/// macOS is the same shape for the same reason. `NSRunningApplication.activate`
/// posts a request to the window server and returns before it is honoured — or
/// declined, which macOS does for a process that is not itself in the user's
/// activation chain. `isActive` is the outcome, so it is polled on the same
/// budget rather than assumed.
#[cfg(any(target_os = "windows", target_os = "macos"))]
const FOREGROUND_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(any(target_os = "windows", target_os = "macos"))]
const FOREGROUND_POLL: std::time::Duration = std::time::Duration::from_millis(25);

/// Apple Event ceiling embedded in the `System Events` window-geometry script's
/// `with timeout of … seconds` block.
///
/// Bounds the *event*, which is the usual way this call hangs: every line inside
/// the `tell` block is a synchronous round trip into System Events, which is
/// itself waiting on the target app.
#[cfg(target_os = "macos")]
const AX_SCRIPT_TIMEOUT_SECS: u32 = 15;

/// Wall-clock ceiling on the `osascript` **process**.
///
/// The Apple Event timeout above is not the same guarantee, and the difference
/// is the whole reason this exists: it only applies once the event is in flight.
/// `osascript` still has to start, connect to System Events, and — if System
/// Events is itself wedged or being launched — may never get that far. Without a
/// process-level cap that is an unbounded `Command::output()` on a blocking
/// thread, which is exactly the failure this repository has already paid for on
/// three other shell-out paths (`xclip`, `notify-send`, `ffmpeg`).
///
/// Sized above the Apple Event ceiling so the script's own error — which is
/// specific and actionable — wins whenever it is available, and this only fires
/// when the process never got to run the script at all.
#[cfg(target_os = "macos")]
const AX_SCRIPT_PROCESS_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(AX_SCRIPT_TIMEOUT_SECS as u64 + 5);

/// Raise a window and report whether it actually reached the foreground.
///
/// This used to fire `SetForegroundWindow` and discard the result — the comment
/// said a refusal "is not a hard failure", so the tool answered `focused: true`
/// for a window Windows had merely flashed in the taskbar. The model then aimed
/// its next keystroke at an app that was never in front.
///
/// **The refusal is not worked around on purpose.** The documented escalation
/// (`AttachThreadInput` onto the foreground thread, then set, then detach) does
/// force the change, and it is exactly the "steal the focus the user is holding"
/// move R5 rules out — the foreground lock is Windows protecting the person
/// typing. Every background use-case that used to need focus now has a path that
/// does not: `set_value` / `ax_action` address an element inside any process via
/// UI Automation, and `screenshot_window` captures an occluded window through
/// `PrintWindow`. So the honest failure costs the model nothing but a redirect.
#[cfg(target_os = "windows")]
fn windows_focus_window(window_id: u64) -> Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
        SW_SHOW,
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
        // Restoring is what makes a minimized window focusable at all; a
        // non-minimized one still needs `SW_SHOW` if it was hidden.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        let _ = SetForegroundWindow(hwnd);

        // Poll for the outcome rather than assume it. Reporting success for a
        // focus change the foreground lock refused is how a model ends up
        // typing into the app the user was working in — the exact failure the
        // type_text focus gate exists to catch, arriving one call earlier.
        let deadline = std::time::Instant::now() + FOREGROUND_WAIT;
        loop {
            if GetForegroundWindow() == hwnd {
                info!(window_id, "Window focused (Windows)");
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(FOREGROUND_POLL);
        }
    }

    Err(DesktopError::WindowFailed(format!(
        "window {window_id} was restored but Windows did not make it foreground within {} ms. \
         The foreground lock blocks a background process from stealing focus while the user is \
         typing elsewhere. Retry after the user interacts with the desktop, or address the app \
         directly with set_value / ax_action, which need no focus.",
        FOREGROUND_WAIT.as_millis()
    )))
}

/// Resolve a window id to a live `HWND`, or say it is not a window.
#[cfg(target_os = "windows")]
fn windows_hwnd(window_id: u64) -> Result<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    let hwnd = HWND(window_id as usize as *mut core::ffi::c_void);
    // SAFETY: documented validity check; a stale or forged id answers `false`.
    if unsafe { IsWindow(hwnd) }.as_bool() {
        Ok(hwnd)
    } else {
        Err(DesktopError::WindowFailed(format!(
            "No window found with id {window_id}"
        )))
    }
}

/// Take a maximized window out of the maximized state before it is moved or
/// resized, **without** activating it.
///
/// A maximized window ignores geometry: Windows keeps the `SW_SHOWMAXIMIZED`
/// state, so `SetWindowPos` either does nothing visible or leaves the window in
/// a state that snaps back at the next restore. `ShowWindow(SW_RESTORE)` fixes
/// that but *activates* the window — yanking focus off whatever the user is
/// typing in, which is the one thing R5 rules out. `SetWindowPlacement` with
/// `SW_SHOWNOACTIVATE` changes the state and nothing else.
#[cfg(target_os = "windows")]
fn windows_unmaximize(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, IsZoomed, SetWindowPlacement, SW_SHOWNOACTIVATE, WINDOWPLACEMENT,
    };

    // SAFETY: `hwnd` is validated; both placement calls read/write a
    // fully-initialized `WINDOWPLACEMENT` whose `length` field is set as the API
    // requires.
    unsafe {
        if !IsZoomed(hwnd).as_bool() {
            return;
        }
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if GetWindowPlacement(hwnd, std::ptr::addr_of_mut!(placement)).is_err() {
            return;
        }
        placement.showCmd = SW_SHOWNOACTIVATE.0 as u32;
        let _ = SetWindowPlacement(hwnd, std::ptr::addr_of!(placement));
    }
}

/// Reposition and/or resize a window via Win32 `SetWindowPos`.
///
/// `SWP_NOZORDER | SWP_NOACTIVATE` are always added so the call never changes
/// the Z-order or steals focus; callers pass `SWP_NOSIZE` (move only) or
/// `SWP_NOMOVE` (resize only) to pin the dimension they want preserved.
///
/// `x`/`y`/`cx`/`cy` are **raw** window-rect values, already compensated for the
/// invisible DWM border by the callers via
/// [`crate::win_window::FramePadding`] — the geometry a caller passes in is the
/// visible frame, because that is the geometry `window_list` handed it.
#[cfg(target_os = "windows")]
fn windows_set_window_pos(
    hwnd: windows::Win32::Foundation::HWND,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: windows::Win32::UI::WindowsAndMessaging::SET_WINDOW_POS_FLAGS,
) -> Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

    // SAFETY: `hwnd` was validated by `windows_hwnd`; `SetWindowPos` is a
    // documented Win32 API and the insert-after handle is ignored because
    // `SWP_NOZORDER` is always set.
    unsafe {
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

    let hwnd = windows_hwnd(window_id)?;
    windows_unmaximize(hwnd);
    // `(x, y)` is where the caller wants the *visible* frame, which is the space
    // `window_list` reports bounds in; `SetWindowPos` speaks the raw rect.
    let (raw_x, raw_y) = crate::win_window::frame_padding(hwnd).raw_origin(x, y);
    windows_set_window_pos(hwnd, raw_x, raw_y, 0, 0, SWP_NOSIZE)?;
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

    let hwnd = windows_hwnd(window_id)?;
    windows_unmaximize(hwnd);
    // Same asymmetry as the move path: the caller means the visible frame.
    let (raw_w, raw_h) = crate::win_window::frame_padding(hwnd).raw_size(cx, cy);
    windows_set_window_pos(hwnd, 0, 0, raw_w, raw_h, SWP_NOMOVE)?;
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
    //
    // The return value is deliberately discarded: measured on macOS 27, it is
    // `true` even when the activation is refused outright and the app never
    // comes forward. Only `isActive` below distinguishes the two.
    let _ = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);

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

    // Verify rather than assume — this function used to discard `activate`'s
    // answer and return `Ok(())` unconditionally.
    //
    // Measured on macOS 27 from a non-bundled background process (what
    // `aleph-server` is): launching TextEdit and then activating it left Safari
    // frontmost for the full second, `isActive` false throughout, while
    // `activate` reported `true` — and `AXUIElementSetAttributeValue(app,
    // AXFrontmost, true)` returned `kAXErrorSuccess` and did nothing either.
    // macOS simply declines to hand the foreground to a process the user is not
    // driving. So the old `Ok(())` was reporting a state change that provably
    // did not happen, and the model's next keystroke went to whatever the user
    // actually had in front of them. Polling `isActive` is the only signal that
    // tells the two apart (it reads `true` immediately for an app that is
    // genuinely active, so a verified success is never lost to the poll).
    let deadline = std::time::Instant::now() + FOREGROUND_WAIT;
    loop {
        if app.isActive() {
            info!(window_id, pid, "Window focused (macOS)");
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(FOREGROUND_POLL);
    }

    Err(DesktopError::WindowFailed(format!(
        "window {window_id} (pid {pid}) did not become frontmost within {} ms — macOS declines \
         activation requests from a background process the user is not currently driving. \
         Nothing you need here requires focus, though: set_value / ax_action address the element \
         directly, screenshot with window_id captures the window even while it is covered, and \
         click / type_text / key_combo deliver into the process when given `app`, `pid` or \
         `window_id`.",
        FOREGROUND_WAIT.as_millis()
    )))
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

    // Two caps, and both are needed. `with timeout of N seconds` bounds the Apple
    // Event — the thing that usually hangs, because every line inside the `tell`
    // block is a synchronous round trip into System Events, which is itself
    // waiting on the target app. `AX_SCRIPT_PROCESS_TIMEOUT` bounds the process,
    // for the case where the script never starts running at all.
    let script = format!(
        r#"on run argv
set pid to (item 1 of argv) as integer
set t to item 2 of argv
set a to (item 3 of argv) as integer
set b to (item 4 of argv) as integer
with timeout of {AX_SCRIPT_TIMEOUT_SECS} seconds
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
end timeout
end run"#
    );

    let mut cmd = crate::script_exec::hidden_std_command("osascript");
    cmd.arg("-e")
        .arg(&script)
        .arg(pid.to_string())
        .arg(title)
        .arg(a.to_string())
        .arg(b.to_string());
    let output = crate::script_exec::output_capped_blocking(
        cmd,
        AX_SCRIPT_PROCESS_TIMEOUT,
        "osascript (window geometry)",
    )
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
