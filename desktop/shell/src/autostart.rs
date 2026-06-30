//! Launch-at-login control, exposed to the Panel over the shell's loopback IPC
//! surface. Thin wrapper over `tauri-plugin-autostart` — it holds no state and
//! makes no policy decision (the user drives it from Settings → General). The
//! plugin maps to a macOS LaunchAgent, the Windows registry Run key, and an XDG
//! autostart `.desktop` entry respectively.
//!
//! Only reachable from an origin the IPC capability allows (loopback Panel /
//! bundled pages). A remote-origin Panel cannot call these; the Panel hides the
//! section when the `get_autostart` probe fails (see settings/desktop_autostart).

use tauri_plugin_autostart::ManagerExt;

/// Whether launch-at-login is currently enabled at the OS level.
#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Enable or disable launch-at-login. Idempotent: enabling when already enabled
/// (or disabling when already disabled) is a no-op the plugin tolerates.
#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| e.to_string())
    } else {
        mgr.disable().map_err(|e| e.to_string())
    }
}
