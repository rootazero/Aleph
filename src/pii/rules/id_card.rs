//! Chinese national ID card detection with structural validation
//!
//! Validates: region code, birth date, and ISO 7064 MOD 11-2 checksum.
//! This prevents false positives on Discord Snowflake IDs and random numbers.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static ID_CARD_RE: OnceLock<Regex> = OnceLock::new();

fn id_card_regex() -> &'static Regex {
    ID_CARD_RE.get_or_init(|| {
        // Real-world ID transcriptions often include a single space or
        // hyphen between digit groups ("110101 1990 0307 002X" or
        // "110101-1990-0307-002X"). The bare 18-digit form still matches
        // because each separator is optional.
        Regex::new(r"\d(?:[ \-]?\d){16}[\dXx]")
            // rust-doctor-disable-next-line unwrap-in-production
            .expect("static id card regex compiles")
    })
}

// Valid Chinese province/municipality codes (first 2 digits)
const VALID_REGIONS: &[u8] = &[
    11, 12, 13, 14, 15, // Beijing, Tianjin, Hebei, Shanxi, Inner Mongolia
    21, 22, 23, // Liaoning, Jilin, Heilongjiang
    31, 32, 33, 34, 35, 36,
    37, // Shanghai, Jiangsu, Zhejiang, Anhui, Fujian, Jiangxi, Shandong
    41, 42, 43, 44, 45, 46, // Henan, Hubei, Hunan, Guangdong, Guangxi, Hainan
    50, 51, 52, 53, 54, // Chongqing, Sichuan, Guizhou, Yunnan, Tibet
    61, 62, 63, 64, 65, // Shaanxi, Gansu, Qinghai, Ningxia, Xinjiang
    71, 81, 82, // Taiwan, Hong Kong, Macau
];

// ISO 7064 MOD 11-2 check code mapping
const CHECK_CODES: &[char] = &['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
const WEIGHTS: &[u32] = &[7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];

pub struct IdCardRule;

impl IdCardRule {
    pub const fn new() -> Self {
        Self
    }

    /// Validate region code (first 2 digits)
    fn is_valid_region(id: &str) -> bool {
        id.get(..2)
            .and_then(|s| s.parse::<u8>().ok())
            .is_some_and(|region| VALID_REGIONS.contains(&region))
    }

    /// Validate birth date (digits 6-13, YYYYMMDD) with exact month/day limits.
    fn is_valid_date(id: &str) -> bool {
        let year: u32 = match id.get(6..10).and_then(|s| s.parse().ok()) {
            Some(y) => y,
            None => return false,
        };
        let month: u32 = match id.get(10..12).and_then(|s| s.parse().ok()) {
            Some(m) => m,
            None => return false,
        };
        let day: u32 = match id.get(12..14).and_then(|s| s.parse().ok()) {
            Some(d) => d,
            None => return false,
        };

        if !(1900..=2100).contains(&year) || !(1..=12).contains(&month) || day == 0 {
            return false;
        }

        let max_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                let is_leap = (year.is_multiple_of(4) && !year.is_multiple_of(100))
                    || year.is_multiple_of(400);
                if is_leap {
                    29
                } else {
                    28
                }
            }
            _ => return false,
        };

        day <= max_day
    }

    /// Validate ISO 7064 MOD 11-2 checksum
    fn is_valid_checksum(id: &str) -> bool {
        let bytes = id.as_bytes();
        if bytes.len() != 18 {
            return false;
        }
        let sum: u32 = bytes
            .iter()
            .take(17)
            .zip(WEIGHTS.iter())
            .map(|(&b, &w)| {
                if !b.is_ascii_digit() {
                    return 0;
                }
                u32::from(b - b'0') * w
            })
            .sum();
        let expected = CHECK_CODES[(sum % 11) as usize];
        let last = match id.chars().last() {
            Some(c) => c.to_ascii_uppercase(),
            None => return false,
        };
        last == expected
    }

    /// Full structural validation
    fn is_valid_id_card(id: &str) -> bool {
        if id.len() != 18 {
            return false;
        }
        Self::is_valid_region(id) && Self::is_valid_date(id) && Self::is_valid_checksum(id)
    }

    /// Check word boundary
    fn has_word_boundary(text: &str, start: usize, end: usize) -> bool {
        let before_ok = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric();
        let after_ok = end >= text.len() || !text.as_bytes()[end].is_ascii_alphanumeric();
        before_ok && after_ok
    }
}

