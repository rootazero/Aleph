//! Chinese mobile phone number detection with anti-false-positive checks
//!
//! Scope: this rule matches **Chinese-mainland mobile numbers only** —
//! exactly 11 digits, prefix `1[3-9]`. International numbers, Chinese
//! landlines (e.g. `010-12345678`), and formatted variants (`+86 138...`,
//! `138-1234-5678`, `138 1234 5678`) are intentionally out of scope.
//!
//! Operators who need international coverage should add a `phone_intl`
//! custom rule via `[[privacy.custom_rules]]` (see `CustomRegexRule`).
//! The combination is safe — `dedup_overlapping` resolves overlap; if a
//! number happens to match both rules the higher-severity one wins.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static PHONE_RE: OnceLock<Regex> = OnceLock::new();
static TIMESTAMP_CONTEXT_RE: OnceLock<Regex> = OnceLock::new();

/// Bytes of text before a match that are inspected for a timestamp label.
///
/// The label has to sit directly in front of the value, so only a short tail
/// can qualify. The bound keeps the per-match cost constant: scanning the
/// whole prefix for every phone-shaped number would be quadratic in the
/// number of such numbers.
const TIMESTAMP_CONTEXT_WINDOW: usize = 64;

fn phone_regex() -> &'static Regex {
    // rust-doctor-disable-next-line unwrap-in-production
    PHONE_RE.get_or_init(|| Regex::new(r"1[3-9]\d{9}").expect("static phone regex compiles"))
}

/// Matches a timestamp-ish **label** that is immediately followed by the
/// value, e.g. `"timestamp": 13891680001`.
///
/// The leading `\b` is load-bearing: without it the alternation matched the
/// tail of every word *ending* in `date`/`time` — `Validate`, `runtime`,
/// `candidate`, `certificate`, `timeline`, `lifetime` — and the old
/// ±60-byte context window then suppressed every phone number near such a
/// word, leaving it in plaintext. The trailing `["']?\s*[:=]\s*$` requires
/// the label to be adjacent to the value, so only a real `key: value` shape
/// suppresses a match.
fn timestamp_context_regex() -> &'static Regex {
    TIMESTAMP_CONTEXT_RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(timestamp|time|date|created_at|updated_at|expires?_at|modified_at)["']?\s*[:=]\s*$"#,
        )
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("static timestamp context regex compiles")
    })
}

pub struct PhoneRule;

impl PhoneRule {
    pub const fn new() -> Self {
        Self
    }

    /// Check if the match is adjacent to hex characters (likely UUID fragment)
    fn is_hex_bounded(text: &str, start: usize, end: usize) -> bool {
        // Check byte before match. Safe to use byte index because ASCII hex
        // letters are single-byte, and a multi-byte continuation byte can never
        // be an ASCII hex digit (all >= 0x80).
        if start > 0 {
            let b = text.as_bytes()[start - 1];
            if b.is_ascii_hexdigit() && !b.is_ascii_digit() {
                return true; // a-f before the number
            }
        }
        // Check byte after match
        if end < text.len() {
            let b = text.as_bytes()[end];
            if b.is_ascii_hexdigit() && !b.is_ascii_digit() {
                return true; // a-f after the number
            }
        }
        false
    }

    fn is_decimal_context(text: &str, start: usize, end: usize) -> bool {
        let bytes = text.as_bytes();
        if start > 0 && bytes[start - 1] == b'.' {
            return true;
        }
        if end < bytes.len()
            && bytes[end] == b'.'
            && end + 1 < bytes.len()
            && bytes[end + 1].is_ascii_digit()
        {
            return true;
        }
        false
    }

    /// Check whether the value sits directly behind a timestamp label, e.g.
    /// `"timestamp": 13891680001`.
    ///
    /// Only the text **before** the match is inspected: a label that merely
    /// appears somewhere nearby is not a timestamp context. The old
    /// ±60-byte window around the match turned every word ending in
    /// `date`/`time` (`Validate`, `runtime`, `candidate`, …) into one and
    /// swallowed the phone numbers next to it.
    fn is_timestamp_context(text: &str, start: usize) -> bool {
        // `start` is a char boundary (the phone regex matched there), so the
        // prefix slice cannot split a multi-byte character.
        let prefix = &text[..start];
        // Whitespace between the separator and the value carries no signal.
        let trimmed = prefix.trim_end();
        let mut window_start = trimmed.len().saturating_sub(TIMESTAMP_CONTEXT_WINDOW);
        if window_start > 0 {
            let prev = trimmed.as_bytes()[window_start - 1];
            // Keep the preceding byte when it is a word character: the
            // regex's leading `\b` has to see the real preceding character,
            // or a window that starts mid-word would fake a word boundary
            // and make `expire_at` inside `myexpire_at` look like a label.
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                window_start -= 1;
            }
        }
        // Snap to a char boundary: the window may have cut a multi-byte char.
        while window_start > 0 && !trimmed.is_char_boundary(window_start) {
            window_start -= 1;
        }
        timestamp_context_regex().is_match(&trimmed[window_start..])
    }

    /// Check word boundary: the match should not be part of a longer digit sequence
    fn has_word_boundary(text: &str, start: usize, end: usize) -> bool {
        let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_digit();
        let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_digit();
        before_ok && after_ok
    }
}

