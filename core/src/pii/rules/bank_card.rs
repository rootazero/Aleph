//! Bank card / credit card number detection with Luhn checksum validation

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static BANK_CARD_RE: OnceLock<Regex> = OnceLock::new();

fn bank_card_regex() -> &'static Regex {
    BANK_CARD_RE.get_or_init(|| Regex::new(r"\d{16,19}").unwrap())
}

pub struct BankCardRule;

impl BankCardRule {
    pub fn new() -> Self {
        Self
    }

    /// Luhn algorithm checksum validation
    fn luhn_check(number: &str) -> bool {
        let digits: Vec<u32> = number
            .chars()
            .filter_map(|c| c.to_digit(10))
            .collect();

        if digits.len() < 16 {
            return false;
        }

        let sum: u32 = digits
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &d)| {
                if i % 2 == 1 {
                    let doubled = d * 2;
                    if doubled > 9 { doubled - 9 } else { doubled }
                } else {
                    d
                }
            })
            .sum();

        sum % 10 == 0
    }

    /// Check if surrounded by decimal context (e.g., JSON float)
    fn is_decimal_context(text: &str, start: usize, end: usize) -> bool {
        if start > 0 && text.as_bytes()[start - 1] == b'.' {
            return true;
        }
        if end < text.len() && text.as_bytes()[end] == b'.' {
            return true;
        }
        false
    }

    /// Check word boundary (not part of longer digit sequence)
    fn has_word_boundary(text: &str, start: usize, end: usize) -> bool {
        let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_digit();
        let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_digit();
        before_ok && after_ok
    }
}

impl PiiRule for BankCardRule {
    fn name(&self) -> &str {
        "bank_card"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::High
    }
    fn placeholder(&self) -> &str {
        "[BANK_CARD]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = bank_card_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            let start = m.start();
            let end = m.end();
            let matched = m.as_str();

            if !Self::has_word_boundary(text, start, end) {
                continue;
            }

            // Skip decimal context
            if Self::is_decimal_context(text, start, end) {
                continue;
            }

            // Validate with Luhn algorithm
            if !Self::luhn_check(matched) {
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

    fn rule() -> BankCardRule {
        BankCardRule::new()
    }

    // Visa test card (Luhn valid)
    #[test]
    fn test_detect_valid_visa_card() {
        // 4532015112830366 is a known Luhn-valid test card number
        let matches = rule().detect("Card: 4532015112830366");
        assert_eq!(matches.len(), 1);
    }

    // Mastercard test card (Luhn valid)
    #[test]
    fn test_detect_valid_mastercard() {
        // 5425233430109903 is Luhn-valid
        let matches = rule().detect("5425233430109903");
        assert_eq!(matches.len(), 1);
    }

    // Number that fails Luhn
    #[test]
    fn test_no_match_invalid_luhn() {
        let matches = rule().detect("1234567890123456");
        assert_eq!(matches.len(), 0, "Should fail Luhn check");
    }

    // Decimal context
    #[test]
    fn test_no_match_decimal_context() {
        let matches = rule().detect("value: .4532015112830366");
        assert_eq!(matches.len(), 0, "Decimal-prefixed number should not match");
    }

    // Short number
    #[test]
    fn test_no_match_short_number() {
        let matches = rule().detect("ID: 12345678");
        assert_eq!(matches.len(), 0);
    }
}
