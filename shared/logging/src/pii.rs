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
    /// Generic `key = value` / `key: value` credential assignment, vendor-agnostic.
    generic_secret: Regex,
    china_mobile: Regex,
    china_id: Regex,
    bank_card: Regex,
}

/// Global PII patterns (lazy-initialized)
static PII_PATTERNS: OnceLock<PiiPatterns> = OnceLock::new();

/// Get or initialize PII patterns
fn get_patterns() -> &'static PiiPatterns {
    PII_PATTERNS.get_or_init(|| PiiPatterns {
        email: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b").expect("static PII regex is valid"),
        phone: Regex::new(r"\b(\+?1?[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b").expect("static PII regex is valid"),
        ssn: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").expect("static PII regex is valid"),
        credit_card: Regex::new(r"\b\d{4}[-\s]?\d{4}[-\s]?\d{4}[-\s]?\d{4}\b").expect("static PII regex is valid"),
        // `Basic\s+<b64>` was previously swallowed as plain `Basic` by the
        // generic_secret regex (its [^\s,}]+ value cap stops at whitespace),
        // so the entire base64 blob leaked past scrubbing. Catch it here so
        // the value is fully redacted before generic_secret runs (which then
        // does no work for Authorization: Basic ...).
        // Trailing `\b` is NOT used uniformly. For short Base64 credentials
        // like `Basic dGVzdDE=`, the final `=` is a non-word character and
        // would otherwise terminate the match BEFORE the padding. We split
        // body and padding so `\b` can land between them: the body ends
        // when the next char is non-base64 (`\b` fires there), then `=*`
        // sweeps any trailing padding without re-imposing a word boundary
        // that would reject the credential at the very end.
        //
        // Note: `regex` does not support look-around (no `(?!...)`), which
        // is why we model the boundary via `\b` instead of a negative
        // lookahead.
        // For Basic auth credentials (which use Base64 padding `=`), the
        // body char class intentionally EXCLUDES `=`, so trailing `=` chars
        // are not absorbed into the body. `\b` then fires between the body
        // and the padding (body char is `\w`, `=` is `\W`), and `=*`
        // sweeps any padding. The trailing `\b` after `=*` is omitted on
        // purpose: `=` is a non-word character followed by the string end
        // or whitespace, neither of which is a `\b`, and imposing one would
        // reject the very credential we are trying to catch. The body
        // length of 8 still bounds the match.
        api_key: Regex::new(r"\b(sk-[a-zA-Z0-9\-_]{20,}|sk-ant-[a-zA-Z0-9\-_]{20,}|tvly-[a-zA-Z0-9\-_]{20,}|xai-[a-zA-Z0-9\-_]{20,}|AIza[a-zA-Z0-9\-_]{30,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|Bearer\s+[a-zA-Z0-9._\-]{8,}|Basic\s+[A-Za-z0-9+/]{8,}=*)").expect("static PII regex is valid"),
        // Catch arbitrary secrets assigned to a credential-like key, regardless of
        // vendor prefix: `password=...`, `api_key: ...`, `token = "..."`, etc.
        // Preserves the key name; redacts only the value. Conservative over-match
        // is acceptable (this module favours false positives over leaks).
        // `[_\-\s]?` (not just `[_-]?`) lets the alternation also match
        // `api key=...` with a literal space, so vendor terminology like
        // `Authorization: api key <value>` is caught.
        //
        // The value arm matches double- or single-quoted strings (including any
        // spaces inside the quotes) so a value like `password = "my secret phrase"`
        // is fully redacted, not just the first word.
        generic_secret: Regex::new(
            r#"(?i)(password|passwd|pwd|secret|token|api[_\-\s]?key|access[_\-\s]?token|authorization)(\s*[:=]\s*)("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^\s",}]+)"#,
        )
        .expect("static PII regex is valid"),
        china_mobile: Regex::new(r"\b1[3-9]\d{9}\b").expect("static PII regex is valid"),
        china_id: Regex::new(r"\b\d{17}[\dXx]\b").expect("static PII regex is valid"),
        // Covers major card networks: Visa (4...), Mastercard (51-55...), Amex (34/37...), UnionPay (62...), Discover (6...)
        bank_card: Regex::new(r"\b(?:4\d{15}|5[1-5]\d{14}|3[47]\d{13}|6\d{15}|62\d{14,17})\b").expect("static PII regex is valid"),
    })
}

