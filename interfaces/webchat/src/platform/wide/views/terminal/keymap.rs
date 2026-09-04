//! Browser `KeyboardEvent.key` → the bytes a PTY expects.
//!
//! Deliberately the "normal" (not application) cursor-key form and
//! meta-sends-escape for Alt: those are what an unconfigured shell assumes,
//! and the alternatives are negotiated by the application via modes this
//! client does not yet track.

/// What the view should do with one keydown.
///
/// Two variants because there are exactly two behaviours at the call site, and
/// naming them here is what stops the paste rule from being spread across the
/// keymap and the view. It replaces an `Option<Vec<u8>>` whose `None` meant
/// "we do not claim this" — the same fact, said in a type that could not also
/// carry "and we are claiming NOT to claim it, on purpose".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    /// Send these bytes to the PTY, and `prevent_default()` so the browser
    /// does not also act on the keystroke.
    Bytes(Vec<u8>),
    /// Leave it entirely to the browser: send nothing, and do NOT
    /// `prevent_default()`.
    ///
    /// Covers two things that behave identically and must keep doing so: a
    /// key we do not claim (`F13`, `BrowserBack`, a bare modifier), and the
    /// platform's PASTE chord — which is deliberately not swallowed, because
    /// swallowing it is what stops the `paste` event from ever firing, and
    /// that event is the only place the clipboard's *text* is readable at all
    /// (a `keydown` cannot read the clipboard).
    Browser,
}

