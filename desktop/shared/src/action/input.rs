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
