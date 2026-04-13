//! Hydration helpers — UTF-8 safe truncation and token estimation.
//!
//! These are deliberately simple and dependency-free. A real tokenizer can
//! replace [`estimate_tokens`] in v1.1 without touching callers.

/// Truncate `s` to at most `max_bytes`, guaranteeing the result is valid UTF-8
/// by backing up to the nearest char boundary.
pub fn truncate_utf8_safe(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Estimate tokens using the existing 4-chars-per-token heuristic (matches
/// `ContextComptroller` behavior). Never returns zero for non-empty text.
pub fn estimate_tokens(s: &str) -> u32 {
    if s.is_empty() {
        return 0;
    }
    ((s.chars().count() as u32) / 4).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_within_limit_returns_original() {
        assert_eq!(truncate_utf8_safe("hello", 10), "hello");
    }

    #[test]
    fn truncate_over_limit_clips_to_boundary() {
        let out = truncate_utf8_safe("hello world", 5);
        assert_eq!(out, "hello");
    }

    #[test]
    fn truncate_respects_multibyte_char_boundary() {
        // "héllo" — é is 2 bytes in UTF-8.
        let s = "h\u{00e9}llo"; // 6 bytes total
        let out = truncate_utf8_safe(s, 2);
        // Byte 2 falls inside "é"; must back up to byte 1 ("h").
        assert_eq!(out, "h");
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn estimate_tokens_empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_tokens_short_is_one() {
        assert_eq!(estimate_tokens("ab"), 1);
    }

    #[test]
    fn estimate_tokens_scales_with_chars() {
        assert_eq!(estimate_tokens("a".repeat(400).as_str()), 100);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn truncate_always_valid_utf8_and_within_limit(
            s in "\\PC*",
            n in 0usize..256,
        ) {
            let out = truncate_utf8_safe(&s, n);
            prop_assert!(out.len() <= n);
            prop_assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        }
    }
}
