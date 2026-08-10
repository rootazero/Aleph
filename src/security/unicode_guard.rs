//! Canonical classification of invisible / dangerous Unicode characters.
//!
//! These code points carry no legitimate textual meaning in untrusted input
//! but enable a family of well-documented attacks:
//!
//! - **Trojan Source** (CVE-2021-42574): bidirectional override / isolate marks
//!   reorder how text renders to a human reviewer versus how a parser (or an
//!   LLM) ingests it.
//! - **ASCII smuggling**: deprecated Unicode *tag* characters (U+E0000 block)
//!   are invisible to humans yet decoded as instructions by some models.
//! - **Pattern-detector evasion**: zero-width characters split an injection
//!   keyword (`ig<ZWSP>nore previous instructions`) so substring scanners miss
//!   it while the model still reads the intended phrase.
//!
//! This module is the single source of truth for that classification. Both the
//! external-content sanitizer (`crate::security::content_sanitizer`) and the
//! sandbox command-output floor (`crate::sandbox::scrub::strip_unsafe_invisible`,
//! the byte-path twin) defer to it, so neither defense can drift from this
//! catalog.

/// Returns true if the character is an invisible or directional-formatting
/// character that has no place in untrusted input.
pub(crate) fn is_invisible_char(c: char) -> bool {
    matches!(
        c,
        '\u{00A0}' // NO-BREAK SPACE — looks like a space to a human, often
        // dropped by tokenizers, ideal for splitting injected keywords
        // without visible cost (review/hub-statics).
        | '\u{200B}' // ZERO WIDTH SPACE
        | '\u{200C}' // ZERO WIDTH NON-JOINER
        | '\u{200D}' // ZERO WIDTH JOINER
        | '\u{FEFF}' // BYTE ORDER MARK / ZERO WIDTH NO-BREAK SPACE
        | '\u{2060}' // WORD JOINER
        | '\u{2061}' // FUNCTION APPLICATION
        | '\u{2062}' // INVISIBLE TIMES
        | '\u{2063}' // INVISIBLE SEPARATOR
        | '\u{2064}' // INVISIBLE PLUS
        | '\u{3164}' // HANGUL FILLER
        | '\u{115F}' // HANGUL CHOSEONG FILLER
        | '\u{1160}' // HANGUL JUNGSEONG FILLER
        | '\u{200E}' // LEFT-TO-RIGHT MARK
        | '\u{200F}' // RIGHT-TO-LEFT MARK
        | '\u{202A}' // LEFT-TO-RIGHT EMBEDDING
        | '\u{202B}' // RIGHT-TO-LEFT EMBEDDING
        | '\u{202C}' // POP DIRECTIONAL FORMATTING
        | '\u{202D}' // LEFT-TO-RIGHT OVERRIDE
        | '\u{202E}' // RIGHT-TO-LEFT OVERRIDE
        | '\u{2066}' // LEFT-TO-RIGHT ISOLATE
        | '\u{2067}' // RIGHT-TO-LEFT ISOLATE
        | '\u{2068}' // FIRST STRONG ISOLATE
        | '\u{2069}' // POP DIRECTIONAL ISOLATE
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

/// Returns true if the character is a deprecated Unicode tag character
/// (U+E0001-U+E007F) — the ASCII-smuggling vector.
fn is_tag_character(c: char) -> bool {
    (0xE0001..=0xE007F).contains(&(c as u32))
}

/// Removes every invisible / dangerous Unicode character from `text`.
///
/// Returns `(cleaned_text, removed_count)`. The text is returned even when
/// nothing was removed so callers need not branch. Confusable homoglyphs are
/// intentionally *not* handled here: the shell path strips them while the
/// content path folds them to ASCII, so each consumer keeps its own homoglyph
/// policy — only the always-strip invisible class is shared.
pub(crate) fn strip_invisible_chars(text: &str) -> (String, usize) {
    let mut removed = 0usize;
    let cleaned: String = text
        .chars()
        .filter(|c| {
            if is_invisible_char(*c) {
                removed += 1;
                false
            } else {
                true
            }
        })
        .collect();
    (cleaned, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_zero_width_and_bidi_and_tag() {
        assert!(is_invisible_char('\u{200B}')); // ZWSP
        assert!(is_invisible_char('\u{202E}')); // RTL override
        assert!(is_invisible_char('\u{2066}')); // LTR isolate
        assert!(is_invisible_char('\u{E0041}')); // tag 'A'
    }

    #[test]
    fn preserves_legitimate_text() {
        assert!(!is_invisible_char('a'));
        assert!(!is_invisible_char('你'));
        assert!(!is_invisible_char('🚀'));
        assert!(!is_invisible_char(' '));
        assert!(!is_invisible_char('\n'));
    }

    #[test]
    fn detects_nbsp_as_invisible() {
        // NBSP is "invisible" for injection-detection purposes: it renders as
        // a space to a human reviewer and is often dropped by tokenizers, so it
        // is the standard invisible-space splitting vector.
        assert!(is_invisible_char('\u{00A0}'));
    }

    #[test]
    fn strip_counts_and_removes() {
        let (cleaned, n) = strip_invisible_chars("ls\u{200B}\u{202E} -la");
        assert_eq!(cleaned, "ls -la");
        assert_eq!(n, 2);
    }

    #[test]
    fn strip_is_noop_on_clean_text() {
        let (cleaned, n) = strip_invisible_chars("echo 你好 🚀");
        assert_eq!(cleaned, "echo 你好 🚀");
        assert_eq!(n, 0);
    }

    #[test]
    fn strip_removes_smuggled_tag_chars() {
        // "hi" with an invisible tag-encoded payload between the letters.
        let (cleaned, n) = strip_invisible_chars("h\u{E0070}\u{E0077}i");
        assert_eq!(cleaned, "hi");
        assert_eq!(n, 2);
    }
}
