//! Char-budget calculation and prompt header rendering.
//!
//! Header format:
//!   `[N% — used/limit chars]`             when usage ≤ limit
//!   `[OVER BUDGET — N% — used/limit chars]` when usage > limit
//!   `[NEAR LIMIT — N% — used/limit chars]`  when ≥ legacy_warn_threshold but ≤ limit

use super::format::{serialize, ENTRY_DELIMITER};

/// Char usage of a list of entries (after § serialization).
pub fn used_chars(entries: &[String]) -> usize {
    if entries.is_empty() {
        return 0;
    }
    entries.iter().map(|e| e.len()).sum::<usize>()
        + ENTRY_DELIMITER.len() * entries.len().saturating_sub(1)
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
        // "a" + "\n§\n" (4 bytes: \n=1, §=2 [U+00A7=0xC2 0xA7], \n=1) + "b" = 1 + 4 + 1 = 6
        assert_eq!(used_chars(&e), 6);
    }

    #[test]
    fn header_under_limit() {
        let e = vec!["abc".to_string()];
        let h = header(&e, 100, 0.95);
        assert!(h.contains("3%"));
        assert!(h.contains("3/100 chars"));
        assert!(!h.contains("OVER BUDGET"));
        assert!(!h.contains("NEAR LIMIT"));
    }

    #[test]
    fn header_near_limit() {
        // 96 chars used out of 100 = 96% > 95% threshold
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
        let e = vec!["x".repeat(94)];
        assert!(!would_exceed(&e, "ab", 100)); // 94 + 4 (\n§\n) + 2 = 100, not exceeding
        assert!(would_exceed(&e, "abc", 100)); // 94 + 4 + 3 = 101, exceeding
    }
}
