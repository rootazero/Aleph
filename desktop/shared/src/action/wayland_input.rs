//! Wayland input fallback via `ydotool` (kernel `uinput`).
//!
//! `enigo`/XTEST cannot inject input under Wayland: the compositor blocks
//! synthetic pointer and key events for unprivileged clients, so the existing
//! `enigo` path silently no-ops (or fails to construct) on a pure Wayland
//! session. `ydotool` writes directly to the kernel `uinput` device, which
//! sits below the display server, so it is unaffected by the Wayland gate.
//!
//! This module is a *fallback*, not a replacement: it is preferred only on
//! Wayland sessions where the `ydotool` client is installed. X11, macOS, and
//! Windows keep the unchanged `enigo` path. nut.js — the engine the reference
//! `UI-TARS-desktop` project relies on — has no Wayland path at all, so this
//! is a genuine capability gain over the reference implementation.
//!
//! The argv-building and key-mapping logic is pure and host-testable on any
//! platform; only the process-spawning executor and session probe are gated to
//! `target_os = "linux"`.

use crate::error::{DesktopError, Result};
use crate::{MouseButton, PressAction};

// ── Pure argv / keycode logic (host-testable on every platform) ──────────────

/// ydotool button code layout: the low nibble selects the button
/// (0 = left, 1 = right, 2 = middle), `0x40` adds a press, `0x80` adds a
/// release; a full click sets both (`0xC0`).
pub(crate) const fn click_code(button: MouseButton, action: PressAction) -> u8 {
    let idx: u8 = match button {
        MouseButton::Left => 0x00,
        MouseButton::Right => 0x01,
        MouseButton::Middle => 0x02,
    };
    match action {
        PressAction::Press => 0x40 | idx,
        PressAction::Release => 0x80 | idx,
        PressAction::Click => 0xC0 | idx,
    }
}

/// `ydotool mousemove --absolute -x X -y Y` — move the pointer to an absolute
/// screen position.
pub(crate) fn mousemove_args(x: i32, y: i32) -> Vec<String> {
    vec![
        "mousemove".into(),
        "--absolute".into(),
        "-x".into(),
        x.to_string(),
        "-y".into(),
        y.to_string(),
    ]
}

/// `ydotool click 0xNN` — press/release/click the given button.
pub(crate) fn click_args(button: MouseButton, action: PressAction) -> Vec<String> {
    vec![
        "click".into(),
        format!("0x{:02X}", click_code(button, action)),
    ]
}

/// `ydotool type -- <text>` — type a literal string. The `--` stops option
/// parsing so text beginning with `-` is typed verbatim.
pub(crate) fn type_args(text: &str) -> Vec<String> {
    vec!["type".into(), "--".into(), text.to_string()]
}

/// `ydotool mousemove --wheel -x H -y V` — emit wheel clicks.
///
/// ydotool models the wheel as a relative pointer move on the wheel axes, so a
/// scroll is a `mousemove` with `--wheel`, not a `click`. The sign convention
/// matches `enigo`'s: positive Y is down, positive X is right.
///
/// `amount` is clamped by the caller (see `input::scroll`) before it gets here.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] for an unknown direction — the same vocabulary
/// and the same message as the enigo path, so the two rails cannot drift.
pub(crate) fn scroll_args(direction: &str, amount: i32) -> Result<Vec<String>> {
    let (x, y) = match direction {
        "down" => (0, amount),
        "up" => (0, amount.saturating_neg()),
        "right" => (amount, 0),
        "left" => (amount.saturating_neg(), 0),
        other => {
            return Err(DesktopError::InputFailed(format!(
                "Unknown scroll direction: '{other}'. Expected up, down, left, or right"
            )));
        }
    };
    Ok(vec![
        "mousemove".into(),
        "--wheel".into(),
        "-x".into(),
        x.to_string(),
        "-y".into(),
        y.to_string(),
    ])
}

