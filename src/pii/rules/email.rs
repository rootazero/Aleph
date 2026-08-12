//! Email address detection

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static EMAIL_RE: OnceLock<Regex> = OnceLock::new();

fn email_regex() -> &'static Regex {
    EMAIL_RE.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}")
            // rust-doctor-disable-next-line unwrap-in-production
            .expect("static email regex compiles")
    })
}

pub struct EmailRule;

impl EmailRule {
    pub const fn new() -> Self {
        Self
    }

    /// Check word boundary: the match should not be part of a longer
    /// alphanumeric sequence to avoid matching substrings inside file paths
    /// or other non-email tokens.
    fn has_word_boundary(text: &str, start: usize, _end: usize) -> bool {
        // Only guard the start boundary. An email directly followed by an
        // alphanumeric char (a digit after the TLD, e.g. `john@host.com1` — the
        // greedy `[A-Za-z]{2,}` TLD already consumed every trailing letter) is
        // still PII: failing an after-boundary check there dropped the match
        // entirely and leaked the address in plaintext.
        start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric()
    }
}

impl PiiRule for EmailRule {
    fn name(&self) -> &str {
        "email"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Medium
    }
    fn placeholder(&self) -> &str {
        "[EMAIL]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = email_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            let start = m.start();
            let end = m.end();
            if !Self::has_word_boundary(text, start, end) {
                continue;
            }
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start,
                end,
                matched_text: m.as_str().to_string(),
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

    fn rule() -> EmailRule {
        EmailRule::new()
    }

    #[test]
    fn test_detect_simple_email() {
        let matches = rule().detect("Contact user@example.com for help");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "user@example.com");
    }

    #[test]
    fn test_detect_email_with_plus() {
        let matches = rule().detect("Email: user+tag@domain.co.uk");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_missing_at() {
        let matches = rule().detect("notanemail.com");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_missing_tld() {
        let matches = rule().detect("user@domain");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_match_email_followed_by_alnum() {
        // An email directly followed by an alphanumeric char (a digit past the
        // TLD) is still PII and must be redacted, not dropped — dropping it
        // previously leaked the address in plaintext.
        let matches = rule().detect("Contact user@example.com1z");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "user@example.com");
    }

    #[test]
    fn test_match_mixed_case_email() {
        // Regression: the email regex accepts mixed case local parts and
        // domains; the allowlist in `allowlist.rs` uses `(?i)` patterns so
        // suffix-based allow entries (e.g. `.com`, `.example`) still match
        // `User@Example.COM`. This test guards against future case-fold
        // regressions in either layer.
        let matches = rule().detect("Contact User@Example.COM");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "User@Example.COM");
    }
}
