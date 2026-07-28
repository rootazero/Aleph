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
pub mod open_path;
// Wayland input fallback via `ydotool`. The argv/keycode helpers are pure and
// host-testable; their only non-test callers are gated to Linux, so suppress
// dead-code lints on other platforms where the runtime side is compiled out.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod wayland_input;
pub mod window;
#[cfg(target_os = "macos")]
mod window_ax;
// Linux window management (native X11/EWMH + sway/Hyprland IPC). The backend
// selection and the compositor argv/JSON parsers are pure and host-testable;
// only the X11 arm is Linux-gated. `window.rs` calls in under `cfg(linux)`, so
// on other platforms the module has no non-test consumer.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod window_linux;

pub use app_launch::{launch_app, quit_app};
pub use input::{
    click, clipboard_read, clipboard_write, cursor_position, double_click, drag, hover, key_button,
    key_combo, mouse_button, scroll, type_text,
};
pub use open_path::open;
pub use window::{focus_window, move_window, resize_window, window_list};

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
///
/// Self-releasing: `Settings::release_keys_when_dropped` defaults to `true`, so
/// anything still held when the instance drops is released. That is what we want
/// for the one-shot actions (click, key_combo, type_text) — they press and
/// release within the call, and the Drop is a safety net.
pub(crate) fn new_enigo() -> Result<Enigo> {
    Enigo::new(&Settings::default())
        .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))
}

/// Create an Enigo instance whose Drop does **not** release held keys/buttons.
///
/// The press/release actions (`key_button`, `mouse_button`) are the only ones
/// whose whole purpose is to leave input held *across* calls — holding Shift
/// while a later call clicks, or pressing the mouse down before a series of
/// drag moves. Every Enigo instance is per-call, so the default
/// `release_keys_when_dropped: true` released the key the instant the function
/// returned, making `PressAction::Press` a structural no-op.
///
/// The caller owns the cleanup instead: the desktop tool records every press in
/// its held-input ledger and releases them at turn end or on Escape abort, so
/// nothing stays stuck on the user's physical keyboard.
pub(crate) fn new_enigo_holding() -> Result<Enigo> {
    Enigo::new(&Settings {
        release_keys_when_dropped: false,
        ..Settings::default()
    })
    .map_err(|e| DesktopError::InputFailed(format!("Failed to create Enigo instance: {e}")))
}

/// Move the pointer to a global screen pixel coordinate.
///
/// Every mouse verb funnels through here so exactly one piece of code decides
/// what an absolute coordinate means on each platform.
///
/// On Windows that is **not** enigo: its backend normalizes an absolute move
/// against the primary monitor and omits `MOUSEEVENTF_VIRTUALDESK`, so a point
/// on any other display was clamped onto the primary one. See
/// [`crate::win_input`]. Everywhere else enigo's absolute move is already the
/// whole-desktop space and is used unchanged.
///
/// `context` names the movement for the error message ("mouse", "to start",
/// "during drag", "to end").
pub(crate) fn move_pointer(enigo: &mut Enigo, x: i32, y: i32, context: &str) -> Result<()> {
    #[cfg(windows)]
    {
        // Buttons still go through `enigo`; they act at the current pointer
        // position and carry no coordinate of their own.
        let _ = enigo;
        crate::win_input::move_cursor_absolute(x, y)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to move {context}: {e}")))
    }
    #[cfg(not(windows))]
    {
        use enigo::Mouse as _;
        enigo
            .move_mouse(x, y, enigo::Coordinate::Abs)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to move {context}: {e}")))
    }
}

/// Fewest intermediate positions a drag passes through, and the wall clock they
/// are spread over.
///
/// A drag with no requested duration used to be press-at-A, one absolute move,
/// release-at-B — on **both** rails (enigo/XTEST and ydotool). That is not a
/// drag to a toolkit: GTK, Qt and Chromium all arm drag-and-drop on *motion
/// while the button is down*, and require the pointer to travel past a small
/// threshold before they begin. A single teleport reads as a click at A followed
/// by a stray release at B, so the gesture silently does nothing while every
/// layer reports success. orca's Linux runtime interpolates unconditionally for
/// exactly this reason; so does UI-TARS' operator.
///
/// The floor is deliberately small: 12 steps over 120ms is imperceptible next to
/// the round trip that requested the drag, and an explicit `duration_ms` still
/// wins whenever it asks for more.
const MIN_DRAG_STEPS: u64 = 12;
const MIN_DRAG_MS: u64 = 120;

