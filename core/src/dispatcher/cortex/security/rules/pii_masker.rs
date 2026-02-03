//! PiiMaskerRule - Locale-aware PII masking
//!
//! This rule detects and masks personally identifiable information (PII)
//! based on the locale context. Supports:
//! - Common patterns: email, IP addresses
//! - Chinese patterns: phone numbers (1[3-9]xxxxxxxxx), ID cards, bank cards
//! - English patterns: SSN, US phone numbers

use regex::Regex;

use crate::dispatcher::cortex::security::{
    Locale, SanitizeContext, SanitizeResult, SanitizerRule, SecurityConfig,
};

/// PII pattern definition with regex and replacement text
struct PiiPattern {
    regex: Regex,
    replacement: &'static str,
    description: &'static str,
}

/// Rule that detects and masks personally identifiable information
///
/// Supports locale-aware pattern matching for Chinese and English PII.
/// Common patterns like email and IP are applied regardless of locale.
pub struct PiiMaskerRule {
    /// Patterns applied regardless of locale
    common_patterns: Vec<PiiPattern>,
    /// Chinese-specific patterns
    zh_patterns: Vec<PiiPattern>,
    /// English-specific patterns
    en_patterns: Vec<PiiPattern>,
}

impl Default for PiiMaskerRule {
    fn default() -> Self {
        Self {
            common_patterns: vec![
                PiiPattern {
                    // Email addresses
                    regex: Regex::new(
                        r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}",
                    )
                    .unwrap(),
                    replacement: "[EMAIL]",
                    description: "email address",
                },
                PiiPattern {
                    // IPv4 addresses
                    regex: Regex::new(
                        r"\b(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\b",
                    )
                    .unwrap(),
                    replacement: "[IP]",
                    description: "IP address",
                },
            ],
            zh_patterns: vec![
                PiiPattern {
                    // Chinese mobile phone numbers (11 digits starting with 1[3-9])
                    regex: Regex::new(r"\b1[3-9]\d{9}\b").unwrap(),
                    replacement: "[PHONE_CN]",
                    description: "Chinese phone number",
                },
                PiiPattern {
                    // Chinese ID card numbers (18 digits, last may be X)
                    regex: Regex::new(r"\b\d{17}[\dXx]\b").unwrap(),
                    replacement: "[ID_CARD_CN]",
                    description: "Chinese ID card",
                },
                PiiPattern {
                    // Chinese bank card numbers (16-19 digits starting with 62)
                    regex: Regex::new(r"\b62\d{14,17}\b").unwrap(),
                    replacement: "[BANK_CARD_CN]",
                    description: "Chinese bank card",
                },
            ],
            en_patterns: vec![
                PiiPattern {
                    // US Social Security Number (XXX-XX-XXXX)
                    regex: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap(),
                    replacement: "[SSN]",
                    description: "US SSN",
                },
                PiiPattern {
                    // US phone numbers with area code
                    // Handles: (415) 555-1234, 415-555-1234, 415.555.1234, +1 415 555 1234
                    regex: Regex::new(
                        r"(?:\+1[-.\s]?)?\([2-9]\d{2}\)[-.\s]?\d{3}[-.\s]?\d{4}|(?:\+1[-.\s]?)?[2-9]\d{2}[-.\s]\d{3}[-.\s]\d{4}",
                    )
                    .unwrap(),
                    replacement: "[PHONE_US]",
                    description: "US phone number",
                },
            ],
        }
    }
}

impl PiiMaskerRule {
    /// Create a new PiiMaskerRule with default patterns
    pub fn new() -> Self {
        Self::default()
    }

    /// Get patterns to apply based on locale
    fn get_patterns_for_locale(&self, locale: Locale) -> Vec<&PiiPattern> {
        let mut patterns: Vec<&PiiPattern> = self.common_patterns.iter().collect();

        match locale {
            Locale::ZhCN => {
                patterns.extend(self.zh_patterns.iter());
            }
            Locale::EnUS => {
                patterns.extend(self.en_patterns.iter());
            }
            Locale::Other => {
                // For unknown locales, apply all patterns
                patterns.extend(self.zh_patterns.iter());
                patterns.extend(self.en_patterns.iter());
            }
        }

        patterns
    }
}

impl SanitizerRule for PiiMaskerRule {
    fn name(&self) -> &str {
        "pii_masker"
    }

    fn priority(&self) -> u32 {
        20
    }

    fn sanitize(&self, input: &str, ctx: &SanitizeContext) -> SanitizeResult {
        let patterns = self.get_patterns_for_locale(ctx.locale);

        let mut output = input.to_string();
        let mut total_matches = 0;
        let mut matched_types = Vec::new();

        for pattern in patterns {
            let count = pattern.regex.find_iter(&output).count();
            if count > 0 {
                total_matches += count;
                matched_types.push(pattern.description);
                output = pattern.regex.replace_all(&output, pattern.replacement).to_string();
            }
        }

        if total_matches > 0 {
            let details = Some(format!(
                "Masked {} PII item(s): {}",
                total_matches,
                matched_types.join(", ")
            ));
            SanitizeResult::masked(output, total_matches, details)
        } else {
            SanitizeResult::pass(input)
        }
    }