impl PiiRule for PhoneRule {
    fn name(&self) -> &str {
        "phone"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::High
    }
    fn placeholder(&self) -> &str {
        "[PHONE]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = phone_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            let start = m.start();
            let end = m.end();
            let matched = m.as_str();

            // Anti-false-positive: word boundary check
            if !Self::has_word_boundary(text, start, end) {
                continue;
            }

            // Anti-false-positive: hex boundary (UUID fragment)
            if Self::is_hex_bounded(text, start, end) {
                continue;
            }

            if Self::is_decimal_context(text, start, end) {
                continue;
            }

            // Anti-false-positive: timestamp context
            if Self::is_timestamp_context(text, start) {
                continue;
            }

            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start,
                end,
                matched_text: matched.to_string(),
                severity: self.severity(),
                placeholder: self.placeholder().to_string(),
            });
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> PhoneRule {
        PhoneRule::new()
    }

    // === Positive matches ===

    #[test]
    fn test_detect_china_mobile_13x() {
        let matches = rule().detect("Call me at 13812345678");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    #[test]
    fn test_detect_china_mobile_15x() {
        let matches = rule().detect("Phone: 15987654321");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_china_mobile_18x() {
        let matches = rule().detect("Contact 18612345678 for details");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_multiple_phones() {
        let matches = rule().detect("13812345678 and 15900001234");
        assert_eq!(matches.len(), 2);
    }

    // === Anti-false-positive: UUID fragments ===

    #[test]
    fn test_no_match_uuid_fragment() {
        // "18160019229" looks like a phone but is part of a UUID
        let matches = rule().detect("id: 18160019229f-4b7a-8c3d");
        assert_eq!(matches.len(), 0, "UUID fragment should not match as phone");
    }

    #[test]
    fn test_no_match_hex_suffix() {
        let matches = rule().detect("a18612345678b");
        // Preceded by hex 'a' — should skip
        assert_eq!(matches.len(), 0, "Hex-bounded number should not match");
    }

    #[test]
    fn test_no_match_isolated_hex_prefix_is_known_limitation() {
        // Documented limitation: an isolated single hex letter before a
        // phone (e.g. "a13812345678") is suppressed by `is_hex_bounded`.
        // This is intentional — the alternative (allowing it) produces
        // false positives on hex literals like `a13812345678` (e.g.
        // serial numbers), and the over-fire rate of the current rule is
        // bounded by the rarity of phones starting with `0` or any
        // non-digit, non-hex char. If a real production hit is observed,
        // tighten `is_hex_bounded` to require a *pair* of hex letters
        // (UUID-like prefix shape) rather than a single one.
        let matches = rule().detect("var a13812345678 = 1;");
        assert_eq!(
            matches.len(),
            0,
            "isolated hex letter before phone is treated as hex context (known limitation)"
        );
    }

    #[test]
    fn test_no_match_decimal_context() {
        let matches = rule().detect("value: 1.13812345678");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_trailing_dot_decimal() {
        let matches = rule().detect("amount 13812345678.5");
        assert_eq!(
            matches.len(),
            0,
            ".digit must be treated as decimal continuation"
        );
    }

    #[test]
    fn test_match_phone_followed_by_period() {
        let matches = rule().detect("call 13812345678.");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    #[test]
    fn test_match_phone_followed_by_dot_then_letter() {
        let matches = rule().detect("Bearer 13812345678.abcdefghijklmnop");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    // === Anti-false-positive: Timestamp context ===

    #[test]
    fn test_no_match_timestamp_context() {
        let matches = rule().detect("\"timestamp\": 13891680001");
        assert_eq!(matches.len(), 0, "Timestamp context should suppress match");
    }

    #[test]
    fn test_no_match_created_at_context() {
        let matches = rule().detect("\"created_at\": 13891680001");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_timestamp_label_with_equals_separator() {
        let matches = rule().detect("timestamp=13891680001");
        assert_eq!(
            matches.len(),
            0,
            "a `label=value` timestamp must be suppressed too"
        );
    }

    // === Regression: the timestamp guard used to swallow real phone numbers ===

    #[test]
    fn test_match_phone_after_word_ending_in_date() {
        // The old guard matched the *tail* of any word ending in `date`/
        // `time` inside a ±60-byte window, so the number below was never
        // redacted. Only a label directly in front of the value suppresses a
        // match.
        let matches = rule().detect("Validate the record: 13812345678");
        assert_eq!(
            matches.len(),
            1,
            "a phone behind a non-label word must still be detected"
        );
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    #[test]
    fn test_match_phone_after_word_ending_in_time() {
        // Same shape with the `time` family (`runtime`, `timeline`,
        // `lifetime`).
        let matches = rule().detect("The candidate runtime value: 13812345678");
        assert_eq!(
            matches.len(),
            1,
            "a phone behind a non-label word must still be detected"
        );
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    // === Normal numbers should not match ===

    #[test]
    fn test_no_match_short_number() {
        let matches = rule().detect("Version 12345");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_invalid_prefix() {
        // 10x, 11x, 12x are not valid Chinese mobile prefixes
        let matches = rule().detect("Number: 10812345678");
        assert_eq!(matches.len(), 0);
    }

    // === UTF-8 safety: Chinese text ===

    #[test]
    fn test_detect_phone_in_chinese_text() {
        let matches = rule().detect("请联系 13812345678 了解详情");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "13812345678");
    }

    #[test]
    fn test_no_panic_on_chinese_timestamp_context() {
        // Chinese characters are multi-byte, ensure no UTF-8 slicing panic
        let matches = rule().detect("时间戳是 13891680001 的记录");
        // Whether it matches or not, it should NOT panic
        let _ = matches;
    }
}