/// `ydotool key M:1 … K:1 K:0 … M:0` — press modifiers, tap the main key, then
/// release modifiers in reverse order, using Linux evdev keycodes.
///
/// # Errors
///
/// [`DesktopError::InputFailed`] if a modifier or the main key has no evdev
/// keycode mapping (e.g. an exotic punctuation key that would need a shifted
/// chord).
pub(crate) fn key_args(modifiers: &[String], key: &str) -> Result<Vec<String>> {
    let main = evdev_keycode(key).ok_or_else(|| {
        DesktopError::InputFailed(format!(
            "ydotool: no evdev keycode for key '{key}' (Wayland key combo)"
        ))
    })?;
    let mods = modifiers
        .iter()
        .map(|m| {
            evdev_modifier(m).ok_or_else(|| {
                DesktopError::InputFailed(format!(
                    "ydotool: no evdev keycode for modifier '{m}' (Wayland key combo)"
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut args = Vec::with_capacity(mods.len() * 2 + 3);
    args.push("key".into());
    for m in &mods {
        args.push(format!("{m}:1"));
    }
    args.push(format!("{main}:1"));
    args.push(format!("{main}:0"));
    for m in mods.iter().rev() {
        args.push(format!("{m}:0"));
    }
    Ok(args)
}

/// Build the `ydotool key` argv for holding (`Press`) or releasing
/// (`Release`) a set of keys without the paired counterpart — the Wayland
/// equivalent of the enigo `key_button` path. ydotool encodes key state as
/// `<code>:1` (down) / `<code>:0` (up).
///
/// On `Press` keys go down in order; on `Release` they come up in reverse so
/// nested holds unwind correctly; `Click` emits a full down-then-up.
pub(crate) fn key_button_args(keys: &[String], action: PressAction) -> Result<Vec<String>> {
    let codes = keys
        .iter()
        .map(|k| {
            evdev_keycode(k)
                .or_else(|| evdev_modifier(k))
                .ok_or_else(|| {
                    DesktopError::InputFailed(format!(
                        "ydotool: no evdev keycode for key '{k}' (Wayland press/release)"
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut args = Vec::with_capacity(codes.len() * 2 + 1);
    args.push("key".into());
    if matches!(action, PressAction::Press | PressAction::Click) {
        for c in &codes {
            args.push(format!("{c}:1"));
        }
    }
    if matches!(action, PressAction::Release | PressAction::Click) {
        for c in codes.iter().rev() {
            args.push(format!("{c}:0"));
        }
    }
    Ok(args)
}

/// Map a modifier name (same vocabulary as [`super::key_parse::parse_modifier`])
/// to its left-side evdev keycode.
pub(crate) fn evdev_modifier(name: &str) -> Option<u16> {
    match name.to_lowercase().as_str() {
        "meta" | "command" | "cmd" | "super" | "win" => Some(125), // KEY_LEFTMETA
        "shift" => Some(42),                                       // KEY_LEFTSHIFT
        "control" | "ctrl" => Some(29),                            // KEY_LEFTCTRL
        "alt" | "option" => Some(56),                              // KEY_LEFTALT
        _ => None,
    }
}

/// Map a key name (same vocabulary as [`super::key_parse::parse_key`]) to its
/// evdev keycode. Single ASCII letters/digits and the common named keys are
/// supported; shifted-punctuation chords are not.
pub(crate) fn evdev_keycode(name: &str) -> Option<u16> {
    if name.chars().count() == 1 {
        if let Some(code) = name.chars().next().and_then(ascii_char_keycode) {
            return Some(code);
        }
    }
    let code = match name.to_lowercase().as_str() {
        "space" => 57,
        "return" | "enter" => 28,
        "tab" => 15,
        "escape" | "esc" => 1,
        "backspace" => 14,       // KEY_BACKSPACE (backward delete)
        "delete" | "del" => 111, // KEY_DELETE (forward delete; matches enigo Key::Delete)
        "up" | "uparrow" => 103,
        "down" | "downarrow" => 108,
        "left" | "leftarrow" => 105,
        "right" | "rightarrow" => 106,
        "home" => 102,
        "end" => 107,
        "pageup" => 104,
        "pagedown" => 109,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        _ => return None,
    };
    Some(code)
}

/// evdev keycodes for single ASCII letters, digits, and space.
const fn ascii_char_keycode(ch: char) -> Option<u16> {
    let code = match ch.to_ascii_lowercase() {
        'a' => 30,
        'b' => 48,
        'c' => 46,
        'd' => 32,
        'e' => 18,
        'f' => 33,
        'g' => 34,
        'h' => 35,
        'i' => 23,
        'j' => 36,
        'k' => 37,
        'l' => 38,
        'm' => 50,
        'n' => 49,
        'o' => 24,
        'p' => 25,
        'q' => 16,
        'r' => 19,
        's' => 31,
        't' => 20,
        'u' => 22,
        'v' => 47,
        'w' => 17,
        'x' => 45,
        'y' => 21,
        'z' => 44,
        '1' => 2,
        '2' => 3,
        '3' => 4,
        '4' => 5,
        '5' => 6,
        '6' => 7,
        '7' => 8,
        '8' => 9,
        '9' => 10,
        '0' => 11,
        ' ' => 57,
        _ => return None,
    };
    Some(code)
}

// ── Linux runtime: session probe + ydotool executor ──────────────────────────

/// Whether input should be routed through `ydotool` instead of `enigo`.
///
/// `Ok(true)` on a Wayland session with the `ydotool` client on `PATH`;
/// `Ok(false)` on X11, where `enigo`/XTEST is the right rail.
///
/// # Why this returns a `Result`
///
/// The third case — **Wayland with no `ydotool`** — used to return `false`,
/// which sent the caller down the `enigo` path. That path cannot work there:
/// the compositor discards synthetic events from unprivileged clients, so
/// `click` / `type_text` / `drag` / `hover` / `key_combo` / `mouse_button` /
/// `key_button` all did nothing and **reported success**. (The `scroll` verb was
/// noticed and fixed earlier for a different reason — it had no ydotool branch
/// at all — but the missing-tool half of the same hole was left in place for
/// every verb.)
///
/// A rail that cannot deliver has to say so. Refusing here costs a Wayland user
/// without `ydotool` nothing they actually had, and it stops the model from
/// building a plan on top of clicks that never landed.
///
/// Both facts come from `crate::linux::session`, the single source of truth.
/// This module used to carry its own copy of the session rules, which is how
/// the clipboard and the permission layer ended up with three subtly different
/// answers to "is this Wayland?".
///
/// # Errors
///
/// [`DesktopError::NotAvailable`] on a Wayland session with no `ydotool`.
#[cfg(target_os = "linux")]
pub(crate) fn should_use_ydotool() -> Result<bool> {
    pick_rail(
        crate::linux::session().kind.is_wayland(),
        crate::linux::tools().has("ydotool"),
    )
}

/// The rail decision as a pure function, so the three-way matrix is testable on
/// any host instead of only on whatever session the developer happens to be in.
///
/// # Errors
///
/// [`DesktopError::NotAvailable`] for Wayland-without-`ydotool`.
pub(crate) fn pick_rail(is_wayland: bool, has_ydotool: bool) -> Result<bool> {
    if !is_wayland {
        return Ok(false);
    }
    if has_ydotool {
        return Ok(true);
    }
    Err(DesktopError::NotAvailable(
        "This is a Wayland session, where the compositor discards synthetic input from \
         ordinary applications — the XTEST path silently does nothing. Install `ydotool` \
         (it injects through the kernel's uinput device, below the display server): \
         `sudo apt install ydotool && sudo systemctl enable --now ydotoold`, and make sure \
         your user can reach the ydotool socket. Meanwhile screenshots, OCR, the \
         accessibility tree and `ax_action` / `set_value` all still work — an AX action is \
         often a better route to the same result than a synthetic click."
            .into(),
    ))
}

/// Cap for one `ydotool` invocation.
///
/// `ydotool` talks to `ydotoold` over a unix socket; a daemon that is present
/// but wedged (a stale socket after a compositor restart is the common shape)
/// leaves the client blocked with no timeout of its own.
#[cfg(target_os = "linux")]
const YDOTOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(target_os = "linux")]
fn run_ydotool(args: &[String]) -> Result<()> {
    use crate::script_exec::{is_spawn_failure, output_capped_blocking};

    let mut cmd = std::process::Command::new("ydotool");
    cmd.args(args);
    let output =
        output_capped_blocking(cmd, YDOTOOL_TIMEOUT, "ydotool input injection").map_err(|e| {
            if is_spawn_failure(&e) {
                DesktopError::InputFailed(format!("Failed to spawn ydotool: {e}"))
            } else {
                e
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::InputFailed(format!(
            "ydotool exited with {} (is the ydotoold daemon running and ~/.ydotool_socket accessible?){}",
            output.status,
            match stderr.trim().lines().last() {
                Some(line) if !line.is_empty() => format!(": {line}"),
                _ => String::new(),
            }
        )));
    }
    Ok(())
}

// ── Linux input primitives mirroring `input.rs` signatures ───────────────────

#[cfg(target_os = "linux")]
pub(crate) fn click(x: i32, y: i32, button: MouseButton) -> Result<()> {
    run_ydotool(&mousemove_args(x, y))?;
    run_ydotool(&click_args(button, PressAction::Click))?;
    tracing::info!(x, y, button = ?button, "Click performed (ydotool/Wayland)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn double_click(x: i32, y: i32, button: MouseButton) -> Result<()> {
    run_ydotool(&mousemove_args(x, y))?;
    run_ydotool(&click_args(button, PressAction::Click))?;
    run_ydotool(&click_args(button, PressAction::Click))?;
    tracing::info!(x, y, button = ?button, "Double-click performed (ydotool/Wayland)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn hover(x: i32, y: i32) -> Result<()> {
    run_ydotool(&mousemove_args(x, y))?;
    tracing::info!(x, y, "Hover performed (ydotool/Wayland)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn mouse_button(x: i32, y: i32, button: MouseButton, action: PressAction) -> Result<()> {
    run_ydotool(&mousemove_args(x, y))?;
    run_ydotool(&click_args(button, action))?;
    tracing::info!(x, y, button = ?button, action = ?action, "Mouse button action performed (ydotool/Wayland)");
    Ok(())
}

/// Drag via press-at-start, move-to-end, release. The pointer jumps directly to
/// the end (ydotool `mousemove` is instantaneous); animated stepping would spawn
/// one subprocess per frame, so the fallback keeps it atomic.
#[cfg(target_os = "linux")]
pub(crate) fn drag(
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    duration_ms: Option<u64>,
) -> Result<()> {
    run_ydotool(&mousemove_args(start_x, start_y))?;
    run_ydotool(&click_args(MouseButton::Left, PressAction::Press))?;
    // Interpolated, not a single jump: a compositor delivers whatever motion it
    // is given, and the toolkit on the other side needs motion-while-held to arm
    // drag-and-drop at all. This rail used to teleport and say so in a comment
    // ("drag is atomic"), which described the implementation rather than what
    // the applications do with it. See `super::drag_path`.
    for (x, y) in super::drag_path(start_x, start_y, end_x, end_y, duration_ms).0 {
        run_ydotool(&mousemove_args(x, y))?;
    }
    run_ydotool(&click_args(MouseButton::Left, PressAction::Release))?;
    tracing::info!(
        start_x,
        start_y,
        end_x,
        end_y,
        "Drag performed (ydotool/Wayland)"
    );
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn type_text(text: &str) -> Result<()> {
    run_ydotool(&type_args(text))?;
    tracing::info!(chars = text.chars().count(), "Text typed (ydotool/Wayland)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn key_combo(modifiers: &[String], key: &str) -> Result<()> {
    let args = key_args(modifiers, key)?;
    run_ydotool(&args)?;
    tracing::info!(modifiers = ?modifiers, key = %key, "Key combo performed (ydotool/Wayland)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn key_button(keys: &[String], action: PressAction) -> Result<()> {
    let args = key_button_args(keys, action)?;
    run_ydotool(&args)?;
    tracing::info!(keys = ?keys, action = ?action, "Key button action performed (ydotool/Wayland)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn scroll(direction: &str, amount: i32) -> Result<()> {
    let args = scroll_args(direction, amount)?;
    run_ydotool(&args)?;
    tracing::info!(direction, amount, "Scroll performed (ydotool/Wayland)");
    Ok(())
}

// ── Tests (pure logic — run on every host) ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_keeps_the_enigo_rail() {
        assert!(!pick_rail(false, false).unwrap());
        assert!(!pick_rail(false, true).unwrap());
    }

    #[test]
    fn wayland_with_ydotool_takes_the_uinput_rail() {
        assert!(pick_rail(true, true).unwrap());
    }

    #[test]
    fn wayland_without_ydotool_refuses_instead_of_silently_doing_nothing() {
        // The bug this replaces: falling through to enigo/XTEST, which the
        // compositor discards, and reporting the no-op as a success.
        let err = pick_rail(true, false).expect_err("must not claim a rail it does not have");
        let msg = err.to_string();
        assert!(msg.contains("ydotool"), "must name the fix: {msg}");
        assert!(
            msg.contains("ydotoold"),
            "the daemon is half the install: {msg}"
        );
        // And it must leave the model a route that still works today.
        assert!(
            msg.contains("ax_action"),
            "must name a working alternative: {msg}"
        );
    }

    #[test]
    fn click_codes_match_ydotool_layout() {
        assert_eq!(click_code(MouseButton::Left, PressAction::Click), 0xC0);
        assert_eq!(click_code(MouseButton::Right, PressAction::Click), 0xC1);
        assert_eq!(click_code(MouseButton::Middle, PressAction::Click), 0xC2);
        assert_eq!(click_code(MouseButton::Left, PressAction::Press), 0x40);
        assert_eq!(click_code(MouseButton::Left, PressAction::Release), 0x80);
        assert_eq!(click_code(MouseButton::Right, PressAction::Press), 0x41);
        assert_eq!(click_code(MouseButton::Middle, PressAction::Release), 0x82);
    }

    #[test]
    fn mousemove_args_are_absolute() {
        assert_eq!(
            mousemove_args(640, 480),
            vec!["mousemove", "--absolute", "-x", "640", "-y", "480"]
        );
    }

    #[test]
    fn click_args_format_button_hex() {
        assert_eq!(
            click_args(MouseButton::Left, PressAction::Click),
            vec!["click", "0xC0"]
        );
        assert_eq!(
            click_args(MouseButton::Right, PressAction::Press),
            vec!["click", "0x41"]
        );
    }

    #[test]
    fn type_args_stop_option_parsing() {
        assert_eq!(type_args("-rf hello"), vec!["type", "--", "-rf hello"]);
    }

    #[test]
    fn key_args_ctrl_c_press_release_order() {
        // ctrl(29) down, c(46) down, c up, ctrl up
        let args = key_args(&["control".into()], "c").unwrap();
        assert_eq!(args, vec!["key", "29:1", "46:1", "46:0", "29:0"]);
    }

    #[test]
    fn key_args_multi_modifier_reverse_release() {
        // ctrl(29)+shift(42)+t(20): press in order, release in reverse.
        let args = key_args(&["ctrl".into(), "shift".into()], "t").unwrap();
        assert_eq!(
            args,
            vec!["key", "29:1", "42:1", "20:1", "20:0", "42:0", "29:0"]
        );
    }

    #[test]
    fn key_args_named_key() {
        let args = key_args(&["alt".into()], "f4").unwrap();
        assert_eq!(args, vec!["key", "56:1", "62:1", "62:0", "56:0"]);
    }

    #[test]
    fn key_args_rejects_unknown_key() {
        let err = key_args(&[], "§").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }

    #[test]
    fn key_button_args_press_only_holds_down() {
        // press 'a'(30): down only, no release.
        let args = key_button_args(&["a".into()], PressAction::Press).unwrap();
        assert_eq!(args, vec!["key", "30:1"]);
    }

    #[test]
    fn key_button_args_release_reverses_order() {
        // release ctrl(29)+shift(42): up in reverse order, no down.
        let args = key_button_args(&["ctrl".into(), "shift".into()], PressAction::Release).unwrap();
        assert_eq!(args, vec!["key", "42:0", "29:0"]);
    }

    #[test]
    fn key_button_args_click_is_full_cycle() {
        let args = key_button_args(&["shift".into()], PressAction::Click).unwrap();
        assert_eq!(args, vec!["key", "42:1", "42:0"]);
    }

    #[test]
    fn key_button_args_modifier_as_held_key() {
        // bare modifier resolves via evdev_modifier fallback (shift = 42).
        let args = key_button_args(&["shift".into()], PressAction::Press).unwrap();
        assert_eq!(args, vec!["key", "42:1"]);
    }

    #[test]
    fn key_args_rejects_unknown_modifier() {
        let err = key_args(&["hyper".into()], "c").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }

    #[test]
    fn evdev_modifier_aliases() {
        for m in ["meta", "command", "cmd", "super", "win"] {
            assert_eq!(evdev_modifier(m), Some(125), "modifier {m}");
        }
        assert_eq!(evdev_modifier("Ctrl"), Some(29));
        assert_eq!(evdev_modifier("ALT"), Some(56));
        assert_eq!(evdev_modifier("option"), Some(56));
        assert_eq!(evdev_modifier("capslock"), None);
    }

    #[test]
    fn scroll_args_use_the_wheel_axes_with_enigo_sign_convention() {
        assert_eq!(
            scroll_args("down", 3).unwrap(),
            vec!["mousemove", "--wheel", "-x", "0", "-y", "3"]
        );
        assert_eq!(
            scroll_args("up", 3).unwrap(),
            vec!["mousemove", "--wheel", "-x", "0", "-y", "-3"]
        );
        assert_eq!(
            scroll_args("right", 2).unwrap(),
            vec!["mousemove", "--wheel", "-x", "2", "-y", "0"]
        );
        assert_eq!(
            scroll_args("left", 2).unwrap(),
            vec!["mousemove", "--wheel", "-x", "-2", "-y", "0"]
        );
    }

    #[test]
    fn scroll_args_reject_an_unknown_direction_like_the_enigo_path() {
        let err = scroll_args("diagonal", 1).unwrap_err();
        assert!(err.to_string().contains("diagonal"), "{err}");
    }

    #[test]
    fn scroll_args_do_not_overflow_at_the_i32_boundary() {
        // `up` negates; a naive `-amount` would panic in debug on i32::MIN.
        assert!(scroll_args("up", i32::MIN).is_ok());
        assert!(scroll_args("left", i32::MIN).is_ok());
    }

    #[test]
    fn evdev_keycode_letters_case_insensitive() {
        assert_eq!(evdev_keycode("a"), Some(30));
        assert_eq!(evdev_keycode("A"), Some(30));
        assert_eq!(evdev_keycode("z"), Some(44));
        assert_eq!(evdev_keycode("0"), Some(11));
    }
}
