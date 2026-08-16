//! Shared text formatting utilities
//!
//! Common functions used by prompt assemblers across the codebase.

/// Truncate to at most `max_chars` CHARACTERS, appending `...` when cut.
///
/// Soft cap: the result may be up to `max_chars + 3` chars, because the marker
/// is appended rather than reserved. `max_chars == 0` yields `""`, not a bare
/// `"..."`.
///
/// # Before adding another truncate helper
///
/// A survey (2026-08-03) found **22** private truncate helpers in this crate.
/// They were NOT 22 copies of this function — they implemented **5 mutually
/// incompatible contracts**, which is why they could not all be folded into
/// this one. All 22 now delegate to this module; pick the matching row:
///
/// | Contract | Function |
/// |---|---|
/// | chars + `...` + soft cap → `String` | [`truncate_text`] (this one) |
/// | chars + any marker + soft cap → `String` | [`truncate_with_marker`] |
/// | chars, no marker, hard cap → `&str` | [`truncate_chars`] |
/// | chars + marker + **hard** cap (total ≤ max) | [`truncate_reserving`] |
/// | **bytes**, no marker, hard cap → `&str` | [`truncate_bytes`] |
///
/// The distinction is load-bearing, not stylistic: the soft-cap callers feed
/// prompt bytes governed by the §2.18 ratchet, and the hard-cap and byte-budget
/// callers enforce external API length limits. Swapping a soft cap for a hard
/// one (or chars for bytes) silently changes lengths on those paths, which is
/// exactly how `mutation_gate` shipped a byte-guard/char-cut bug — see
/// [`tests::truncate_text_limit_is_chars_not_bytes`].
///
/// So: reuse the row that matches. If none does, add a **named** variant here
/// rather than hand-rolling copy #23 — and do not "unify" these five without
/// checking each caller's budget.
#[must_use]
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    // Single-pass: find the byte offset of the (max_chars)th character
    match text.char_indices().nth(max_chars) {
        None => text.to_string(),                         // under limit
        Some((idx, _)) => format!("{}...", &text[..idx]), // truncate
    }
}

/// Truncate to at most `max_chars` CHARACTERS, appending `marker` when cut.
///
/// Soft cap, like [`truncate_text`] but with a caller-chosen marker: the result
/// may be up to `max_chars` chars plus the marker.
///
/// Unlike [`truncate_text`] there is deliberately **no `max_chars == 0` guard**.
/// At zero this returns the bare `marker`, which is what every caller migrated
/// onto this function already did. `prompt_build`'s `extra_files` genuinely
/// reaches zero (`per_file_max_chars.min(total_max_chars - total)` once the
/// budget is spent), so adding a guard here would be a silent behaviour change
/// on a live path rather than a cleanup.
#[must_use]
pub fn truncate_with_marker(text: &str, max_chars: usize, marker: &str) -> String {
    match text.char_indices().nth(max_chars) {
        None => text.to_string(),
        Some((idx, _)) => format!("{}{marker}", &text[..idx]),
    }
}

/// Truncate to at most `max_chars` CHARACTERS with no marker, borrowing.
///
/// Hard cap and zero-alloc — the result is always a prefix of `text` and never
/// longer than `max_chars` chars. Use when the caller must not pay an
/// allocation, or when a truncation marker would corrupt the value (an ID, a
/// field an external API length-checks).
#[must_use]
pub fn truncate_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        None => text,
        Some((idx, _)) => &text[..idx],
    }
}

/// Truncate so the result INCLUDING `marker` is at most `max_total_chars`.
///
/// Hard cap: unlike [`truncate_with_marker`], the marker's width is reserved
/// before cutting, so the return value never exceeds the budget. Use where the
/// number is a real limit (a fixed-width column, an external API cap) rather
/// than a hint. Saturates when `max_total_chars` is shorter than the marker.
#[must_use]
pub fn truncate_reserving(text: &str, max_total_chars: usize, marker: &str) -> String {
    if text.chars().count() <= max_total_chars {
        return text.to_string();
    }
    let keep = max_total_chars.saturating_sub(marker.chars().count());
    format!("{}{marker}", truncate_chars(text, keep))
}

