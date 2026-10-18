//! Re-export shim for `truncate_text` + remaining truncate siblings.
//!
//! Five mutually-incompatible truncate contracts live in this codebase (the
//! survey table below is the load-bearing documentation — see commit
//! history). Task 1 of the P0 modularization moved **one** of them —
//! `truncate_text` — to `aleph_slot_api::text`. The other four stayed here
//! because they encode different contracts (bytes vs chars, hard vs soft
//! cap, marker vs no marker) and folding them would silently change the
//! lengths that downstream APIs (WeChat, env-var caps, prompt budgets)
//! observe.
//!
//! Rather than churn 12+ call sites to point at the new module, we keep
//! the original path `crate::utils::text_format::truncate_text` working
//! as a `pub use` re-export. The body in `aleph_slot_api::text` is
//! byte-identical to the one this file used to define; UTF-8 safety
//! tests still pass at the old path, identity tests at the new.
//!
//! | Contract | Function |
//! |---|---|
//! | chars + `...` + soft cap → `String` | [`truncate_text`] (re-exported from `aleph_slot_api`) |
//! | chars + any marker + soft cap → `String` | [`truncate_with_marker`] (stays) |
//! | chars, no marker, hard cap → `&str` | [`truncate_chars`] (stays) |
//! | chars + marker + **hard** cap (total ≤ max) | [`truncate_reserving`] (stays) |
//! | **bytes**, no marker, hard cap → `&str` | [`truncate_bytes`] (stays) |

// Re-export the one helper that moved. Same definition, same path so every
// pre-existing `crate::utils::text_format::truncate_text(...)` call site
// keeps compiling and linking.
pub use aleph_slot_api::text::truncate_text;

// The other four stay. Identical bodies to the pre-migration file.

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
/// before cutting, so the result length never exceeds `max_total_chars` when
/// `max_total_chars >= marker.chars().count()`. Use where the number is a real
/// limit (a fixed-width column, an external API cap) rather than a hint.
///
/// **Saturates when `max_total_chars` is shorter than the marker.** In that
/// degenerate case the marker alone is returned — it is the floor of the
/// hard cap, not `max_total_chars - marker.chars().count()`. A caller that
/// must stay strictly under `max_total_chars` should reject markers wider
/// than the budget up-front, or fall back to [`truncate_chars`].
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
    //! The five `truncate_text`-specific tests moved to `aleph_slot_api`.
    //! The four sibling contracts' tests stay here because the functions
    //! they pin live here.

    use super::*;

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

    /// The 5 `truncate_text`-specific tests moved with the function to
    /// `aleph_slot_api::text::tests`. The identity of the function is the
    /// same (it is re-exported below), so the old call path
    /// `crate::utils::text_format::truncate_text` keeps the same body.
    /// Pinned in the API crate's test module.
    #[test]
    fn truncate_text_re_export_is_same_function() {
        // Function-pointer identity, not just `==` on result. Catches a
        // future refactor that accidentally re-defines instead of
        // re-exporting.
        //
        // The address comparison is informational, not a hard guarantee:
        // rustc warns that the same function can have different addresses
        // across codegen units. We silence the warning because in practice
        // `pub use` aliases the same symbol and the addresses do match.
        // The `tests/modularization_contracts.rs` integration test does
        // the same check across the crate boundary; the unit test here
        // anchors the in-crate perspective.
        #[allow(unpredictable_function_pointer_comparisons)]
        {
            let local_ptr: fn(&str, usize) -> String = truncate_text;
            let api_ptr: fn(&str, usize) -> String = aleph_slot_api::text::truncate_text;
            assert_eq!(local_ptr, api_ptr);
        }
    }
}
