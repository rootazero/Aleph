//! Char-budget calculation and prompt header rendering.
//!
//! Header format:
//!   `[N% — used/limit chars]`             when usage ≤ limit
//!   `[OVER BUDGET — N% — used/limit chars]` when usage > limit
//!   `[NEAR LIMIT — N% — used/limit chars]`  when ≥ legacy_warn_threshold but ≤ limit

use super::format::{serialize, ENTRY_DELIMITER};

/// Char usage of a list of entries (after § serialization).
///
/// Counts `n` delimiters total: `n-1` between entries plus `1` trailing.
/// Matches `format::serialize`, which emits a trailing `\n§\n` sentinel so
/// the file is unambiguously distinguishable from legacy markdown on reload.
pub fn used_chars(entries: &[String]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    entries.iter().map(|e| e.len()).sum::<usize>() + ENTRY_DELIMITER.len() * entries.len()
}

/// Percentage of `limit` consumed (0..=100, capped at 100 for display).
pub fn usage_pct(used: usize, limit: usize) -> u8 {
    if limit == 0 {
        return 100;
    }
    let raw = (used as f64 / limit as f64 * 100.0).round();
    raw.min(100.0) as u8
}

/// Render the prompt header line. Returns an empty string if `entries` is empty.
pub fn header(entries: &[String], limit: usize, near_threshold: f32) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let used = used_chars(entries);
    let pct = usage_pct(used, limit);
    let pct_label = if used > limit {
        format!("OVER BUDGET — {}%", pct)
    } else if (used as f32) >= (limit as f32) * near_threshold {
        format!("NEAR LIMIT — {}%", pct)
    } else {
        format!("{}%", pct)
    };
    format!("[{} — {}/{} chars]", pct_label, used, limit)
}

/// Sanity check: would adding `new_content` exceed the limit?
pub fn would_exceed(entries: &[String], new_content: &str, limit: usize) -> bool {
    let projected: Vec<String> = entries
        .iter()
        .cloned()
        .chain(std::iter::once(new_content.to_string()))
        .collect();
    let _ = serialize(&projected); // keep type honest
    used_chars(&projected) > limit
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
        // 1 + 4 + 1 + 4 = 10. Delim is 4 bytes (\n=1, §=2 [U+00A7=0xC2 0xA7], \n=1).
        assert_eq!(used_chars(&e), 10);
    }

    #[test]
    fn header_under_limit() {
        let e = vec!["abc".to_string()];
        // "abc" + trailing "\n§\n" = 3 + 4 = 7 bytes.
        let h = header(&e, 100, 0.95);
        assert!(h.contains("7%"));
        assert!(h.contains("7/100 chars"));
        assert!(!h.contains("OVER BUDGET"));
        assert!(!h.contains("NEAR LIMIT"));
    }

    #[test]
    fn header_near_limit() {
        // "x"*96 + trailing "\n§\n" = 96 + 4 = 100 chars used (== limit, not over).
        // 100% ≥ 95% threshold → NEAR LIMIT.
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

    #[test]
    fn would_exceed_when_adding() {
        let e = vec!["x".repeat(90)];
        // Projected ["x"*90, "ab"] → 90 + 4 (between) + 2 + 4 (trailing) = 100, not exceeding.
        assert!(!would_exceed(&e, "ab", 100));
        // Projected ["x"*90, "abc"] → 90 + 4 + 3 + 4 = 101, exceeding.
        assert!(would_exceed(&e, "abc", 100));
    }
}
