//! Window listing and focus management (platform-specific).

use tracing::info;

use crate::error::{DesktopError, Result};
use crate::WindowInfo;

/// List all visible on-screen windows.
///
/// - **Linux**: Uses `wmctrl -l -p` to enumerate windows.
/// - **macOS / Windows**: Returns `NotImplemented` (requires native APIs
///   not yet ported to this crate).
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
        Err(DesktopError::NotImplemented(
            "window_list not yet implemented for Windows in aleph-desktop crate".into(),
        ))
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
/// - **Linux**: Uses `wmctrl -i -a <hex_id>` to activate the window.
/// - **macOS / Windows**: Returns `NotImplemented`.
///
/// # Errors
///
/// - [`DesktopError::WindowFailed`] if the platform command fails.
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
        let _ = window_id;
        Err(DesktopError::NotImplemented(
            "focus_window not yet implemented for Windows in aleph-desktop crate".into(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = window_id;
        Err(DesktopError::NotImplemented(
            "focus_window not implemented on this platform".into(),
        ))
    }
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
        DesktopError::WindowFailed(format!("No window found with id {}", window_id))
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
            "No application found with PID {}",
            pid
        ))),
    }
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
