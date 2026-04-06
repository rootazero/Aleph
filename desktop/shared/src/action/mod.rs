//! Action capabilities — mouse, keyboard, scroll, app launch, window management.
//!
//! This module provides cross-platform desktop input automation using `enigo`
//! for mouse/keyboard operations, and platform-specific commands for app
//! launching and window management.
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

pub mod app_launch;
pub mod input;
pub mod key_parse;
pub mod window;

pub use app_launch::{launch_app, quit_app};
pub use input::{
    click, clipboard_read, clipboard_write, cursor_position, double_click, drag, hover, key_combo,
    mouse_button, scroll, type_text,
};
pub use window::{focus_window, window_list};

use enigo::{Button, Enigo, Settings};

use crate::error::{DesktopError, Result};
use crate::MouseButton;

// ── Coordinate validation ────────────────────────────────────────

/// Validate an `f64` coordinate for safe conversion to `i32`.
///
/// Rejects NaN, Infinity, and values outside the `i32` representable range.
pub(crate) fn validate_coordinate(value: f64, name: &str) -> Result<i32> {
    if value.is_nan() {
        return Err(DesktopError::InputFailed(format!(
            "Coordinate '{name}' is NaN"
        )));
    }
    if value.is_infinite() {
        return Err(DesktopError::InputFailed(format!(
            "Coordinate '{name}' is infinite"
        )));
    }
    if value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(DesktopError::InputFailed(format!(
            "Coordinate '{name}' value {value} is outside i32 range"
        )));
    }
    Ok(value as i32)
}

/// Create a new Enigo instance.
pub(crate) fn new_enigo() -> Result<Enigo> {
    Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))
}

/// Convert Aleph's MouseButton to enigo's Button.
pub(crate) fn to_enigo_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use enigo::Button;

    // ── coordinate validation ────────────────────────────────────

    #[test]
    fn test_validate_coordinate_normal() {
        assert_eq!(validate_coordinate(100.0, "x").unwrap(), 100);
        assert_eq!(validate_coordinate(-50.5, "y").unwrap(), -50);
        assert_eq!(validate_coordinate(0.0, "x").unwrap(), 0);
    }

    #[test]
    fn test_validate_coordinate_nan() {
        let err = validate_coordinate(f64::NAN, "x").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
        let msg = format!("{err}");
        assert!(msg.contains("NaN"), "Error should mention NaN: {msg}");
    }

    #[test]
    fn test_validate_coordinate_infinity() {
        let err = validate_coordinate(f64::INFINITY, "x").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
        let msg = format!("{err}");
        assert!(
            msg.contains("infinite"),
            "Error should mention infinite: {msg}"
        );

        let err = validate_coordinate(f64::NEG_INFINITY, "y").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }

    #[test]
    fn test_validate_coordinate_out_of_range() {
        let err = validate_coordinate(3e10, "x").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
        let msg = format!("{err}");
        assert!(
            msg.contains("outside i32 range"),
            "Error should mention range: {msg}"
        );
    }

    #[test]
    fn test_validate_coordinate_boundary() {
        // i32::MAX = 2_147_483_647, i32::MIN = -2_147_483_648
        assert_eq!(
            validate_coordinate(f64::from(i32::MAX), "x").unwrap(),
            i32::MAX
        );
        assert_eq!(
            validate_coordinate(f64::from(i32::MIN), "y").unwrap(),
            i32::MIN
        );
    }

    // ── to_enigo_button ──────────────────────────────────────────

    #[test]
    fn test_to_enigo_button() {
        assert_eq!(to_enigo_button(MouseButton::Left), Button::Left);
        assert_eq!(to_enigo_button(MouseButton::Right), Button::Right);
        assert_eq!(to_enigo_button(MouseButton::Middle), Button::Middle);
    }
}
