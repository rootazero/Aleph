//! Unicode/invisible character sanitization for shell command security.
//!
//! Detects and strips invisible, zero-width, confusable, and dangerous Unicode characters
//! that could be used to disguise malicious commands from human reviewers.
//!
//! The invisible / directional-formatting / tag-character classification is
//! shared with the external-content sanitizer via
//! [`crate::security::unicode_guard`]; this module layers shell-specific
//! confusable-homoglyph handling on top.

use crate::security::unicode_guard::is_invisible_char;

/// Returns true if the character is a confusable homoglyph that could disguise
/// ASCII letters in shell commands (e.g., Cyrillic 'а' looks like ASCII 'a').
const fn is_confusable_homoglyph(c: char) -> bool {
    matches!(
        c,
        // Cyrillic letters that look like ASCII (homoglyph attack)
        '\u{0430}' // а — Cyrillic small letter a
        | '\u{0435}' // е — Cyrillic small letter ie
        | '\u{043E}' // о — Cyrillic small letter o
        | '\u{0440}' // р — Cyrillic small letter er
        | '\u{0441}' // с — Cyrillic small letter es
        | '\u{0445}' // х — Cyrillic small letter ha
        | '\u{0443}' // у — Cyrillic small letter u
        | '\u{043A}' // к — Cyrillic small letter ka
        | '\u{0412}' // В — Cyrillic capital letter Ve
        | '\u{041D}' // Н — Cyrillic capital letter En
        | '\u{0420}' // Р — Cyrillic capital letter Er
        | '\u{0421}' // С — Cyrillic capital letter Es
        | '\u{0422}' // Т — Cyrillic capital letter Te
        | '\u{0425}' // Х — Cyrillic capital letter Ha
        // Greek letters that look like ASCII
        | '\u{03B1}' // α — Greek small letter alpha
        | '\u{03B5}' // ε — Greek small letter epsilon
        | '\u{03BF}' // ο — Greek small letter omicron
        | '\u{03C1}' // ρ — Greek small letter rho
        | '\u{03C3}' // σ — Greek small letter sigma
        | '\u{03C4}' // τ — Greek small letter tau
        | '\u{0391}' // Α — Greek capital letter Alpha
        | '\u{0395}' // Ε — Greek capital letter Epsilon
        | '\u{039F}' // Ο — Greek capital letter Omicron
        | '\u{03A1}' // Ρ — Greek capital letter Rho
        | '\u{03A3}' // Σ — Greek capital letter Sigma
        | '\u{03A4}' // Τ — Greek capital letter Tau
    )
}

/// Returns true if the text contains any confusable homoglyph characters.
pub fn has_confusable_chars(text: &str) -> bool {
    text.chars().any(is_confusable_homoglyph)
}

/// Returns true if the text contains any invisible or dangerous Unicode characters.
pub fn has_invisible_chars(text: &str) -> bool {
    text.chars().any(is_invisible_char)
}

/// Returns a copy of the text with all invisible, dangerous, and confusable
/// Unicode characters removed.
#[must_use]
pub fn sanitize_display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !is_invisible_char(*c) && !is_confusable_homoglyph(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_text_no_invisible() {
        assert!(!has_invisible_chars("ls -la"));
        assert!(!has_invisible_chars("echo hello world"));
        assert!(!has_invisible_chars("rm -rf ./temp"));
    }

    #[test]
    fn test_detects_zwsp() {
        assert!(has_invisible_chars("ls\u{200B} -la"));
    }

    #[test]
    fn test_detects_bidi_override() {
        assert!(has_invisible_chars("safe\u{202E}command"));
    }

    #[test]
    fn test_detects_hangul_filler() {
        assert!(has_invisible_chars("rm\u{3164}file"));
    }

    #[test]
    fn test_sanitize_strips_invisible() {
        let result = sanitize_display_text("ls\u{200B}\u{200C}\u{200D} -la");
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn test_sanitize_preserves_cjk() {
        let input = "echo 你好世界";
        assert_eq!(sanitize_display_text(input), input);
    }

    #[test]
    fn test_sanitize_preserves_emoji() {
        let input = "echo 🚀";
        assert_eq!(sanitize_display_text(input), input);
    }

    #[test]
    fn test_detects_variation_selector() {
        assert!(has_invisible_chars("test\u{FE0F}text"));
    }

    #[test]
    fn test_bidi_attack_sanitized() {
        let result = sanitize_display_text("cat /etc/\u{202E}dwssap");
        assert_eq!(result, "cat /etc/dwssap");
    }

    #[test]
    fn test_detects_cyrillic_homoglyph() {
        assert!(has_confusable_chars("sudo\u{0430} rm -rf /"));
    }

    #[test]
    fn test_detects_greek_homoglyph() {
        assert!(has_confusable_chars("\u{03C1}m -rf /"));
    }

    #[test]
    fn test_sanitize_strips_cyrillic_homoglyph() {
        let result = sanitize_display_text("sudo\u{0430} rm -rf /");
        assert_eq!(result, "sudo rm -rf /");
    }

    #[test]
    fn test_sanitize_strips_greek_homoglyph() {
        let result = sanitize_display_text("\u{03C1}m -rf /");
        assert_eq!(result, "m -rf /");
    }

    #[test]
    fn test_preserves_normal_ascii() {
        let input = "sudo rm -rf /";
        assert!(!has_confusable_chars(input));
        assert_eq!(sanitize_display_text(input), input);
    }
}
