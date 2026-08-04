//! Hydration helpers — token estimation.
//!
//! These are deliberately simple and dependency-free. A real tokenizer can
//! replace [`estimate_tokens`] in v1.1 without touching callers.
//!
//! `truncate_utf8_safe` used to live here. Its sole caller (`hybrid::hydrate`)
//! was passing a CHARACTER budget into its byte parameter, under-filling every
//! non-ASCII envelope ~3x; the caller now uses
//! [`crate::utils::text_format::truncate_chars`], which leaves this byte helper
//! with no production consumer, so it was withdrawn (R10) rather than left as a
//! second way to spell [`crate::utils::text_format::truncate_bytes`].

/// Estimate tokens using the existing 4-chars-per-token heuristic (matches
/// `ContextComptroller` behavior). Never returns zero for non-empty text.
#[must_use]
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
