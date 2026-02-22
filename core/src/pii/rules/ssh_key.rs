//! SSH private key detection
//!
//! Detects full PEM-encoded private key blocks (from BEGIN header to END footer),
//! ensuring that the base64-encoded key body is also captured and redacted.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static SSH_KEY_RE: OnceLock<Regex> = OnceLock::new();

fn ssh_key_regex() -> &'static Regex {
    SSH_KEY_RE.get_or_init(|| {
        // Match the full PEM block from BEGIN to END, including key body.
        // (?s) enables dot-matches-newline so .* spans across lines.
        Regex::new(
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
        )
        .unwrap()
    })
}

pub struct SshKeyRule;

impl SshKeyRule {
    pub fn new() -> Self {
        Self
    }
}

impl PiiRule for SshKeyRule {
    fn name(&self) -> &str {
        "ssh_key"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Critical
    }
    fn placeholder(&self) -> &str {
        "[SSH_KEY]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = ssh_key_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start: m.start(),
                end: m.end(),
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

    fn rule() -> SshKeyRule {
        SshKeyRule::new()
    }

    #[test]
    fn test_detect_rsa_private_key_full_block() {
        let text = "Here is my key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQ...\n-----END RSA PRIVATE KEY-----\nDone.";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1);
        // Must capture the entire PEM block, not just the header
        assert!(matches[0].matched_text.contains("MIIEowIBAAKCAQ"));
        assert!(matches[0]
            .matched_text
            .ends_with("-----END RSA PRIVATE KEY-----"));
    }

    #[test]
    fn test_detect_ec_private_key() {
        let text =
            "-----BEGIN EC PRIVATE KEY-----\nbase64data\n-----END EC PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].matched_text.contains("base64data"));
    }

    #[test]
    fn test_detect_openssh_private_key() {
        let text = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk...\n-----END OPENSSH PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].matched_text.contains("b3BlbnNzaC1rZXk"));
    }

    #[test]
    fn test_detect_generic_private_key() {
        let text =
            "-----BEGIN PRIVATE KEY-----\nMIIEvAIBADANBg...\n-----END PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].matched_text.contains("MIIEvAIBADANBg"));
    }

    #[test]
    fn test_header_only_no_match() {
        // A header without a corresponding END footer should NOT match
        let matches = rule().detect("-----BEGIN RSA PRIVATE KEY-----");
        assert_eq!(
            matches.len(),
            0,
            "Header-only without END footer should not match"
        );
    }

    #[test]
    fn test_no_match_public_key() {
        let matches = rule().detect("-----BEGIN PUBLIC KEY-----\ndata\n-----END PUBLIC KEY-----");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_certificate() {
        let matches =
            rule().detect("-----BEGIN CERTIFICATE-----\ndata\n-----END CERTIFICATE-----");
        assert_eq!(matches.len(), 0);
    }
}
