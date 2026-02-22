//! Chinese mobile phone number detection with anti-false-positive checks

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static PHONE_RE: OnceLock<Regex> = OnceLock::new();
static TIMESTAMP_CONTEXT_RE: OnceLock<Regex> = OnceLock::new();

fn phone_regex() -> &'static Regex {
    PHONE_RE.get_or_init(|| Regex::new(r"1[3-9]\d{9}").unwrap())
}

fn timestamp_context_regex() -> &'static Regex {
    TIMESTAMP_CONTEXT_RE.get_or_init(|| {
        Regex::new(
            r"(?i)(timestamp|time|date|created_at|updated_at|expires?_at|modified_at)\b",
        )
        .unwrap()
    })
}

pub struct PhoneRule;

impl PhoneRule {
    pub fn new() -> Self {
        Self
    }

    /// Check if the match is adjacent to hex characters (likely UUID fragment)
    fn is_hex_bounded(text: &str, start: usize, end: usize) -> bool {
        // Check character before match
        if start > 0 {
            if let Some(c) = text[..start].chars().last() {
                if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                    return true; // a-f before the number
                }
            }
        }
        // Check character after match
        if end < text.len() {
            if let Some(c) = text[end..].chars().next() {
                if c.is_ascii_hexdigit() && !c.is_ascii_digit() {
                    return true; // a-f after the number
                }
            }
        }
        false
    }

    /// Check if match is in a timestamp context (surrounding 80 chars)
    fn is_timestamp_context(text: &str, start: usize) -> bool {
        let ctx_start = start.saturating_sub(40);
        let ctx_end = (start + 40).min(text.len());
        let context = &text[ctx_start..ctx_end];
        timestamp_context_regex().is_match(context)
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
}