/// Map one keydown to [`KeyAction`].
///
/// Returning the key's *name* for an unrecognised key would type "F13" into
/// the shell, so anything unclaimed is [`KeyAction::Browser`].
///
/// # Paste
///
/// D11: `Ctrl-Shift-V` — the paste chord on Windows and Linux terminals —
/// used to arrive here as `key: "V", ctrl: true` and leave as `0x16`, so the
/// browser's `paste` event never fired and the platform's own paste shortcut
/// typed a control character instead. `Cmd-V` was handled, but by a `meta`
/// bail in the view rather than here, which left the rule in two places
/// (判据 §6). Both now answer `Browser`, from this one function.
///
/// **Plain `Ctrl-V` still sends `0x16`** and that is not an oversight: in a
/// terminal it is `quoted-insert`, and a shell user pressing it means it.
#[must_use]
pub fn encode_key(key: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> KeyAction {
    // The paste chords, before anything else claims their bytes.
    if meta {
        return KeyAction::Browser;
    }
    if ctrl && shift && key.eq_ignore_ascii_case("v") {
        return KeyAction::Browser;
    }

    let base: Vec<u8> = match key {
        "Enter" => vec![b'\r'],
        "Tab" => vec![b'\t'],
        "Backspace" => vec![0x7f],
        "Escape" => vec![0x1b],
        "ArrowUp" => b"\x1b[A".to_vec(),
        "ArrowDown" => b"\x1b[B".to_vec(),
        "ArrowRight" => b"\x1b[C".to_vec(),
        "ArrowLeft" => b"\x1b[D".to_vec(),
        "Home" => b"\x1b[H".to_vec(),
        "End" => b"\x1b[F".to_vec(),
        "PageUp" => b"\x1b[5~".to_vec(),
        "PageDown" => b"\x1b[6~".to_vec(),
        "Delete" => b"\x1b[3~".to_vec(),
        "Insert" => b"\x1b[2~".to_vec(),
        "F1" => b"\x1bOP".to_vec(),
        "F2" => b"\x1bOQ".to_vec(),
        "F3" => b"\x1bOR".to_vec(),
        "F4" => b"\x1bOS".to_vec(),
        "F5" => b"\x1b[15~".to_vec(),
        "F6" => b"\x1b[17~".to_vec(),
        "F7" => b"\x1b[18~".to_vec(),
        "F8" => b"\x1b[19~".to_vec(),
        "F9" => b"\x1b[20~".to_vec(),
        "F10" => b"\x1b[21~".to_vec(),
        "F11" => b"\x1b[23~".to_vec(),
        "F12" => b"\x1b[24~".to_vec(),
        // A single printable character (any script). Anything longer is a
        // named key we do not claim.
        k if k.chars().count() == 1 => {
            // `let ... else`, not `?`: this function no longer returns an
            // `Option`, and the guard above already guarantees the `Some`.
            let Some(c) = k.chars().next() else {
                return KeyAction::Browser;
            };
            if ctrl {
                // Ctrl-<letter> is the letter's position in the alphabet.
                let upper = c.to_ascii_uppercase();
                if upper.is_ascii_uppercase() {
                    vec![(upper as u8) - b'A' + 1]
                } else {
                    match c {
                        '[' => vec![0x1b],
                        '\\' => vec![0x1c],
                        ']' => vec![0x1d],
                        ' ' => vec![0x00],
                        _ => c.to_string().into_bytes(),
                    }
                }
            } else {
                c.to_string().into_bytes()
            }
        }
        _ => return KeyAction::Browser,
    };
    if alt {
        let mut out = vec![0x1b];
        out.extend_from_slice(&base);
        return KeyAction::Bytes(out);
    }
    KeyAction::Bytes(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five-argument call the view makes, with no modifiers.
    fn plain(key: &str) -> KeyAction {
        encode_key(key, false, false, false, false)
    }

    fn bytes(key: &str, ctrl: bool, alt: bool, shift: bool) -> KeyAction {
        encode_key(key, ctrl, alt, shift, false)
    }

    #[test]
    fn printable_keys_are_utf8_bytes() {
        assert_eq!(plain("a"), KeyAction::Bytes(b"a".to_vec()));
        assert_eq!(
            bytes("A", false, false, true),
            KeyAction::Bytes(b"A".to_vec())
        );
        assert_eq!(plain("中"), KeyAction::Bytes("中".as_bytes().to_vec()));
    }

    #[test]
    fn named_keys_map_to_their_control_bytes() {
        assert_eq!(plain("Enter"), KeyAction::Bytes(b"\r".to_vec()));
        assert_eq!(plain("Tab"), KeyAction::Bytes(b"\t".to_vec()));
        assert_eq!(plain("Backspace"), KeyAction::Bytes(vec![0x7f]));
        assert_eq!(plain("Escape"), KeyAction::Bytes(vec![0x1b]));
    }

    #[test]
    fn arrows_use_the_normal_cursor_key_form() {
        assert_eq!(plain("ArrowUp"), KeyAction::Bytes(b"\x1b[A".to_vec()));
        assert_eq!(plain("ArrowDown"), KeyAction::Bytes(b"\x1b[B".to_vec()));
        assert_eq!(plain("ArrowRight"), KeyAction::Bytes(b"\x1b[C".to_vec()));
        assert_eq!(plain("ArrowLeft"), KeyAction::Bytes(b"\x1b[D".to_vec()));
    }

    /// Ctrl-C is the single most important key in a terminal. Getting the
    /// arithmetic wrong here means the user cannot stop a runaway process.
    #[test]
    fn ctrl_letters_become_c0_controls() {
        assert_eq!(bytes("c", true, false, false), KeyAction::Bytes(vec![0x03]));
        assert_eq!(bytes("C", true, false, true), KeyAction::Bytes(vec![0x03]));
        assert_eq!(bytes("d", true, false, false), KeyAction::Bytes(vec![0x04]));
        assert_eq!(bytes("a", true, false, false), KeyAction::Bytes(vec![0x01]));
    }

    /// D11. The two paste chords must reach the browser so its `paste` event
    /// fires — a `keydown` handler cannot read the clipboard, so swallowing
    /// them is not "handling paste badly", it is having no paste at all.
    ///
    /// Plain `Ctrl-V` keeps sending `0x16`: in a terminal that is
    /// `quoted-insert`, and it is what a shell user pressing it means.
    ///
    /// Reddens if: `Ctrl-Shift-V` falls through to the `ctrl` branch and
    /// becomes `0x16` again (the shipped behaviour before this change); if
    /// the meta bail is dropped or moved back into the view; or if plain
    /// `Ctrl-V` is "fixed" along with them, silently deleting quoted-insert.
    #[test]
    fn cmd_v_and_ctrl_shift_v_are_left_to_the_browser_ctrl_v_is_0x16() {
        // macOS: Cmd-V. The browser reports the letter unshifted.
        assert_eq!(
            encode_key("v", false, false, false, true),
            KeyAction::Browser
        );
        // Windows / Linux: Ctrl-Shift-V. With shift held the browser reports
        // the UPPERCASE letter, which is why this is matched case-insensitively.
        assert_eq!(
            encode_key("V", true, false, true, false),
            KeyAction::Browser
        );
        assert_eq!(
            encode_key("v", true, false, true, false),
            KeyAction::Browser
        );

        // ...and the one that must NOT change.
        assert_eq!(
            encode_key("v", true, false, false, false),
            KeyAction::Bytes(vec![0x16]),
            "plain Ctrl-V is quoted-insert and stays 0x16"
        );

        // A meta chord that is not paste is still the browser's (Cmd-C,
        // Cmd-Tab, Cmd-L): the view used to bail on `meta` before consulting
        // this function at all, and that behaviour moved here rather than
        // being narrowed to V.
        assert_eq!(
            encode_key("c", false, false, false, true),
            KeyAction::Browser
        );
        assert_eq!(
            encode_key("ArrowLeft", false, false, false, true),
            KeyAction::Browser
        );
    }

    /// Alt is ESC-prefix (meta-sends-escape), the default every shell assumes.
    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            bytes("b", false, true, false),
            KeyAction::Bytes(vec![0x1b, b'b'])
        );
    }

    /// Modifier-only keydowns must send nothing, or holding Shift types
    /// garbage into the shell.
    #[test]
    fn modifier_only_keys_send_nothing() {
        for k in ["Shift", "Control", "Alt", "Meta", "CapsLock"] {
            assert_eq!(plain(k), KeyAction::Browser, "{k} must send nothing");
        }
    }

    /// Browser shortcuts we deliberately do not swallow.
    #[test]
    fn unknown_named_keys_send_nothing_rather_than_their_name() {
        assert_eq!(plain("F13"), KeyAction::Browser);
        assert_eq!(plain("BrowserBack"), KeyAction::Browser);
    }
}