impl PiiRule for IdCardRule {
    fn name(&self) -> &str {
        "id_card"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Critical
    }
    fn placeholder(&self) -> &str {
        "[ID_CARD]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = id_card_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            let start = m.start();
            let end = m.end();
            let matched = m.as_str();

            if !Self::has_word_boundary(text, start, end) {
                continue;
            }

            // Strip optional space/hyphen separators before structural
            // validation: real-world transcriptions are often grouped
            // (e.g. "110101 1990 0307 002X"). The matched span keeps the
            // original separators so the placeholder aligns with what the
            // user sees, but the checksum only sees the 18 raw chars.
            let digits_only: String = matched
                .chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .collect();

            // Normalize X to uppercase for validation
            let normalized = digits_only.to_uppercase();
            if !Self::is_valid_id_card(&normalized) {
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

    fn rule() -> IdCardRule {
        IdCardRule::new()
    }

    // === Positive matches ===

    #[test]
    fn test_detect_valid_id_card() {
        // Known-valid ID card with correct checksum (region=110101, date=19900307, seq=002, check=X)
        let matches = rule().detect("ID: 11010119900307002X");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_id_with_lowercase_x() {
        let matches = rule().detect("身份证号: 11010119900307002x");
        assert_eq!(matches.len(), 1);
    }

    // === Spaced/hyphenated forms (regression: B2-H2) ===

    #[test]
    fn test_detect_spaced_id_card() {
        let matches = rule().detect("身份证号: 110101 1990 0307 002X");
        assert_eq!(matches.len(), 1, "spaced ID card must be detected");
    }

    #[test]
    fn test_detect_hyphenated_id_card() {
        let matches = rule().detect("ID: 110101-1990-0307-002-X");
        assert_eq!(matches.len(), 1, "hyphenated ID card must be detected");
    }

    #[test]
    fn test_no_match_spaced_id_card_with_bad_checksum() {
        // Same shape as the spaced test, but the final digit is wrong.
        // The checksum strip path must still reject invalid IDs.
        let matches = rule().detect("ID: 110101 1990 0307 002Y");
        assert_eq!(matches.len(), 0, "spaced ID with bad checksum must be rejected");
    }

    // === Anti-false-positive: Discord Snowflake ===

    #[test]
    fn test_no_match_discord_snowflake() {
        // Discord Snowflake IDs: 17-20 digit numbers, won't pass structural validation
        let matches = rule().detect("channel: 1468256454954975286");
        assert_eq!(
            matches.len(),
            0,
            "Discord Snowflake should not match as ID card"
        );
    }

    // === Anti-false-positive: Random 18-digit numbers ===

    #[test]
    fn test_no_match_random_18_digits() {
        let matches = rule().detect("Order: 123456789012345678");
        assert_eq!(
            matches.len(),
            0,
            "Random number should fail structural validation"
        );
    }

    // === Anti-false-positive: Invalid region code ===

    #[test]
    fn test_no_match_invalid_region() {
        // Region code 99 is invalid
        let matches = rule().detect("ID: 990101199001011234");
        assert_eq!(matches.len(), 0);
    }

    // === Anti-false-positive: Invalid date ===

    #[test]
    fn test_no_match_invalid_date() {
        // Month 13 is invalid
        let matches = rule().detect("ID: 110101199013011234");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_invalid_day_for_month() {
        // April has 30 days, so 31st is invalid
        let matches = rule().detect("ID: 110101199004311234");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_feb_30() {
        // February never has 30 days
        let matches = rule().detect("ID: 110101199002301234");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_feb_29_non_leap() {
        // 2021 is not a leap year, Feb 29 is invalid
        let matches = rule().detect("ID: 110101202102291234");
        assert_eq!(matches.len(), 0);
    }
}