/// Truncate to at most `max_bytes` BYTES, walking back to a char boundary.
///
/// The odd one out: the budget is bytes, not chars, because some limits really
/// are byte limits (WeChat's message API, an environment-variable cap). Never
/// splits a multi-byte codepoint — `&text[..max_bytes]` would panic instead.
#[must_use]
pub fn truncate_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_text_zero_limit() {
        let text = "Hello world";
        assert_eq!(truncate_text(text, 0), "");
    }

    #[test]
    fn test_truncate_text_under_limit() {
        let text = "Hello world";
        assert_eq!(truncate_text(text, 20), "Hello world");
    }

    #[test]
    fn test_truncate_text_over_limit() {
        let text = "Hello world, this is a longer text";
        let result = truncate_text(text, 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 13 + 3); // 10 chars + "..."
    }

    #[test]
    fn test_truncate_text_unicode() {
        let text = "你好世界，这是一段中文";
        let result = truncate_text(text, 5);
        assert!(result.ends_with("..."));
        assert_eq!(result, "你好世界，...");
    }

    #[test]
    fn truncate_with_marker_is_a_soft_cap() {
        // Under the limit: untouched, no marker.
        assert_eq!(truncate_with_marker("hello", 10, "…"), "hello");
        // Over: max_chars of content PLUS the marker — the marker is not
        // reserved, so the result is allowed to exceed max_chars.
        assert_eq!(truncate_with_marker("hello world", 5, "…"), "hello…");
        assert_eq!(truncate_with_marker("你好世界啊", 2, "…"), "你好…");
        // Deliberately no zero-guard: prompt_build's extra_files budget really
        // reaches 0, and it has always emitted the bare marker there.
        assert_eq!(truncate_with_marker("hello", 0, "…"), "…");
    }

    #[test]
    fn truncate_reserving_is_a_hard_cap() {
        assert_eq!(truncate_reserving("hello", 10, "…"), "hello");
        // Total length including the marker never exceeds the budget.
        let out = truncate_reserving("0123456789", 5, "…");
        assert_eq!(out, "0123…");
        assert_eq!(out.chars().count(), 5);
        let wide = truncate_reserving("0123456789", 5, "...");
        assert_eq!(wide, "01...");
        assert_eq!(wide.chars().count(), 5);
        // Budget narrower than the marker saturates rather than panicking.
        assert_eq!(truncate_reserving("0123456789", 1, "..."), "...");
    }

    #[test]
    fn truncate_chars_borrows_and_never_marks() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 2), "he");
        // Multi-byte: cuts on a codepoint boundary, never mid-sequence.
        assert_eq!(truncate_chars("你好世界", 2), "你好");
        assert_eq!(truncate_chars("你好世界", 0), "");
    }

    /// Relocated from `memory::assembler::hydration` when its byte-capped
    /// wrapper was withdrawn — the property belongs to the surviving
    /// implementation, not to the wrapper that was deleted.
    #[test]
    fn truncate_bytes_property_never_exceeds_budget_or_breaks_utf8() {
        use proptest::prelude::*;
        proptest!(|(s in "\\PC*", n in 0usize..256)| {
            let out = truncate_bytes(&s, n);
            prop_assert!(out.len() <= n);
            prop_assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        });
    }

    #[test]
    fn truncate_bytes_budget_is_bytes_and_stays_on_boundaries() {
        assert_eq!(truncate_bytes("hello", 10), "hello");
        assert_eq!(truncate_bytes("hello", 2), "he");
        // "你" is 3 bytes. A 4-byte budget fits exactly one char, and the
        // 4th byte must be dropped rather than splitting the second char.
        assert_eq!(truncate_bytes("你好", 4), "你");
        assert_eq!(truncate_bytes("你好", 2), "");
        assert_eq!(truncate_bytes("你好", 6), "你好");
    }

    /// The limit is counted in CHARACTERS, never bytes. A multi-byte string
    /// that is under the limit must come back untouched — in particular it must
    /// not gain an ellipsis advertising a cut that never happened.
    ///
    /// `mutation_gate.rs` had exactly this bug: it guarded with `s.len() <= max`
    /// (bytes) but cut with `char_indices().nth(max)` (chars), so a 4-char /
    /// 12-byte CJK string against `max = 8` took the "too long" branch, found no
    /// 8th char, fell back to the full length, and returned `"你好世界..."`.
    /// Invisible in pure ASCII, which is why it survived. Pinned here because
    /// every migrated caller now relies on this function for that guarantee.
    #[test]
    fn truncate_text_limit_is_chars_not_bytes() {
        // 4 chars, 12 bytes — well over any byte-based reading of `max = 8`.
        let text = "你好世界";
        assert_eq!(truncate_text(text, 8), text);
        assert!(!truncate_text(text, 8).ends_with("..."));
        // Exactly at the limit is still "under" — no ellipsis.
        assert_eq!(truncate_text(text, 4), text);
    }
}
