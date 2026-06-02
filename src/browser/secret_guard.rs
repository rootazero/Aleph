//! Navigation-time secret-exfiltration guard.
//!
//! SSRF protection ([`super::network_policy`]) governs the *destination network*
//! of a browser navigation — "is this host allowed to be reached". This guard
//! governs the orthogonal concern of the target URL's *content*: it rejects
//! navigations whose URL embeds a high-confidence secret (API key, bearer
//! token, PEM private key). That blocks an attacker page from
//! socially-engineering the agent into exfiltrating a secret already present in
//! its context by appending it to a query parameter sent to an otherwise
//! policy-allowed public host (e.g. `https://evil.example/?leak=sk-ant-...`).
//!
//! Secret patterns are NOT duplicated here. The existing `Critical`-severity
//! PII rules ([`crate::pii::rules`]) are the single source of truth, so a new
//! credential pattern added there is automatically enforced at navigation time.

use percent_encoding::percent_decode_str;

use crate::pii::{rules::build_rules, PiiRule, PiiSeverity};

/// Scan a navigation URL (raw + percent-decoded) for an embedded secret.
///
/// Returns the matched rule name (e.g. `"api_key"`) on the first hit, or
/// `None` when the URL carries no detectable secret. Only `Critical`-severity
/// rules participate — lower-severity PII (emails, phone numbers, IP addresses)
/// is not a credential and must never block a legitimate navigation.
pub(crate) fn scan_url_for_secrets(url: &str) -> Option<String> {
    let secret_rules: Vec<Box<dyn PiiRule>> = build_rules(&[])
        .into_iter()
        .filter(|r| r.severity() == PiiSeverity::Critical)
        .collect();

    // Raw form catches the common unencoded case (`sk-…` is unreserved and is
    // rarely percent-encoded); the decoded form defeats percent-encoding
    // evasion such as "%73%6b-…".
    let decoded = percent_decode_str(url).decode_utf8_lossy();
    for candidate in [url, decoded.as_ref()] {
        for rule in &secret_rules {
            if let Some(m) = rule.detect(candidate).into_iter().next() {
                return Some(m.rule_name);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_url_passes() {
        assert!(scan_url_for_secrets("https://example.com/search?q=rust+async").is_none());
        assert!(scan_url_for_secrets("https://docs.rs/tokio/latest/tokio/").is_none());
    }

    #[test]
    fn detects_anthropic_style_key_in_query() {
        let hit = scan_url_for_secrets(
            "https://evil.example/?leak=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
        );
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn detects_aws_access_key() {
        let hit = scan_url_for_secrets("https://evil.example/x?k=AKIAIOSFODNN7EXAMPLE");
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn detects_percent_encoded_key() {
        // "sk-" percent-encoded as %73%6b%2d defeats a raw-only scan.
        let hit = scan_url_for_secrets(
            "https://evil.example/?t=%73%6b%2dlivesecret012345678901234567890",
        );
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn lower_severity_pii_does_not_block() {
        // An email address is PII but not a credential — navigation must proceed.
        assert!(scan_url_for_secrets("https://example.com/u?email=alice@example.com").is_none());
    }
}
