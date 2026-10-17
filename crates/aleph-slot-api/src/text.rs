//! UTF-8 safe text helpers — `truncate_text` is the one moved out of
//! `alephcore::utils::text_format`. The other four truncate variants stay in
//! core because they encode different contracts (bytes vs chars, hard vs
//! soft cap, marker vs no marker) and the survey that motivated them
//! explicitly refused to fold them — see the module-level table in
//! `alephcore::utils::text_format`.
//!
//! Behavioural contract (pin verbatim):
//!   * `max_chars == 0` → `String::new()`, NOT `"..."`. The zero case is
//!     load-bearing for the `MissingInput::user_friendly_message` formatter,
//!     which feeds it `100` and would silently grow to `100 + 3` chars if we
//!     swapped a soft cap for a hard one.
//!   * under limit → `text.to_string()` (no marker).
//!   * at-or-over limit → `&text[..idx] + "..."` where `idx` is the byte
//!     offset of the `max_chars`th character. `char_indices` is the
//!     boundary-safe way to compute that without scanning twice.
//!
//! Identical body to the pre-migration `src/utils/text_format.rs::truncate_text`
//! (5 effective lines, no imports).

/// Truncate to at most `max_chars` CHARACTERS, appending `...` when cut.
///
/// Soft cap: result may be up to `max_chars + 3` chars (the marker is
/// appended, not reserved). `max_chars == 0` yields `""`, not `"..."`.
///
/// UTF-8 safe by construction: `char_indices` walks on codepoint boundaries,
/// and `&text[..idx]` is therefore a valid slice.
#[must_use]
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    match text.char_indices().nth(max_chars) {
        None => text.to_string(),
        Some((idx, _)) => format!("{}...", &text[..idx]),
    }
}

#[cfg(test)]
mod tests {
    //! These five unit tests mirror the same-named tests that used to live
    //! in `alephcore::utils::text_format::tests`. They travel with the
    //! function; the four sibling truncate tests (`truncate_with_marker`,
    //! `truncate_reserving`, `truncate_chars`, `truncate_bytes`) stay in
    //! core where their functions still live.

    use super::truncate_text;

    #[test]
    fn truncate_text_zero_limit_returns_empty_string() {
        assert_eq!(truncate_text("Hello world", 0), "");
    }

    #[test]
    fn truncate_text_under_limit_returns_input_unchanged() {
        assert_eq!(truncate_text("Hello world", 20), "Hello world");
    }

    #[test]
    fn truncate_text_over_limit_appends_marker() {
        let result = truncate_text("Hello world, this is a longer text", 10);
        assert!(result.ends_with("..."));
        // Soft cap is 10 chars + 3 bytes for "...".
        assert!(result.len() <= 13 + 3);
    }

    #[test]
    fn truncate_text_unicode_appends_marker() {
        // "你好世界，这是一段中文" — limit 5 cuts after "你" through "，"
        assert_eq!(truncate_text("你好世界，这是一段中文", 5), "你好世界，...");
    }

    #[test]
    fn truncate_text_limit_is_chars_not_bytes() {
        // 4 chars / 12 bytes — under a char-based `max = 8`, must NOT cut.
        let text = "你好世界";
        assert_eq!(truncate_text(text, 8), text);
        assert!(!truncate_text(text, 8).ends_with("..."));
        assert_eq!(truncate_text(text, 4), text);
    }
}
