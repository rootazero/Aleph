//! Key and modifier name parsing for enigo.

use enigo::Key;

use crate::error::{DesktopError, Result};

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
    // Single character keys (use char count, not byte length, for Unicode safety)
    if name.chars().count() == 1 {
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
    use crate::error::DesktopError;

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

    #[test]
    fn test_parse_key_multibyte_unicode_not_single_char() {
        // A Chinese character is 3 bytes in UTF-8 but 1 char — should map to Unicode key.
        assert_eq!(parse_key("\u{4e2d}").unwrap(), Key::Unicode('\u{4e2d}'));
    }

    #[test]
    fn test_parse_key_multibyte_string_rejected() {
        // Two Chinese characters — should NOT be treated as single char.
        let err = parse_key("\u{4e2d}\u{6587}").unwrap_err();
        assert!(matches!(err, DesktopError::InputFailed(_)));
    }
}
