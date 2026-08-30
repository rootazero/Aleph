//! Browser `KeyboardEvent.key` → the bytes a PTY expects.
//!
//! Deliberately the "normal" (not application) cursor-key form and
//! meta-sends-escape for Alt: those are what an unconfigured shell assumes,
//! and the alternatives are negotiated by the application via modes this
//! client does not yet track.

/// `None` means "send nothing" — a modifier-only keydown, or a key we do not
/// claim. Returning the key's *name* instead would type "F13" into the shell.
#[must_use]
pub fn encode_key(key: &str, ctrl: bool, alt: bool, _shift: bool) -> Option<Vec<u8>> {
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
            let c = k.chars().next()?;
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
        _ => return None,
    };
    if alt {
        let mut out = vec![0x1b];
        out.extend_from_slice(&base);
        return Some(out);
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_keys_are_utf8_bytes() {
        assert_eq!(encode_key("a", false, false, false), Some(b"a".to_vec()));
        assert_eq!(encode_key("A", false, false, true), Some(b"A".to_vec()));
        assert_eq!(
            encode_key("中", false, false, false),
            Some("中".as_bytes().to_vec())
        );
    }

    #[test]
    fn named_keys_map_to_their_control_bytes() {
        assert_eq!(
            encode_key("Enter", false, false, false),
            Some(b"\r".to_vec())
        );
        assert_eq!(encode_key("Tab", false, false, false), Some(b"\t".to_vec()));
        assert_eq!(
            encode_key("Backspace", false, false, false),
            Some(vec![0x7f])
        );
        assert_eq!(encode_key("Escape", false, false, false), Some(vec![0x1b]));
    }

    #[test]
    fn arrows_use_the_normal_cursor_key_form() {
        assert_eq!(
            encode_key("ArrowUp", false, false, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key("ArrowDown", false, false, false),
            Some(b"\x1b[B".to_vec())
        );
        assert_eq!(
            encode_key("ArrowRight", false, false, false),
            Some(b"\x1b[C".to_vec())
        );
        assert_eq!(
            encode_key("ArrowLeft", false, false, false),
            Some(b"\x1b[D".to_vec())
        );
    }

    /// Ctrl-C is the single most important key in a terminal. Getting the
    /// arithmetic wrong here means the user cannot stop a runaway process.
    #[test]
    fn ctrl_letters_become_c0_controls() {
        assert_eq!(encode_key("c", true, false, false), Some(vec![0x03]));
        assert_eq!(encode_key("C", true, false, true), Some(vec![0x03]));
        assert_eq!(encode_key("d", true, false, false), Some(vec![0x04]));
        assert_eq!(encode_key("a", true, false, false), Some(vec![0x01]));
    }

    /// Alt is ESC-prefix (meta-sends-escape), the default every shell assumes.
    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(encode_key("b", false, true, false), Some(vec![0x1b, b'b']));
    }

    /// Modifier-only keydowns must send nothing, or holding Shift types
    /// garbage into the shell.
    #[test]
    fn modifier_only_keys_send_nothing() {
        for k in ["Shift", "Control", "Alt", "Meta", "CapsLock"] {
            assert_eq!(
                encode_key(k, false, false, false),
                None,
                "{k} must send nothing"
            );
        }
    }

    /// Browser shortcuts we deliberately do not swallow.
    #[test]
    fn unknown_named_keys_send_nothing_rather_than_their_name() {
        assert_eq!(encode_key("F13", false, false, false), None);
        assert_eq!(encode_key("BrowserBack", false, false, false), None);
    }
}
