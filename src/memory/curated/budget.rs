//! Char-budget calculation and prompt header rendering.
//!
//! Header format:
//!   `[N% — used/limit chars]`             when usage ≤ limit
//!   `[OVER BUDGET — N% — used/limit chars]` when usage > limit
//!   `[NEAR LIMIT — N% — used/limit chars]`  when ≥ `legacy_warn_threshold` but ≤ limit

use super::format::ENTRY_DELIMITER;

/// Char usage of a list of entries (after § serialization).
///
/// Counts `n` delimiters total: `n-1` between entries plus `1` trailing.
/// Matches `format::serialize`, which emits a trailing `\n§\n` sentinel so
/// the file is unambiguously distinguishable from legacy markdown on reload.
///
/// Counted in **chars, not bytes**: the name, the header line, and the
/// over-budget error all advertise "chars" to the model, and the USER.md half
/// (`snapshot::user_header`) already counts that way. Counting bytes here gave
/// a CJK user ~1/3 of the advertised budget.
#[must_use]
pub fn used_chars(entries: &[String]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    entries.iter().map(|e| e.chars().count()).sum::<usize>()
        + ENTRY_DELIMITER.chars().count() * entries.len()
}

/// Percentage of `limit` consumed (0..=100, capped at 100 for display).
#[must_use]
pub fn usage_pct(used: usize, limit: usize) -> u8 {
    if limit == 0 {
        return 100;
    }
    let raw = (used as f64 / limit as f64 * 100.0).round();
    raw.min(100.0) as u8
}

/// Render the prompt header line. Returns an empty string if `entries` is empty.
#[must_use]
pub fn header(entries: &[String], limit: usize, near_threshold: f32) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let used = used_chars(entries);
    let pct = usage_pct(used, limit);
    let pct_label = if used > limit {
        format!("OVER BUDGET — {pct}%")
    } else if (used as f32) >= (limit as f32) * near_threshold {
        format!("NEAR LIMIT — {pct}%")
    } else {
        format!("{pct}%")
    };
    format!("[{pct_label} — {used}/{limit} chars]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_entries_zero_chars() {
        assert_eq!(used_chars(&[]), 0);
        assert_eq!(header(&[], 100, 0.95), "");
    }

    #[test]
    fn used_chars_counts_delimiters() {
        let e = vec!["a".to_string(), "b".to_string()];
        // serialize emits "a" + "\n§\n" + "b" + "\n§\n" (trailing sentinel).
        // 1 + 3 + 1 + 3 = 8. Delim is 3 chars (\n, §, \n) — 4 bytes, but the
        // budget is advertised to the model in chars.
        assert_eq!(used_chars(&e), 8);
    }

    #[test]
    fn used_chars_counts_cjk_as_one_char_each() {
        // 10 CJK chars = 30 bytes UTF-8. Counting bytes would bill 30 + delim
        // and hand a Chinese-speaking user ~1/3 of the advertised budget.
        let e = vec!["中文测试内容一二三四".to_string()];
        assert_eq!(used_chars(&e), 13, "10 chars + 3-char delimiter");
        let h = header(&e, 100, 0.95);
        assert!(h.contains("13/100 chars"), "header was {h}");
        assert!(!h.contains("OVER BUDGET"), "header was {h}");
    }

    #[test]
    fn header_under_limit() {
        let e = vec!["abc".to_string()];
        // "abc" + trailing "\n§\n" = 3 + 3 = 6 chars.
        let h = header(&e, 100, 0.95);
        assert!(h.contains("6%"));
        assert!(h.contains("6/100 chars"));
        assert!(!h.contains("OVER BUDGET"));
        assert!(!h.contains("NEAR LIMIT"));
    }

    #[test]
    fn header_near_limit() {
        // "x"*96 + trailing "\n§\n" = 96 + 3 = 99 chars used (≤ limit, not over).
        // 99% ≥ 95% threshold → NEAR LIMIT.
        let e = vec!["x".repeat(96)];
        let h = header(&e, 100, 0.95);
        assert!(h.contains("NEAR LIMIT"), "header was {h}");
    }

    #[test]
    fn header_over_limit() {
        let e = vec!["x".repeat(120)];
        let h = header(&e, 100, 0.95);
        assert!(h.contains("OVER BUDGET"), "header was {h}");
        assert!(h.contains("100%"), "pct capped at 100, got {h}");
    }
}
