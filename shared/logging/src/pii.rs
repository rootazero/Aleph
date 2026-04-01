/// PII (Personally Identifiable Information) scrubbing utilities
///
/// Removes sensitive information from text before logging or storage.
/// All scrubbing is local and conservative (false positives are acceptable).
use regex::Regex;
use std::sync::OnceLock;

/// PII scrubbing regex patterns (compiled once for performance)
struct PiiPatterns {
    email: Regex,
    phone: Regex,
    ssn: Regex,
    credit_card: Regex,
    api_key: Regex,
    china_mobile: Regex,
    china_id: Regex,
    bank_card: Regex,
}

/// Global PII patterns (lazy-initialized)
static PII_PATTERNS: OnceLock<PiiPatterns> = OnceLock::new();

/// Get or initialize PII patterns
fn get_patterns() -> &'static PiiPatterns {
    PII_PATTERNS.get_or_init(|| PiiPatterns {
        email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").unwrap(),
        phone: Regex::new(r"\b(\+?1?[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").unwrap(),
        ssn: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap(),
        credit_card: Regex::new(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b").unwrap(),
        api_key: Regex::new(r"\b(sk-[a-zA-Z0-9\-_]{20,}|sk-ant-[a-zA-Z0-9\-_]{20,}|tvly-[a-zA-Z0-9\-_]{20,}|xai-[a-zA-Z0-9\-_]{20,}|AIza[a-zA-Z0-9\-_]{30,}|Bearer\s+[a-zA-Z0-9._\-]{20,})\b").unwrap(),
        china_mobile: Regex::new(r"\b1[3-9]\d{9}\b").unwrap(),
        china_id: Regex::new(r"\b\d{17}[\dXx]\b").unwrap(),
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
/// - Chinese ID cards → [ID_CARD]
/// - Bank card numbers → [BANK_CARD]
pub fn scrub_pii(text: &str) -> String {
    let patterns = get_patterns();
    let mut scrubbed = text.to_string();

    // Apply in order: more specific patterns first
    scrubbed = patterns
        .api_key
        .replace_all(&scrubbed, "[REDACTED]")
        .to_string();
    scrubbed = patterns
        .china_id
        .replace_all(&scrubbed, "[ID_CARD]")
        .to_string();
    scrubbed = patterns.email.replace_all(&scrubbed, "[EMAIL]").to_string();
    scrubbed = patterns
        .china_mobile
        .replace_all(&scrubbed, "[PHONE]")
        .to_string();
    scrubbed = patterns.phone.replace_all(&scrubbed, "[PHONE]").to_string();
    scrubbed = patterns.ssn.replace_all(&scrubbed, "[SSN]").to_string();
    scrubbed = patterns
        .credit_card
        .replace_all(&scrubbed, "[CREDIT_CARD]")
        .to_string();
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
        let scrubbed = scrub_pii("Contact me at john.doe@example.com");
        assert_eq!(scrubbed, "Contact me at [EMAIL]");
    }

    #[test]
    fn test_scrub_phone() {
        let scrubbed = scrub_pii("Call 123-456-7890");
        assert!(scrubbed.contains("[PHONE]"));
        assert!(!scrubbed.contains("123-456-7890"));
    }

    #[test]
    fn test_scrub_api_key() {
        let scrubbed = scrub_pii("Key: sk-proj1234567890abcdefghijklmnopqrstuvwxyz");
        assert_eq!(scrubbed, "Key: [REDACTED]");
    }

    #[test]
    fn test_scrub_china_mobile() {
        let scrubbed = scrub_pii("Phone: 13812345678");
        assert_eq!(scrubbed, "Phone: [PHONE]");
    }

    #[test]
    fn test_no_pii() {
        let text = "Normal text with no PII.";
        assert_eq!(scrub_pii(text), text);
    }
}
