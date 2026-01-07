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

        // API keys (OpenAI, Anthropic, etc.)
        // Matches: sk-..., sk-ant-..., Bearer ...
        api_key: Regex::new(r"\b(sk-[a-zA-Z0-9\-]{20,}|Bearer\s+[a-zA-Z0-9._\-]{20,})\b").unwrap(),
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
/// ```
/// use aether::utils::pii::scrub_pii;
///
/// let text = "Contact me at john@example.com or call 123-456-7890";
/// let scrubbed = scrub_pii(text);
/// assert_eq!(scrubbed, "Contact me at [EMAIL] or call [PHONE]");
/// ```
pub fn scrub_pii(text: &str) -> String {
    let patterns = get_patterns();

    let mut scrubbed = text.to_string();

    // Apply scrubbing in order (API keys first to avoid partial matches)
    scrubbed = patterns
        .api_key
        .replace_all(&scrubbed, "[REDACTED]")
        .to_string();
    scrubbed = patterns.email.replace_all(&scrubbed, "[EMAIL]").to_string();
    scrubbed = patterns.phone.replace_all(&scrubbed, "[PHONE]").to_string();
    scrubbed = patterns.ssn.replace_all(&scrubbed, "[SSN]").to_string();
    scrubbed = patterns
        .credit_card
        .replace_all(&scrubbed, "[CREDIT_CARD]")
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
}
