//! Mouse and keyboard input actions using enigo.

use enigo::{Axis, Direction, Keyboard, Mouse};
use tracing::info;

use super::{move_pointer, new_enigo, new_enigo_holding, to_enigo_button, validate_coordinate};
use crate::error::DesktopError;
use crate::error::Result;
use crate::MouseButton;

/// Maximum scroll clicks accepted from untrusted IPC. Prevents the Linux enigo
/// implementation from looping billions of times on a malicious `i32::MAX`.
const MAX_SCROLL_CLICKS: i32 = 10_000;

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

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::click(ix, iy, button);
    }

    let enigo_button = to_enigo_button(button);

    let mut enigo = new_enigo()?;

    move_pointer(&mut enigo, ix, iy, "mouse")?;

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
    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::type_text(text);
    }

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

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::key_combo(modifiers, key);
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

/// Press or release one or more keys *without* the paired counterpart, so a
/// key (or chord) can be held down across subsequent actions and released
/// later. This backs the UI-TARS `press` / `release` verbs, which differ from
/// [`key_combo`] (an atomic press-and-release).
///
/// `keys` may mix modifiers and regular keys; each token is parsed leniently
/// via [`super::key_parse::parse_key_or_modifier`]. On `Press` the keys go down
/// in order; on `Release` they come up in reverse order so nested holds unwind
/// correctly. `Click` performs a full press-and-release.
///
/// # Platform behavior of a cross-call hold
///
/// A held key only *composes* with input sent by a later call where the OS
/// derives modifier state from the global event stream:
///
/// - Windows (`SendInput`) and Linux (XTEST / ydotool): the key-down mutates
///   global keyboard state, so a subsequent [`type_text`] or [`click`] from a
///   fresh enigo instance really does see the modifier. A cross-call hold works.
/// - macOS: enigo builds every event from a `CGEventSourceStateID::Private`
///   source and stamps it with flags accumulated **inside that one instance**,
///   which is dropped when this function returns. A later call therefore posts
///   its event with *empty* flags, and the held modifier does not apply. The key
///   is genuinely down at the HID layer (and must still be released — see below),
///   but `press("cmd")` followed by `type_text("c")` will not produce Cmd+C.
///   Use [`key_combo`] for chords on macOS; it presses and releases inside a
///   single instance, so the flags do compose.
///
/// Because the enigo instance here is constructed with
/// `release_keys_when_dropped: false`, a `Press` leaves the key physically down
/// after this call returns — which is the whole point, and also why the caller
/// must own the release. The desktop tool records every press in its held-input
/// ledger and drains it at turn end / on Escape abort.
///
/// # Errors
///
/// - [`DesktopError::InputFailed`] if `keys` is empty, a key name is invalid,
///   enigo cannot be created, or the key operation fails.
pub fn key_button(keys: &[String], action: crate::PressAction) -> Result<()> {
    if keys.is_empty() {
        return Err(DesktopError::InputFailed(
            "key_button requires at least one key".into(),
        ));
    }

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::key_button(keys, action);
    }

    let parsed = keys
        .iter()
        .map(|k| super::key_parse::parse_key_or_modifier(k))
        .collect::<Result<Vec<_>>>()?;

    // A self-releasing instance would undo a `Press` the instant this function
    // returns, making the press/release split a no-op.
    let mut enigo = match action {
        crate::PressAction::Click => new_enigo()?,
        crate::PressAction::Press | crate::PressAction::Release => new_enigo_holding()?,
    };

    let mut press = |dir: Direction, reverse: bool| -> Result<()> {
        if reverse {
            for key in parsed.iter().rev() {
                enigo
                    .key(*key, dir)
                    .map_err(|e| DesktopError::InputFailed(format!("Failed key {dir:?}: {e}")))?;
            }
        } else {
            for key in &parsed {
                enigo
                    .key(*key, dir)
                    .map_err(|e| DesktopError::InputFailed(format!("Failed key {dir:?}: {e}")))?;
            }
        }
        Ok(())
    };

    match action {
        crate::PressAction::Press => press(Direction::Press, false)?,
        crate::PressAction::Release => press(Direction::Release, true)?,
        crate::PressAction::Click => {
            press(Direction::Press, false)?;
            press(Direction::Release, true)?;
        }
    }

    info!(keys = ?keys, action = ?action, "Key button action performed");
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
    // `amount` arrives from untrusted IPC; clamp it before negation so the
    // Linux enigo backend cannot be asked to emit billions of click events.
    let amount = amount.clamp(-MAX_SCROLL_CLICKS, MAX_SCROLL_CLICKS);

    // Validate the vocabulary *before* choosing a rail. An unknown direction is
    // the caller's mistake on every platform, and answering it with "install
    // ydotool" would send the model off to fix the wrong thing.
    //
    // `amount` is documented as positive, but negating `i32::MIN` would overflow
    // (panic in debug). `saturating_neg` is identical for all valid positive
    // inputs and keeps the helper panic-free at the boundary (P7).
    let (axis, length) = match direction {
        "down" => (Axis::Vertical, amount),
        "up" => (Axis::Vertical, amount.saturating_neg()),
        "right" => (Axis::Horizontal, amount),
        "left" => (Axis::Horizontal, amount.saturating_neg()),
        other => {
            return Err(DesktopError::InputFailed(format!(
                "Unknown scroll direction: '{other}'. Expected up, down, left, or right"
            )));
        }
    };

    // Every other input verb already had this branch; scroll did not, so on a
    // Wayland session it fell through to enigo/XTEST — which the compositor
    // blocks — and reported the resulting no-op as a success.
    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::scroll(direction, amount);
    }

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

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::double_click(ix, iy, button);
    }

    let btn = to_enigo_button(button);
    let mut enigo = new_enigo()?;
    move_pointer(&mut enigo, ix, iy, "mouse")?;
    enigo
        .button(btn, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to click: {e}")))?;
    enigo
        .button(btn, Direction::Click)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to double-click: {e}")))?;
    info!(x, y, button = ?button, "Double-click performed");
    Ok(())
}