/// Longest a drag may hold a worker thread, however long the caller asked for.
const MAX_DRAG_MS: u64 = 10_000;

/// Most intermediate positions a drag will emit — 10 seconds at 60fps.
const MAX_DRAG_STEPS: u64 = 600;

/// The intermediate positions a drag travels through, and how long to wait
/// between them.
///
/// The path is eased (cubic ease-out) so it looks like a hand rather than a
/// metronome — some drag targets sample velocity — and always **ends exactly on
/// the endpoint**, because the eased curve only reaches it to within the integer
/// cast.
///
/// Pure, so the floors and caps are pinned by tests rather than by watching a
/// pointer move.
#[must_use]
pub(crate) fn drag_path(
    sx: i32,
    sy: i32,
    ex: i32,
    ey: i32,
    duration_ms: Option<u64>,
) -> (Vec<(i32, i32)>, std::time::Duration) {
    let ms = duration_ms
        .unwrap_or(0)
        .clamp(MIN_DRAG_MS, MAX_DRAG_MS.max(MIN_DRAG_MS));
    let steps = (((ms as f64 / 1000.0) * 60.0).ceil() as u64).clamp(MIN_DRAG_STEPS, MAX_DRAG_STEPS);

    let mut path = Vec::with_capacity(steps as usize);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let eased = 1.0 - (1.0 - t).powi(3);
        let cx = (f64::from(ex) - f64::from(sx)).mul_add(eased, f64::from(sx));
        let cy = (f64::from(ey) - f64::from(sy)).mul_add(eased, f64::from(sy));
        path.push((cx as i32, cy as i32));
    }
    // Land on the requested endpoint exactly; the release must happen where the
    // caller asked, not one pixel short of it.
    if path.last() != Some(&(ex, ey)) {
        path.push((ex, ey));
    }

    (path, std::time::Duration::from_millis(ms / steps.max(1)))
}

/// Convert Aleph's `MouseButton` to enigo's Button.
pub(crate) const fn to_enigo_button(button: MouseButton) -> Button {
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

    // ── drag_path: a drag has to actually move ──────────────────────────────

    #[test]
    fn a_drag_with_no_duration_still_travels_through_intermediate_points() {
        // The regression this pins: press at A, one absolute move, release at B
        // is a click and a stray release to GTK/Qt/Chromium, and every layer
        // reported it as a successful drag.
        let (path, _) = drag_path(0, 0, 300, 200, None);
        assert!(
            path.len() >= MIN_DRAG_STEPS as usize,
            "a drag must move: {} points",
            path.len()
        );
        assert_ne!(path[0], (300, 200), "the first move is not the endpoint");
    }

    #[test]
    fn a_drag_always_lands_exactly_on_the_endpoint() {
        // The eased curve reaches t == 1, but only to within the integer cast —
        // and the button comes up wherever the last move left it.
        for (ex, ey) in [(300, 200), (-40, 17), (1, 1), (0, 0)] {
            let (path, _) = drag_path(0, 0, ex, ey, None);
            assert_eq!(*path.last().unwrap(), (ex, ey), "end ({ex},{ey})");
        }
    }

    #[test]
    fn an_explicit_duration_wins_when_it_asks_for_more() {
        let (short, _) = drag_path(0, 0, 100, 100, None);
        let (long, _) = drag_path(0, 0, 100, 100, Some(1_000));
        assert!(
            long.len() > short.len(),
            "1s should interpolate more finely than the floor: {} vs {}",
            long.len(),
            short.len()
        );
    }

    #[test]
    fn a_drag_cannot_pin_a_worker_thread_indefinitely() {
        // Both bounds matter and neither implies the other: the step cap bounds
        // iterations, the duration cap bounds wall clock.
        let (path, delay) = drag_path(0, 0, 100, 100, Some(u64::MAX));
        assert!(path.len() <= MAX_DRAG_STEPS as usize + 1);
        let total = delay.as_millis() as u64 * path.len() as u64;
        assert!(total <= MAX_DRAG_MS * 2, "total {total}ms");
    }

    #[test]
    fn a_zero_length_drag_is_still_a_bounded_path() {
        // Same start and end: nothing to interpolate, but the loop must not
        // spin and the release must still happen at the point asked for.
        let (path, _) = drag_path(50, 50, 50, 50, None);
        assert!(!path.is_empty());
        assert!(path.iter().all(|p| *p == (50, 50)));
    }

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