    fn is_enabled(&self, config: &SecurityConfig) -> bool {
        config.enabled && config.pii_masking_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chinese_phone_masking() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };

        let input = "My phone is 13812345678";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "My phone is [PHONE_CN]");
        assert!(matches!(
            result.action,
            crate::dispatcher::cortex::security::SanitizeAction::Masked(1)
        ));
    }

    #[test]
    fn test_chinese_id_card() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };

        let input = "ID: 110101199003071234";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "ID: [ID_CARD_CN]");
    }

    #[test]
    fn test_chinese_id_card_with_x() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };

        let input = "ID: 11010119900307123X";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "ID: [ID_CARD_CN]");
    }

    #[test]
    fn test_chinese_bank_card() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };

        let input = "Card: 6222021234567890123";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "Card: [BANK_CARD_CN]");
    }

    #[test]
    fn test_us_ssn() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::EnUS,
            ..Default::default()
        };

        let input = "SSN: 123-45-6789";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "SSN: [SSN]");
    }

    #[test]
    fn test_us_phone() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::EnUS,
            ..Default::default()
        };

        // Test various US phone formats
        let inputs = vec![
            "(415) 555-1234",
            "415-555-1234",
            "415.555.1234",
            "+1 415 555 1234",
        ];

        for input in inputs {
            let result = rule.sanitize(input, &ctx);
            assert!(result.triggered, "Failed for: {}", input);
            assert_eq!(result.output, "[PHONE_US]", "Failed for: {}", input);
        }
    }

    #[test]
    fn test_email_all_locales() {
        let rule = PiiMaskerRule::default();
        let input = "Contact: test@example.com";

        // Test email masking works for all locales
        for locale in [Locale::EnUS, Locale::ZhCN, Locale::Other] {
            let ctx = SanitizeContext {
                locale,
                ..Default::default()
            };

            let result = rule.sanitize(input, &ctx);
            assert!(result.triggered, "Failed for locale: {:?}", locale);
            assert_eq!(
                result.output, "Contact: [EMAIL]",
                "Failed for locale: {:?}",
                locale
            );
        }
    }

    #[test]
    fn test_ip_address() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext::default();

        let input = "Server IP: 192.168.1.100";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "Server IP: [IP]");
    }

    #[test]
    fn test_multiple_pii() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };

        let input = "Email: test@example.com, Phone: 13812345678";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert_eq!(result.output, "Email: [EMAIL], Phone: [PHONE_CN]");
        assert!(matches!(
            result.action,
            crate::dispatcher::cortex::security::SanitizeAction::Masked(2)
        ));
    }

    #[test]
    fn test_no_pii() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext::default();

        let input = "Hello, how can I help you today?";
        let result = rule.sanitize(input, &ctx);

        assert!(!result.triggered);
        assert_eq!(result.output, input);
        assert!(matches!(
            result.action,
            crate::dispatcher::cortex::security::SanitizeAction::Pass
        ));
    }

    #[test]
    fn test_locale_specific_patterns() {
        let rule = PiiMaskerRule::default();

        // Chinese phone should not be detected in EnUS locale
        let ctx = SanitizeContext {
            locale: Locale::EnUS,
            ..Default::default()
        };
        let input = "Number: 13812345678";
        let result = rule.sanitize(input, &ctx);
        // Chinese phone format happens to match 10-digit part, but 11 digits won't match US format
        // Actually, the Chinese phone pattern uses word boundaries, so this won't match in EnUS
        assert!(!result.triggered);
        assert_eq!(result.output, input);

        // US SSN should not be detected in ZhCN locale
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };
        let input = "SSN: 123-45-6789";
        let result = rule.sanitize(input, &ctx);
        assert!(!result.triggered);
        assert_eq!(result.output, input);
    }

    #[test]
    fn test_other_locale_applies_all_patterns() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::Other,
            ..Default::default()
        };

        // Should detect both Chinese and US patterns
        let input = "Phone CN: 13812345678, SSN: 123-45-6789";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        assert!(result.output.contains("[PHONE_CN]"));
        assert!(result.output.contains("[SSN]"));
    }

    #[test]
    fn test_is_enabled() {
        let rule = PiiMaskerRule::default();

        // Enabled when both master and feature flags are on
        let config = SecurityConfig::default_enabled();
        assert!(rule.is_enabled(&config));

        // Disabled when master is off
        let config = SecurityConfig {
            enabled: false,
            pii_masking_enabled: true,
            ..Default::default()
        };
        assert!(!rule.is_enabled(&config));

        // Disabled when feature flag is off
        let config = SecurityConfig {
            enabled: true,
            pii_masking_enabled: false,
            ..Default::default()
        };
        assert!(!rule.is_enabled(&config));
    }

    #[test]
    fn test_details_contain_pii_types() {
        let rule = PiiMaskerRule::default();
        let ctx = SanitizeContext {
            locale: Locale::ZhCN,
            ..Default::default()
        };

        let input = "Email: test@example.com, Phone: 13812345678";
        let result = rule.sanitize(input, &ctx);

        assert!(result.triggered);
        let details = result.details.unwrap();
        assert!(details.contains("email"));
        assert!(details.contains("phone"));
        assert!(details.contains("2 PII"));
    }
}