/// Drag from (`start_x`, `start_y`) to (`end_x`, `end_y`) with optional animation.
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

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        let _ = duration_ms; // ydotool mousemove is instantaneous; drag is atomic.
        return super::wayland_input::drag(sx, sy, ex, ey);
    }

    let mut enigo = new_enigo()?;

    move_pointer(&mut enigo, sx, sy, "to start")?;
    enigo
        .button(enigo::Button::Left, Direction::Press)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to press for drag: {e}")))?;

    match duration_ms {
        Some(ms) if ms > 0 => {
            // Cap raw duration at 10s so an untrusted caller cannot pin a
            // worker thread for arbitrary time despite the step cap. The
            // 600-step cap already bounds iterations; this bounds wall
            // clock. Above the ceiling, fall back to an instantaneous drag
            // (the Some(ms) arm becoming effectively `_ =>`).
            let ms = ms.min(10_000);
            // Cap at 600 steps (10 seconds at 60fps) to prevent excessive iteration
            // from malicious or erroneous duration values.
            let steps = ((ms as f64 / 1000.0) * 60.0).ceil().clamp(1.0, 600.0) as u64;
            let step_delay = Duration::from_millis(ms / steps.max(1));
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let eased = 1.0 - (1.0 - t).powi(3);
                let cx = (f64::from(ex) - f64::from(sx)).mul_add(eased, f64::from(sx));
                let cy = (f64::from(ey) - f64::from(sy)).mul_add(eased, f64::from(sy));
                move_pointer(&mut enigo, cx as i32, cy as i32, "during drag")?;
                std::thread::sleep(step_delay);
            }
        }
        _ => {
            move_pointer(&mut enigo, ex, ey, "to end")?;
        }
    }

    enigo
        .button(enigo::Button::Left, Direction::Release)
        .map_err(|e| DesktopError::InputFailed(format!("Failed to release after drag: {e}")))?;

    info!(
        start_x,
        start_y,
        end_x,
        end_y,
        ?duration_ms,
        "Drag performed"
    );
    Ok(())
}

