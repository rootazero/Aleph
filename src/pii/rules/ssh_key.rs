//! SSH private key detection
//!
//! Detects full PEM-encoded private key blocks (from BEGIN header to END footer),
//! ensuring that the base64-encoded key body is also captured and redacted.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static SSH_KEY_BEGIN_RE: OnceLock<Regex> = OnceLock::new();

/// Matches only the BEGIN header, capturing its label.
///
/// The END footer is **not** part of this pattern. The intent — a block whose
/// END label equals its BEGIN label, so a malformed bundle
/// (`BEGIN RSA … END EC …`) is not swallowed as one key — was originally
/// written as a back-reference (`\1`), which the `regex` crate does not
/// support: `Regex::new` returned `Syntax("backreferences are not supported")`
/// and the `expect` beside it panicked on **first use**. Since `detect` is on
/// the path every browser page read, every unattended output mask and every
/// PII scan takes, the rule crashed its caller rather than redacting anything.
///
/// Keeping the label comparison therefore means doing it outside the engine:
/// find a BEGIN, then look for the literal END built from that same label.
fn ssh_key_begin_regex() -> &'static Regex {
    SSH_KEY_BEGIN_RE.get_or_init(|| {
        // rust-doctor-disable-next-line unwrap-in-production
        Regex::new(r"-----BEGIN ([A-Z0-9 ]*PRIVATE) KEY-----")
            .expect("static SSH key regex compiles")
    })
}

pub struct SshKeyRule;

impl SshKeyRule {
    pub const fn new() -> Self {
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

    /// Full PEM blocks, from `BEGIN <label> KEY` to the *matching*
    /// `END <label> KEY`.
    ///
    /// Two phases rather than one pattern — see [`ssh_key_begin_regex`]. Scans
    /// forward from the end of each match so overlapping/nested headers cannot
    /// produce two matches over the same bytes; a BEGIN with no matching END is
    /// not a block and is skipped, which is the same verdict the original
    /// pattern gave it.
    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = ssh_key_begin_regex();
        let mut results = Vec::new();
        let mut cursor = 0usize;

        while let Some(caps) = re.captures_at(text, cursor) {
            let whole = caps.get(0).expect("group 0 always present");
            let label = caps.get(1).expect("group 1 always present").as_str();
            let footer = format!("-----END {label} KEY-----");
            let Some(rel) = text[whole.end()..].find(&footer) else {
                // No matching footer: not a block. Resume after this header so
                // a later, well-formed block is still found.
                cursor = whole.end();
                continue;
            };
            let end = whole.end() + rel + footer.len();
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start: whole.start(),
                end,
                matched_text: text[whole.start()..end].to_string(),
                severity: self.severity(),
                placeholder: self.placeholder().to_string(),
            });
            cursor = end;
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
        let text = "-----BEGIN EC PRIVATE KEY-----\nbase64data\n-----END EC PRIVATE KEY-----";
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
        let text = "-----BEGIN PRIVATE KEY-----\nMIIEvAIBADANBg...\n-----END PRIVATE KEY-----";
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
        let matches = rule().detect("-----BEGIN CERTIFICATE-----\ndata\n-----END CERTIFICATE-----");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_mismatched_begin_end_labels() {
        // BEGIN/END labels must match (back-reference). A concatenated
        // bundle with mismatched labels — e.g. `cat rsa.pem ec.pem`
        // producing `... END RSA PRIVATE KEY ... BEGIN EC PRIVATE KEY
        // ...` — must NOT be accepted as a single key block.
        let text = "-----BEGIN RSA PRIVATE KEY-----\nrsadata\n-----END EC PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(
            matches.len(),
            0,
            "mismatched BEGIN/END labels must not match (back-reference guard)"
        );
    }
}
