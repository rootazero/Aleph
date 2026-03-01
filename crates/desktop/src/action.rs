//! Action capabilities — mouse, keyboard, scroll, app launch, window management.
//!
//! This module provides cross-platform desktop input automation using `enigo`
//! for mouse/keyboard operations, and platform-specific commands for app
//! launching and window management.
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use tracing::info;

use crate::error::{DesktopError, Result};
use crate::{MouseButton, WindowInfo};

// ── Input actions (enigo-based, cross-platform) ──────────────────

/// Move the mouse to (x, y) and click the specified button.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if enigo cannot be created or the
///   mouse/click operation fails.
pub fn click(x: f64, y: f64, button: MouseButton) -> Result<()> {
    let enigo_button = match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))?;

    enigo
        .move_mouse(x as i32, y as i32, Coordinate::Abs)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;

    enigo
        .button(enigo_button, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to click: {e}")))?;

    info!(x, y, button = ?button, "Click performed");
    Ok(())
}

/// Type a string of text at the current cursor position.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if enigo cannot be created or the
///   text typing operation fails.
pub fn type_text(text: &str) -> Result<()> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))?;

    enigo
        .text(text)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to type text: {e}")))?;

    let char_count = text.chars().count();
    info!(chars = char_count, "Text typed");
    Ok(())
}

/// Press a key combination (e.g., Cmd+C, Ctrl+Shift+Tab).
///
/// `modifiers` contains modifier key names: "meta", "shift", "control", "alt".
/// `key` is the main key name: single character or named key ("return", "tab", etc.).
///
/// Modifiers are pressed in order, the main key is clicked, then modifiers
/// are released in reverse order.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the key or modifier names are invalid,
///   enigo cannot be created, or the key press operation fails.
pub fn key_combo(modifiers: &[String], key: &str) -> Result<()> {
    if key.is_empty() {
        return Err(DesktopError::InputFailed(
            "Key cannot be empty".into(),
        ));
    }

    let main_key = parse_key(key)?;
    let modifier_keys: Vec<Key> = modifiers
        .iter()
        .map(|s| parse_modifier(s))
        .collect::<Result<Vec<_>>>()?;

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))?;

    // Press all modifiers
    for m in &modifier_keys {
        enigo
            .key(*m, Direction::Press)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to press modifier: {e}")))?;
    }

    // Click the main key
    enigo
        .key(main_key, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to click key: {e}")))?;

    // Release modifiers in reverse order
    for m in modifier_keys.iter().rev() {
        enigo
            .key(*m, Direction::Release)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to release modifier: {e}")))?;
    }

    info!(modifiers = ?modifiers, key = %key, "Key combo performed");
    Ok(())
}

/// Scroll the mouse wheel.
///
/// `direction` is "up", "down", "left", or "right".
/// `amount` is the number of scroll clicks (always positive).
///
/// Enigo convention: positive length = down/right, negative = up/left.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the direction is invalid, enigo cannot
///   be created, or the scroll operation fails.
pub fn scroll(direction: &str, amount: i32) -> Result<()> {
    let (axis, length) = match direction {
        "down" => (Axis::Vertical, amount),
        "up" => (Axis::Vertical, -amount),
        "right" => (Axis::Horizontal, amount),
        "left" => (Axis::Horizontal, -amount),
        other => {
            return Err(DesktopError::InputFailed(format!(
                "Unknown scroll direction: '{}'. Expected up, down, left, or right",
                other
            )));
        }
    };

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))?;

    enigo
        .scroll(length, axis)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to scroll: {e}")))?;

    info!(direction, amount, "Scroll performed");
    Ok(())
}

// ── App launch (platform-specific) ──────────────────────────────