/// Move mouse to (x, y) without clicking.
pub fn hover(x: f64, y: f64) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::hover(ix, iy);
    }

    let mut enigo = new_enigo()?;
    move_pointer(&mut enigo, ix, iy, "mouse")?;
    info!(x, y, "Hover performed");
    Ok(())
}

/// Get current mouse cursor position.
pub fn cursor_position() -> Result<(f64, f64)> {
    // Wayland has no pointer-position query for ordinary clients (the protocol
    // deliberately withholds the global cursor), and ydotool is write-only. The
    // enigo path would fail here with an opaque construction error, so say what
    // is actually true and what to do instead.
    #[cfg(target_os = "linux")]
    if crate::linux::session().kind.is_wayland() {
        return Err(DesktopError::NotImplemented(
            "Wayland does not expose the pointer position to applications. Aleph's input actions \
             take explicit coordinates, so locate the target with a screenshot (desktop_som / \
             gui_locate) and click that point instead of reading the cursor."
                .into(),
        ));
    }

    let enigo = new_enigo()?;
    let (x, y) = enigo
        .location()
        .map_err(|e| DesktopError::InputFailed(format!("Failed to get cursor position: {e}")))?;
    Ok((f64::from(x), f64::from(y)))
}

/// Press, release, or click a mouse button at (x, y).
///
/// Unlike a held *key* (see [`key_button`]), a held mouse button does not depend
/// on enigo's per-instance modifier flags, so a cross-call hold — press at A,
/// move, release at B — composes correctly on every platform. The caller owns
/// the release: this instance does not self-release on drop.
pub fn mouse_button(x: f64, y: f64, button: MouseButton, action: crate::PressAction) -> Result<()> {
    let ix = validate_coordinate(x, "x")?;
    let iy = validate_coordinate(y, "y")?;

    #[cfg(target_os = "linux")]
    if super::wayland_input::should_use_ydotool()? {
        return super::wayland_input::mouse_button(ix, iy, button, action);
    }

    let btn = to_enigo_button(button);
    let dir = match action {
        crate::PressAction::Press => Direction::Press,
        crate::PressAction::Release => Direction::Release,
        crate::PressAction::Click => Direction::Click,
    };
    let mut enigo = match action {
        crate::PressAction::Click => new_enigo()?,
        crate::PressAction::Press | crate::PressAction::Release => new_enigo_holding()?,
    };
    move_pointer(&mut enigo, ix, iy, "mouse")?;
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
        crate::macos::clipboard::read_text()
    }

    #[cfg(target_os = "linux")]
    {
        // One Linux clipboard implementation, session-aware and exit-status
        // checked — see `crate::linux::clipboard`. This arm used to be a second,
        // X11-only copy that reported failures as an empty clipboard.
        crate::linux::clipboard::read_text()
    }

    #[cfg(target_os = "windows")]
    {
        use clipboard_win::{formats, get_clipboard};

        get_clipboard::<String, _>(formats::Unicode)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to read clipboard: {e}")))
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
        // This arm used to drop `setString:forType:`'s answer on the floor and
        // return `Ok(())` regardless — a refused write reported as a copy.
        crate::macos::clipboard::write_text(text)
    }

    #[cfg(target_os = "linux")]
    {
        crate::linux::clipboard::write_text(text)?;
        info!("Clipboard write performed");
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        use clipboard_win::{formats, set_clipboard};

        set_clipboard(formats::Unicode, text)
            .map_err(|e| DesktopError::InputFailed(format!("Failed to write clipboard: {e}")))?;
        info!("Clipboard write performed");
        Ok(())
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
