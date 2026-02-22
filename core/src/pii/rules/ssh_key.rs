//! SSH private key detection
//!
//! Detects PEM-encoded private key headers. Matching the header line
//! is sufficient — the key material follows immediately in the same block.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static SSH_KEY_RE: OnceLock<Regex> = OnceLock::new();

fn ssh_key_regex() -> &'static Regex {
    SSH_KEY_RE.get_or_init(|| {
        Regex::new(
            r"-----BEGIN [A-Z ]*(?:PRIVATE KEY|RSA PRIVATE KEY|EC PRIVATE KEY|DSA PRIVATE KEY|OPENSSH PRIVATE KEY)-----",
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
    fn test_detect_rsa_private_key() {
        let text = "Here is my key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQ...\n-----END RSA PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_ec_private_key() {
        let matches = rule().detect("-----BEGIN EC PRIVATE KEY-----");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_openssh_private_key() {
        let matches = rule().detect("-----BEGIN OPENSSH PRIVATE KEY-----");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_generic_private_key() {
        let matches = rule().detect("-----BEGIN PRIVATE KEY-----");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_public_key() {
        let matches = rule().detect("-----BEGIN PUBLIC KEY-----");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_certificate() {
        let matches = rule().detect("-----BEGIN CERTIFICATE-----");
        assert_eq!(matches.len(), 0);
    }
}
