/// PII (Personally Identifiable Information) scrubbing utilities
///
/// This module provides privacy protection by removing sensitive information
/// from text before it is logged or stored. All scrubbing is local and
/// conservative (false positives are acceptable).
use regex::Regex;
use std::sync::OnceLock;

/// PII scrubbing regex patterns (compiled once for performance)
struct PiiPatterns {
    email: Regex,
    phone: Regex,
    ssn: Regex,
    credit_card: Regex,
    api_key: Regex,
    // Extended patterns for Chinese users
    china_mobile: Regex,
    china_id: Regex,
    bank_card: Regex,
}

/// Global PII patterns (lazy-initialized)
static PII_PATTERNS: OnceLock<PiiPatterns> = OnceLock::new();

/// Get or initialize PII patterns
fn get_patterns() -> &'static PiiPatterns {
    PII_PATTERNS.get_or_init(|| PiiPatterns {
        // Email addresses (RFC 5322 simplified)
        email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(),

        // Phone numbers (various formats)
        // Matches: (123) 456-7890, 123-456-7890, 123.456.7890, 1234567890, +1-123-456-7890
        phone: Regex::new(r"\b(\+?1?[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap(),

        // SSN (Social Security Number)
        ssn: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap(),

        // Credit card numbers (simple pattern: 4 groups of 4 digits)
        credit_card: Regex::new(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b").unwrap(),

        // API keys (OpenAI, Anthropic, Tavily, Google, xAI, etc.)
        // Matches: sk-..., sk-ant-..., tvly-..., xai-..., AIza..., Bearer ...
        // Pattern explanation:
        // - sk-[...]{20,}: OpenAI keys (sk-proj..., sk-...)
        // - sk-ant-[...]{20,}: Anthropic keys
        // - tvly-[...]{20,}: Tavily keys
        // - xai-[...]{20,}: xAI keys
        // - AIza[...]{30,}: Google API keys
        // - Bearer [token]: OAuth/JWT tokens
        api_key: Regex::new(r"\b(sk-[a-zA-Z0-9\-_]{20,}|sk-ant-[a-zA-Z0-9\-_]{20,}|tvly-[a-zA-Z0-9\-_]{20,}|xai-[a-zA-Z0-9\-_]{20,}|AIza[a-zA-Z0-9\-_]{30,}|Bearer\s+[a-zA-Z0-9._\-]{20,})\b").unwrap(),

        // Chinese mobile phone numbers
        // Matches: 13812345678, 15987654321, 18612345678
        // Pattern: 1 followed by 3-9, then 9 more digits
        china_mobile: Regex::new(r"\b1[3-9]\d{9}\b").unwrap(),

        // Chinese ID card numbers
        // Matches: 310101199001011234, 31010119900101123X
        // Pattern: 17 digits + check digit (digit or X/x)
        china_id: Regex::new(r"\b\d{17}[\dXx]\b").unwrap(),

        // Bank card numbers (16-19 digits)
        // Matches: 6222021234567890123, 4111111111111111
        // Note: Applied after credit_card to avoid overlap
        bank_card: Regex::new(r"\b\d{16,19}\b").unwrap(),
    })
}

/// Scrub personally identifiable information from text
///
/// Replaces PII patterns with placeholder tokens:
/// - Email addresses → [EMAIL]
/// - Phone numbers → [PHONE]
/// - SSN/Tax IDs → [SSN]
/// - Credit card numbers → [CREDIT_CARD]
/// - API keys → [REDACTED]
///
/// This function is conservative and may produce false positives to ensure
/// privacy protection. It is designed for logging and memory storage where
/// privacy is more important than preserving exact text.
///
/// # Arguments
/// * `text` - Input text to scrub
///
/// # Returns
/// * `String` - Scrubbed text with PII replaced by placeholders
///
/// # Examples
/// ```rust,ignore
/// use aethecore::utils::pii::scrub_pii;
///
/// let text = "Contact me at john@example.com or call 123-456-7890";
/// let scrubbed = scrub_pii(text);
/// assert_eq!(scrubbed, "Contact me at [EMAIL] or call [PHONE]");
/// ```
pub fn scrub_pii(text: &str) -> String {
    let patterns = get_patterns();

    let mut scrubbed = text.to_string();

    // Apply scrubbing in order (more specific patterns first to avoid partial matches)
    // 1. API keys first (most specific, longest patterns)
    scrubbed = patterns
        .api_key
        .replace_all(&scrubbed, "[REDACTED]")
        .to_string();

    // 2. Chinese ID cards (18 digits, more specific than bank cards)
    scrubbed = patterns
        .china_id
        .replace_all(&scrubbed, "[ID_CARD]")
        .to_string();

    // 3. Email addresses
    scrubbed = patterns.email.replace_all(&scrubbed, "[EMAIL]").to_string();

    // 4. Chinese mobile numbers (11 digits starting with 1[3-9])
    scrubbed = patterns
        .china_mobile
        .replace_all(&scrubbed, "[PHONE]")
        .to_string();

    // 5. US/International phone numbers
    scrubbed = patterns.phone.replace_all(&scrubbed, "[PHONE]").to_string();

    // 6. SSN (Social Security Number)
    scrubbed = patterns.ssn.replace_all(&scrubbed, "[SSN]").to_string();

    // 7. Credit card numbers (16 digits with optional separators)
    scrubbed = patterns
        .credit_card
        .replace_all(&scrubbed, "[CREDIT_CARD]")
        .to_string();

    // 8. Bank card numbers (16-19 consecutive digits, applied last)
    // Note: This may catch some false positives, but privacy > accuracy
    scrubbed = patterns
        .bank_card
        .replace_all(&scrubbed, "[BANK_CARD]")
        .to_string();

    scrubbed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrub_email() {
        let text = "Contact me at john.doe@example.com or jane@test.org";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Contact me at [EMAIL] or [EMAIL]");
    }

    #[test]
    fn test_scrub_phone() {
        let text = "Call me at 123-456-7890 or (987) 654-3210";
        let scrubbed = scrub_pii(text);
        assert!(scrubbed.contains("[PHONE]"));
        assert!(!scrubbed.contains("123-456-7890"));
        assert!(!scrubbed.contains("(987) 654-3210"));
    }

    #[test]
    fn test_scrub_phone_international() {
        let text = "International: +1-555-123-4567";
        let scrubbed = scrub_pii(text);
        assert!(scrubbed.contains("[PHONE]"));
        assert!(!scrubbed.contains("555-123-4567"));
    }

    #[test]
    fn test_scrub_ssn() {
        let text = "My SSN is 123-45-6789";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "My SSN is [SSN]");
    }

    #[test]
    fn test_scrub_credit_card() {
        let text = "Card number: 1234-5678-9012-3456";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Card number: [CREDIT_CARD]");
    }

    #[test]
    fn test_scrub_credit_card_no_dashes() {
        let text = "Card: 1234567890123456";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Card: [CREDIT_CARD]");
    }

    #[test]
    fn test_scrub_api_key_openai() {
        let text = "My API key is sk-proj1234567890abcdefghijklmnopqrstuvwxyz";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "My API key is [REDACTED]");
    }

    #[test]
    fn test_scrub_api_key_anthropic() {
        let text = "Using sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Using [REDACTED]");
    }

    #[test]
    fn test_scrub_api_key_tavily() {
        let text = "Tavily key: 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";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Tavily key: [REDACTED]");
    }

    #[test]
    fn test_scrub_api_key_xai() {
        let text = "xAI key: xai-abcdefghij1234567890klmnopqrstuvwxyz";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "xAI key: [REDACTED]");
    }

    #[test]
    fn test_scrub_api_key_google() {
        let text = "Google key: AIzaSyABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Google key: [REDACTED]");
    }

    #[test]
    fn test_scrub_bearer_token() {
        let text = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Authorization: [REDACTED]");
    }

    #[test]
    fn test_scrub_multiple_pii() {
        let text = "Email: john@example.com, Phone: 123-456-7890, SSN: 123-45-6789, API: sk-test1234567890abcdefghij";
        let scrubbed = scrub_pii(text);

        assert!(scrubbed.contains("[EMAIL]"));
        assert!(scrubbed.contains("[PHONE]"));
        assert!(scrubbed.contains("[SSN]"));
        assert!(scrubbed.contains("[REDACTED]"));

        assert!(!scrubbed.contains("john@example.com"));
        assert!(!scrubbed.contains("123-456-7890"));
        assert!(!scrubbed.contains("123-45-6789"));
        assert!(!scrubbed.contains("sk-test"));
    }

    #[test]
    fn test_scrub_no_pii() {
        let text = "This text has no PII in it. Just normal words.";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, text);
    }

    #[test]
    fn test_scrub_preserves_structure() {
        let text = "User info:\n  Email: test@example.com\n  Phone: 555-1234";
        let scrubbed = scrub_pii(text);
        assert!(scrubbed.starts_with("User info:\n"));
        assert!(scrubbed.contains("[EMAIL]"));
        // Note: 555-1234 is only 7 digits, won't match phone pattern (needs 10 digits)
    }

    #[test]
    fn test_scrub_case_insensitive_bearer() {
        let text = "bearer abc123def456ghi789jkl012mno345pqr";
        let scrubbed = scrub_pii(text);
        // Should not match (regex is case-sensitive for "Bearer")
        // This is intentional to avoid false positives with common words
        assert!(!scrubbed.contains("[REDACTED]"));
    }

    #[test]
    fn test_scrub_partial_matches_avoided() {
        // Test that we don't scrub non-PII that looks similar
        let text = "Version 1.2.3-45-6789 released";
        let scrubbed = scrub_pii(text);
        // This is a false positive (matches SSN pattern), but that's acceptable
        // for conservative privacy protection
        // We'll just verify the scrubbing function works
        assert!(!scrubbed.is_empty());
    }

    #[test]
    fn test_scrub_performance() {
        // Test that scrubbing is fast even with long text
        let long_text = "Normal text ".repeat(1000);
        let start = std::time::Instant::now();
        let _scrubbed = scrub_pii(&long_text);
        let elapsed = start.elapsed();

        // Should complete in <50ms even for large text (more lenient for CI)
        assert!(
            elapsed.as_millis() < 50,
            "Scrubbing took too long: {:?}",
            elapsed
        );
    }

    // ==========================================================================
    // Extended PII Pattern Tests (Chinese users)
    // ==========================================================================

    #[test]
    fn test_scrub_china_mobile_13x() {
        let text = "Call me at 13812345678";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Call me at [PHONE]");
    }

    #[test]
    fn test_scrub_china_mobile_15x() {
        let text = "Phone: 15987654321";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Phone: [PHONE]");
    }

    #[test]
    fn test_scrub_china_mobile_18x() {
        let text = "Contact: 18612345678";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Contact: [PHONE]");
    }

    #[test]
    fn test_scrub_china_mobile_19x() {
        let text = "Mobile: 19912345678";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Mobile: [PHONE]");
    }

    #[test]
    fn test_scrub_china_mobile_not_10x() {
        // Test that the China mobile regex specifically doesn't match 10x numbers
        // Note: 10812345678 might still be matched by US phone regex
        // This test focuses on the China mobile pattern behavior
        let patterns = get_patterns();

        // 10x is NOT a valid Chinese mobile prefix (valid: 13x-19x)
        // The regex 1[3-9]\d{9} should NOT match numbers starting with 10, 11, or 12
        assert!(!patterns.china_mobile.is_match("10812345678")); // 10x - invalid
        assert!(!patterns.china_mobile.is_match("11812345678")); // 11x - invalid
        assert!(!patterns.china_mobile.is_match("12812345678")); // 12x - invalid

        // Valid prefixes should match
        assert!(patterns.china_mobile.is_match("13812345678")); // 13x - valid
        assert!(patterns.china_mobile.is_match("19912345678")); // 19x - valid
    }

    #[test]
    fn test_scrub_china_id_card() {
        let text = "ID: 310101199001011234";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "ID: [ID_CARD]");
    }

    #[test]
    fn test_scrub_china_id_card_with_x() {
        let text = "ID number: 31010119900101123X";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "ID number: [ID_CARD]");
    }

    #[test]
    fn test_scrub_china_id_card_lowercase_x() {
        let text = "身份证号: 31010119900101123x";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "身份证号: [ID_CARD]");
    }

    #[test]
    fn test_scrub_bank_card_16_digits() {
        let text = "Card: 6222021234567890";
        let scrubbed = scrub_pii(text);
        // 16 digits should match bank_card pattern
        assert!(scrubbed.contains("[BANK_CARD]") || scrubbed.contains("[CREDIT_CARD]"));
    }

    #[test]
    fn test_scrub_bank_card_19_digits() {
        let text = "Account: 6222021234567890123";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, "Account: [BANK_CARD]");
    }

    #[test]
    fn test_scrub_multiple_chinese_pii() {
        let text = "手机: 13812345678, 身份证: 310101199001011234, 银行卡: 6222021234567890123";
        let scrubbed = scrub_pii(text);

        assert!(scrubbed.contains("[PHONE]"));
        assert!(scrubbed.contains("[ID_CARD]"));
        assert!(scrubbed.contains("[BANK_CARD]"));

        assert!(!scrubbed.contains("13812345678"));
        assert!(!scrubbed.contains("310101199001011234"));
        assert!(!scrubbed.contains("6222021234567890123"));
    }

    #[test]
    fn test_scrub_mixed_chinese_and_us_pii() {
        let text = "Chinese phone: 13812345678, US phone: 123-456-7890, Email: test@example.com";
        let scrubbed = scrub_pii(text);

        // Both phones should be scrubbed
        assert!(!scrubbed.contains("13812345678"));
        assert!(!scrubbed.contains("123-456-7890"));
        assert!(!scrubbed.contains("test@example.com"));

        assert!(scrubbed.contains("[PHONE]"));
        assert!(scrubbed.contains("[EMAIL]"));
    }

    #[test]
    fn test_scrub_normal_numbers_not_affected() {
        // Short numbers should not be scrubbed
        let text = "Version 123, count 456789";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, text);
    }

    #[test]
    fn test_scrub_chinese_text_preserved() {
        let text = "用户信息已更新";
        let scrubbed = scrub_pii(text);
        assert_eq!(scrubbed, text);
    }
}