/// Scrub personally identifiable information from text
///
/// Replaces PII patterns with placeholder tokens:
/// - Email addresses → [EMAIL]
/// - Phone numbers → [PHONE]
/// - SSN/Tax IDs → [SSN]
/// - Credit card numbers → [`CREDIT_CARD`]
/// - API keys → [REDACTED]
/// - Chinese ID cards → [`ID_CARD`]
/// - Bank card numbers → [`BANK_CARD`]
#[must_use]
pub fn scrub_pii(text: &str) -> String {
    let patterns = get_patterns();
    let mut scrubbed = text.to_string();

    // Apply in order: more specific patterns first
    scrubbed = patterns
        .api_key
        .replace_all(&scrubbed, "[REDACTED]")
        .to_string();
    // Generic credential assignments (keeps the key and separator, redacts the
    // whole value — including any surrounding quotes and spaces inside quotes).
    scrubbed = patterns
        .generic_secret
        .replace_all(&scrubbed, "${1}${2}[REDACTED]")
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

    #[test]
    fn test_scrub_github_token() {
        let scrubbed = scrub_pii("token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"));
    }

    #[test]
    fn test_scrub_aws_access_key() {
        let scrubbed = scrub_pii("AKIAIOSFODNN7EXAMPLE in config");
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_scrub_generic_password_assignment() {
        let scrubbed = scrub_pii("db_password=hunter2supersecret host=localhost");
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("hunter2supersecret"));
        // Non-secret fields are untouched.
        assert!(scrubbed.contains("host=localhost"));
    }

    #[test]
    fn test_generic_secret_keeps_key_name() {
        let scrubbed = scrub_pii("secret: topsecretvalue");
        assert!(scrubbed.starts_with("secret"));
        assert!(scrubbed.contains("[REDACTED]"));
        assert!(!scrubbed.contains("topsecretvalue"));
    }

    #[test]
    fn test_generic_secret_redacts_quoted_multi_word_value() {
        let scrubbed = scrub_pii("password = \"my secret phrase\"");
        assert!(scrubbed.contains("password = [REDACTED]"), "got: {scrubbed}");
        assert!(!scrubbed.contains("my secret phrase"), "got: {scrubbed}");

        let scrubbed = scrub_pii("api_key='another secret value'");
        assert!(scrubbed.contains("api_key=[REDACTED]"), "got: {scrubbed}");
        assert!(!scrubbed.contains("another secret value"), "got: {scrubbed}");
    }

    #[test]
    fn test_scrub_basic_auth_padded_credential() {
        // Base64 credential padded with `=` previously slipped past the
        // body `\b` boundary in the api_key regex. Regression test: the
        // base64 blob MUST now be redacted. The body here is 8 base64
        // chars (dGVzdDEz) followed by one or two `=` padding chars.
        let scrubbed = scrub_pii("Authorization: Basic dGVzdDEz=");
        assert!(
            scrubbed.starts_with("Authorization: [REDACTED]"),
            "got: {scrubbed}"
        );
        assert!(!scrubbed.contains("dGVzdDEz="), "got: {scrubbed}");

        let scrubbed = scrub_pii("Authorization: Basic dGVzdDEz==");
        assert!(
            scrubbed.starts_with("Authorization: [REDACTED]"),
            "got: {scrubbed}"
        );
        assert!(!scrubbed.contains("dGVzdDEz=="), "got: {scrubbed}");
    }

    #[test]
    fn test_scrub_basic_auth_unpadded_credential() {
        // Unpadded Base64 credential must also be caught.
        let scrubbed = scrub_pii("Authorization: Basic dGVzdDEz");
        assert!(
            scrubbed.starts_with("Authorization: [REDACTED]"),
            "got: {scrubbed}"
        );
        assert!(!scrubbed.contains("dGVzdDEz"), "got: {scrubbed}");
    }

    #[test]
    fn test_scrub_bearer_token_at_string_end() {
        // Bearer token at end of string must still be caught.
        let scrubbed = scrub_pii("Authorization: Bearer abc123def");
        assert!(scrubbed.contains("[REDACTED]"), "got: {scrubbed}");
        assert!(!scrubbed.contains("abc123def"), "got: {scrubbed}");
    }
}