/// Launch an application by name or bundle ID.
///
/// - **macOS**: `open -b <bundle_id>` (or `open -a <app_name>` if not a bundle ID)
/// - **Linux**: `xdg-open <app_name>`
/// - **Windows**: `cmd /C start "" "<app_name>"`
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the application cannot be launched.
/// - [`DesktopError::NotImplemented`] on unsupported platforms.
pub fn launch_app(app_name: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Try bundle ID first (e.g., "com.apple.Safari"), then fall back to app name
        let (flag, name) = if app_name.contains('.') {
            ("-b", app_name)
        } else {
            ("-a", app_name)
        };

        let output = std::process::Command::new("open")
            .args([flag, name])
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}",
                app_name,
                stderr.trim()
            )));
        }

        info!(app_name, "App launched (macOS)");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("xdg-open")
            .arg(app_name)
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}",
                app_name,
                stderr.trim()
            )));
        }

        info!(app_name, "App launched (Linux)");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "start", "", app_name])
            .output()
            .map_err(|e| DesktopError::InputFailed(format!("Failed to launch app: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "Failed to launch '{}': {}",
                app_name,
                stderr.trim()
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

// ── Window management (platform-specific) ───────────────────────

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
    #[cfg(target_os = "linux")]
    {
        linux_window_list()
    }

    #[cfg(target_os = "windows")]
    {
        // TODO: Port EnumWindows implementation from Tauri bridge
        Err(DesktopError::NotImplemented(
            "window_list not yet implemented for Windows in aleph-desktop crate".into(),
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
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
    #[cfg(target_os = "linux")]
    {
        linux_focus_window(window_id)
    }

    #[cfg(target_os = "windows")]
    {
        // TODO: Port SetForegroundWindow implementation from Tauri bridge
        let _ = window_id;
        Err(DesktopError::NotImplemented(
            "focus_window not yet implemented for Windows in aleph-desktop crate".into(),
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = window_id;
        Err(DesktopError::NotImplemented(
            "focus_window not implemented on this platform".into(),
        ))
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

// ── Key parsing helpers ──────────────────────────────────────────

/// Parse a modifier name to an enigo [`Key`].
///
/// Recognized names (case-insensitive):
/// - Meta/Command/Cmd/Super/Win -> `Key::Meta`
/// - Shift -> `Key::Shift`
/// - Control/Ctrl -> `Key::Control`
/// - Alt/Option -> `Key::Alt`
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the name is not a recognized modifier.
pub fn parse_modifier(name: &str) -> Result<Key> {
    match name.to_lowercase().as_str() {
        "meta" | "command" | "cmd" | "super" | "win" => Ok(Key::Meta),
        "shift" => Ok(Key::Shift),
        "control" | "ctrl" => Ok(Key::Control),
        "alt" | "option" => Ok(Key::Alt),
        other => Err(DesktopError::InputFailed(format!(
            "Unknown modifier: '{}'. Expected meta/command/cmd, shift, control/ctrl, alt/option",
            other
        ))),
    }
}

/// Parse a key name to an enigo [`Key`].
///
/// Single characters are mapped to `Key::Unicode(ch)`. Multi-character
/// names are looked up in a table of common key names (case-insensitive).
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if the name is not a recognized key.
pub fn parse_key(name: &str) -> Result<Key> {
    // Single character keys
    if name.len() == 1 {
        let ch = name.chars().next().unwrap();
        return Ok(Key::Unicode(ch));
    }

    // Named keys (case-insensitive)
    match name.to_lowercase().as_str() {
        "space" => Ok(Key::Unicode(' ')),
        "return" | "enter" => Ok(Key::Return),
        "tab" => Ok(Key::Tab),
        "escape" | "esc" => Ok(Key::Escape),
        "backspace" | "delete" => Ok(Key::Backspace),
        "up" | "uparrow" => Ok(Key::UpArrow),
        "down" | "downarrow" => Ok(Key::DownArrow),
        "left" | "leftarrow" => Ok(Key::LeftArrow),
        "right" | "rightarrow" => Ok(Key::RightArrow),
        "home" => Ok(Key::Home),
        "end" => Ok(Key::End),
        "pageup" => Ok(Key::PageUp),
        "pagedown" => Ok(Key::PageDown),
        "f1" => Ok(Key::F1),
        "f2" => Ok(Key::F2),
        "f3" => Ok(Key::F3),
        "f4" => Ok(Key::F4),
        "f5" => Ok(Key::F5),
        "f6" => Ok(Key::F6),
        "f7" => Ok(Key::F7),
        "f8" => Ok(Key::F8),
        "f9" => Ok(Key::F9),
        "f10" => Ok(Key::F10),
        "f11" => Ok(Key::F11),
        "f12" => Ok(Key::F12),
        other => Err(DesktopError::InputFailed(format!(
            "Unknown key: '{}'. Use single char or named key (space, return, tab, escape, etc.)",
            other
        ))),
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use enigo::Key;

    // ── parse_modifier tests ──────────────────────────────────

    #[test]
    fn test_parse_modifier_meta() {
        // All aliases for Meta should resolve correctly.
        for name in &["meta", "command", "cmd", "super", "win"] {
            let key = parse_modifier(name).unwrap();
            assert_eq!(key, Key::Meta, "Expected Meta for '{name}'");
        }
    }

    #[test]
    fn test_parse_modifier_shift() {
        assert_eq!(parse_modifier("shift").unwrap(), Key::Shift);
        assert_eq!(parse_modifier("Shift").unwrap(), Key::Shift);
        assert_eq!(parse_modifier("SHIFT").unwrap(), Key::Shift);
    }

    #[test]
    fn test_parse_modifier_control() {
        assert_eq!(parse_modifier("control").unwrap(), Key::Control);
        assert_eq!(parse_modifier("ctrl").unwrap(), Key::Control);
        assert_eq!(parse_modifier("CTRL").unwrap(), Key::Control);
    }

    #[test]
    fn test_parse_modifier_alt() {
        assert_eq!(parse_modifier("alt").unwrap(), Key::Alt);
        assert_eq!(parse_modifier("option").unwrap(), Key::Alt);
        assert_eq!(parse_modifier("Option").unwrap(), Key::Alt);
    }

    #[test]
    fn test_parse_modifier_unknown() {
        let err = parse_modifier("capslock").unwrap_err();
        assert!(
            matches!(err, DesktopError::InputFailed(_)),
            "Expected InputFailed, got: {err:?}"
        );
    }

    // ── parse_key tests ──────────────────────────────────────

    #[test]
    fn test_parse_key_single_char() {
        assert_eq!(parse_key("c").unwrap(), Key::Unicode('c'));
        assert_eq!(parse_key("A").unwrap(), Key::Unicode('A'));
        assert_eq!(parse_key("1").unwrap(), Key::Unicode('1'));
    }

    #[test]
    fn test_parse_key_return() {
        assert_eq!(parse_key("return").unwrap(), Key::Return);
        assert_eq!(parse_key("enter").unwrap(), Key::Return);
        assert_eq!(parse_key("Return").unwrap(), Key::Return);
    }

    #[test]
    fn test_parse_key_tab() {
        assert_eq!(parse_key("tab").unwrap(), Key::Tab);
        assert_eq!(parse_key("Tab").unwrap(), Key::Tab);
    }

    #[test]
    fn test_parse_key_escape() {
        assert_eq!(parse_key("escape").unwrap(), Key::Escape);
        assert_eq!(parse_key("esc").unwrap(), Key::Escape);
    }

    #[test]
    fn test_parse_key_arrows() {
        assert_eq!(parse_key("up").unwrap(), Key::UpArrow);
        assert_eq!(parse_key("down").unwrap(), Key::DownArrow);
        assert_eq!(parse_key("left").unwrap(), Key::LeftArrow);
        assert_eq!(parse_key("right").unwrap(), Key::RightArrow);
        assert_eq!(parse_key("UpArrow").unwrap(), Key::UpArrow);
    }

    #[test]
    fn test_parse_key_function_keys() {
        assert_eq!(parse_key("f1").unwrap(), Key::F1);
        assert_eq!(parse_key("F12").unwrap(), Key::F12);
    }

    #[test]
    fn test_parse_key_space() {
        assert_eq!(parse_key("space").unwrap(), Key::Unicode(' '));
    }

    #[test]
    fn test_parse_key_unknown() {
        let err = parse_key("nonexistent").unwrap_err();
        assert!(
            matches!(err, DesktopError::InputFailed(_)),
            "Expected InputFailed, got: {err:?}"
        );
    }

    // ── scroll validation ────────────────────────────────────

    #[test]
    fn test_scroll_invalid_direction() {
        let err = scroll("diagonal", 3).unwrap_err();
        assert!(
            matches!(err, DesktopError::InputFailed(_)),
            "Expected InputFailed, got: {err:?}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("diagonal"),
            "Error should mention the invalid direction"
        );
    }

    // ── key_combo validation ─────────────────────────────────

    #[test]
    fn test_key_combo_empty_key() {
        let err = key_combo(&[], "").unwrap_err();
        assert!(
            matches!(err, DesktopError::InputFailed(_)),
            "Expected InputFailed for empty key, got: {err:?}"
        );
    }
}
