//! Unicode/invisible character sanitization for shell command security.
//!
//! Detects and strips invisible, zero-width, and confusable Unicode characters
//! that could be used to disguise malicious commands from human reviewers.

/// Returns true if the character is an invisible or potentially dangerous Unicode character.
fn is_invisible_char(c: char) -> bool {
    matches!(
        c,
        // Zero-width characters
        '\u{200B}' // ZERO WIDTH SPACE
        | '\u{200C}' // ZERO WIDTH NON-JOINER
        | '\u{200D}' // ZERO WIDTH JOINER
        | '\u{FEFF}' // BYTE ORDER MARK / ZERO WIDTH NO-BREAK SPACE
        // Word joiners and math invisible operators
        | '\u{2060}' // WORD JOINER
        | '\u{2061}' // FUNCTION APPLICATION (invisible math operator)
        | '\u{2062}' // INVISIBLE TIMES
        | '\u{2063}' // INVISIBLE SEPARATOR
        | '\u{2064}' // INVISIBLE PLUS
        // Hangul fillers
        | '\u{3164}' // HANGUL FILLER
        | '\u{115F}' // HANGUL CHOSEONG FILLER
        | '\u{1160}' // HANGUL JUNGSEONG FILLER
        // Bidi controls: LRM/RLM
        | '\u{200E}' // LEFT-TO-RIGHT MARK
        | '\u{200F}' // RIGHT-TO-LEFT MARK
        // Bidi embedding controls
        | '\u{202A}' // LEFT-TO-RIGHT EMBEDDING
        | '\u{202B}' // RIGHT-TO-LEFT EMBEDDING
        | '\u{202C}' // POP DIRECTIONAL FORMATTING
        | '\u{202D}' // LEFT-TO-RIGHT OVERRIDE
        | '\u{202E}' // RIGHT-TO-LEFT OVERRIDE
        // Bidi isolates
        | '\u{2066}' // LEFT-TO-RIGHT ISOLATE
        | '\u{2067}' // RIGHT-TO-LEFT ISOLATE
        | '\u{2068}' // FIRST STRONG ISOLATE
        | '\u{2069}' // POP DIRECTIONAL ISOLATE
        // Variation selectors
        | '\u{FE00}'
        | '\u{FE01}'
        | '\u{FE02}'
        | '\u{FE03}'
        | '\u{FE04}'
        | '\u{FE05}'
        | '\u{FE06}'
        | '\u{FE07}'
        | '\u{FE08}'
        | '\u{FE09}'
        | '\u{FE0A}'
        | '\u{FE0B}'
        | '\u{FE0C}'
        | '\u{FE0D}'
        | '\u{FE0E}'
        | '\u{FE0F}'
    ) || is_tag_character(c)
}

/// Returns true if the character is a deprecated Unicode tag character (U+E0001-U+E007F).
fn is_tag_character(c: char) -> bool {
    (0xE0001..=0xE007F).contains(&(c as u32))
}

/// Returns true if the text contains any invisible or dangerous Unicode characters.
pub fn has_invisible_chars(text: &str) -> bool {
    text.chars().any(is_invisible_char)
}

/// Returns a copy of the text with all invisible/dangerous Unicode characters removed.
pub fn sanitize_display_text(text: &str) -> String {
    text.chars().filter(|c| !is_invisible_char(*c)).collect()
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
}
