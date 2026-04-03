//! Mouse and keyboard input actions using enigo.

use enigo::{Axis, Coordinate, Direction, Keyboard, Mouse};
use tracing::info;

use super::{new_enigo, to_enigo_button, validate_coordinate};
use crate::error::Result;
use crate::error::DesktopError;
use crate::MouseButton;

/// Move the mouse to (x, y) and click the specified button.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if coordinates are invalid (NaN, Infinity,
///   out of i32 range), enigo cannot be created, or the mouse/click operation
///   fails.
pub fn click(x: f64, y: f64, button: MouseButton) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;

    let enigo_button = to_enigo_button(button);

    let mut enigo = new_enigo()?;

    enigo
        .move_mouse(ix, iy, Coordinate::Abs)
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
    let mut enigo = new_enigo()?;

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
        return Err(DesktopError::InputFailed("Key cannot be empty".into()));
    }

    let main_key = super::key_parse::parse_key(key)?;
    let modifier_keys = modifiers
        .iter()
        .map(|s| super::key_parse::parse_modifier(s))
        .collect::<Result<Vec<_>>>()?;

    let mut enigo = new_enigo()?;

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

    let mut enigo = new_enigo()?;

    enigo
        .scroll(length, axis)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to scroll: {e}")))?;

    info!(direction, amount, "Scroll performed");
    Ok(())
}

/// Double-click at (x, y).
pub fn double_click(x: f64, y: f64, button: MouseButton) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let btn = to_enigo_button(button);
    let mut enigo = new_enigo()?;
    enigo
        .move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    enigo
        .button(btn, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to click: {e}")))?;
    enigo
        .button(btn, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to double-click: {e}")))?;
    info!(x, y, button = ?button, "Double-click performed");
    Ok(())
}

/// Drag from (start_x, start_y) to (end_x, end_y) with optional animation.
pub fn drag(
    start_x: f64,
    start_y: f64,
    end_x: f64,
    end_y: f64,
    duration_ms: Option<u64>,
) -> Result<()> {
    use std::time::Duration;

    let sx = validate_coordinate(start_x, "start_x")?;
    let sy = validate_coordinate(start_y, "start_y")?;
    let ex = validate_coordinate(end_x, "end_x")?;
    let ey = validate_coordinate(end_y, "end_y")?;

    let mut enigo = new_enigo()?;

    enigo
        .move_mouse(sx, sy, Coordinate::Abs)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to move to start: {e}")))?;
    enigo
        .button(enigo::Button::Left, Direction::Press)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to press for drag: {e}")))?;

    match duration_ms {
        Some(ms) if ms > 0 => {
            let steps = ((ms as f64 / 1000.0) * 60.0).ceil().max(1.0) as u64;
            let step_delay = Duration::from_millis(ms / steps.max(1));
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let eased = 1.0 - (1.0 - t).powi(3);
                let cx = sx as f64 + (ex as f64 - sx as f64) * eased;
                let cy = sy as f64 + (ey as f64 - sy as f64) * eased;
                enigo
                    .move_mouse(cx as i32, cy as i32, Coordinate::Abs)
                    .map_err(|e| {
                        DesktopError::InputFailed(format!("Failed to move during drag: {e}"))
                    })?;
                std::thread::sleep(step_delay);
            }
        }
        _ => {
            enigo
                .move_mouse(ex, ey, Coordinate::Abs)
                .map_err(|e| {
                    DesktopError::InputFailed(format!("Failed to move to end: {e}"))
                })?;
        }
    }

    enigo
        .button(enigo::Button::Left, Direction::Release)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to release after drag: {e}")))?;

    info!(start_x, start_y, end_x, end_y, ?duration_ms, "Drag performed");
    Ok(())
}

/// Move mouse to (x, y) without clicking.
pub fn hover(x: f64, y: f64) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let mut enigo = new_enigo()?;
    enigo
        .move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    info!(x, y, "Hover performed");
    Ok(())
}

/// Get current mouse cursor position.
pub fn cursor_position() -> Result<(f64, f64)> {
    let enigo = new_enigo()?;
    let (x, y) = enigo
        .location()
        .map_err(|e| DesktopError::InputFailed(format!("Failed to get cursor position: {e}")))?;
    Ok((x as f64, y as f64))
}

/// Press, release, or click a mouse button at (x, y).
pub fn mouse_button(
    x: f64,
    y: f64,
    button: MouseButton,
    action: crate::PressAction,
) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;
    let btn = to_enigo_button(button);
    let dir = match action {
        crate::PressAction::Press => Direction::Press,
        crate::PressAction::Release => Direction::Release,
        crate::PressAction::Click => Direction::Click,
    };
    let mut enigo = new_enigo()?;
    enigo
        .move_mouse(ix, iy, Coordinate::Abs)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to move mouse: {e}")))?;
    enigo
        .button(btn, dir)
        .map_err(|e| DesktopError::InputFailed(format!("Failed mouse button action: {e}")))?;
    info!(x, y, button = ?button, action = ?action, "Mouse button action performed");
    Ok(())
}

/// Read text from system clipboard.
pub fn clipboard_read() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

        let pb = NSPasteboard::generalPasteboard();
        let pasteboard_type = unsafe { NSPasteboardTypeString };
        match pb.stringForType(pasteboard_type) {
            Some(s) => Ok(s.to_string()),
            None => Ok(String::new()),
        }
    }

    #[cfg(target_os = "linux")]
    {
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .or_else(|_| {
                std::process::Command::new("xsel")
                    .args(["--clipboard", "--output"])
                    .output()
            })
            .map_err(|e| {
                DesktopError::InputFailed(format!(
                    "Failed to read clipboard (install xclip or xsel): {e}"
                ))
            })?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    #[cfg(target_os = "windows")]
    {
        Err(DesktopError::NotImplemented(
            "clipboard_read not yet implemented for Windows".into(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err(DesktopError::NotImplemented(
            "clipboard_read not implemented on this platform".into(),
        ))
    }
}

/// Write text to system clipboard.
pub fn clipboard_write(text: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        use objc2_foundation::NSString;

        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ns_string = NSString::from_str(text);
        let pasteboard_type = unsafe { NSPasteboardTypeString };
        pb.setString_forType(&ns_string, pasteboard_type);
        info!("Clipboard write performed");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .or_else(|_| {
                std::process::Command::new("xsel")
                    .args(["--clipboard", "--input"])
                    .stdin(std::process::Stdio::piped())
                    .spawn()
            })
            .map_err(|e| {
                DesktopError::InputFailed(format!(
                    "Failed to write clipboard (install xclip or xsel): {e}"
                ))
            })?;
        if let Some(ref mut stdin) = child.stdin {
            stdin.write_all(text.as_bytes()).map_err(|e| {
                DesktopError::InputFailed(format!("Failed to write to clipboard: {e}"))
            })?;
        }
        child
            .wait()
            .map_err(|e| DesktopError::InputFailed(format!("Clipboard process failed: {e}")))?;
        info!("Clipboard write performed");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let _ = text;
        Err(DesktopError::NotImplemented(
            "clipboard_write not yet implemented for Windows".into(),
        ))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        Err(DesktopError::NotImplemented(
            "clipboard_write not implemented on this platform".into(),
        ))
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::DesktopError;

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

    #[test]
    fn test_key_combo_empty_key() {
        let err = key_combo(&[], "").unwrap_err();
        assert!(
            matches!(err, DesktopError::InputFailed(_)),
            "Expected InputFailed for empty key, got: {err:?}"
        );
    }
}
