//! SSH private key detection
//!
//! Detects full PEM-encoded private key blocks (from BEGIN header to END footer),
//! ensuring that the base64-encoded key body is also captured and redacted.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static SSH_KEY_BEGIN_RE: OnceLock<Regex> = OnceLock::new();

/// Matches only the PEM **header**. The matching footer is located in code by
/// [`SshKeyRule::detect`], not by the regex.
///
/// The label equality between `BEGIN <label> KEY` and `END <label> KEY` is a
/// back-reference, and the `regex` crate does not support back-references —
/// it rejects `\1` at *parse* time, inside this `OnceLock`, so the previous
/// spelling did not fail to match: it panicked on the first PII scan the
/// process ever ran. Nothing in the toolchain sees it, because the pattern is
/// a string literal (`cargo check`, `clippy`, and rustc are all green on a
/// regex that cannot be built). Do not reintroduce `\1` here; if the pairing
/// rule ever needs to grow, grow it in [`SshKeyRule::detect`] where it is
/// ordinary Rust the compiler can read.
fn ssh_key_begin_regex() -> &'static Regex {
    SSH_KEY_BEGIN_RE.get_or_init(|| {
        Regex::new(r"-----BEGIN ([A-Z ]*PRIVATE) KEY-----")
            // rust-doctor-disable-next-line unwrap-in-production
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

    /// Pairs each `BEGIN <label> KEY` header with the `END <label> KEY` footer
    /// carrying the **same** label, so a concatenated bundle whose labels do
    /// not pair (`cat rsa.pem ec.pem` truncated mid-file) is never accepted as
    /// one block spanning unrelated keys.
    ///
    /// A header with no matching footer yields no match, and scanning resumes
    /// just past that header — a truncated key at the top of a log must not
    /// hide a well-formed one below it.
    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = ssh_key_begin_regex();
        let mut results = Vec::new();
        let mut cursor = 0usize;

        while let Some(caps) = re.captures_at(text, cursor) {
            // Group 0 is always present on a successful capture.
            let Some(header) = caps.get(0) else { break };
            let label = caps.get(1).map_or("", |m| m.as_str());
            let footer = format!("-----END {label} KEY-----");
            let Some(offset) = text[header.end()..].find(&footer) else {
                cursor = header.end();
                continue;
            };
            let end = header.end() + offset + footer.len();
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start: header.start(),
                end,
                matched_text: text[header.start()..end].to_string(),
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
        // BEGIN/END labels must pair. A concatenated bundle with mismatched
        // labels — e.g. `cat rsa.pem ec.pem` producing `... END RSA PRIVATE
        // KEY ... BEGIN EC PRIVATE KEY ...` — must NOT be accepted as a
        // single key block.
        let text = "-----BEGIN RSA PRIVATE KEY-----\nrsadata\n-----END EC PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(
            matches.len(),
            0,
            "mismatched BEGIN/END labels must not match"
        );
    }

    /// The pattern this rule is built from lives in a string literal, so no
    /// part of the toolchain reads it: `cargo check`, `clippy` and rustc were
    /// all green while the pattern carried a back-reference the `regex` crate
    /// rejects at parse time. The failure was therefore not "no match" but a
    /// panic inside the `OnceLock`, on the first PII scan the process ran —
    /// which took 114 tests across `browser::*`, `pii::*`, `guardrails::*` and
    /// `security::runtime_guard` down with it, none of them owned by this file.
    ///
    /// This test exists to make building the regex a thing that fails *here*,
    /// by name, instead of everywhere else.
    #[test]
    fn the_header_pattern_compiles() {
        assert!(
            ssh_key_begin_regex().is_match("-----BEGIN OPENSSH PRIVATE KEY-----"),
            "the header pattern must build and match a real PEM header"
        );
    }

    /// The label-pairing rule must survive a footer of the *wrong* label
    /// appearing before the right one — the shape a truncated bundle takes.
    /// A regex alone cannot do this without back-references, which is why the
    /// pairing is ordinary Rust in `detect`.
    #[test]
    fn a_wrong_label_footer_does_not_hide_the_right_one() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nrsadata\n-----END EC PRIVATE KEY-----\nmore\n-----END RSA PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1, "the RSA block closes at the RSA footer");
        assert!(matches[0]
            .matched_text
            .ends_with("-----END RSA PRIVATE KEY-----"));
        assert!(
            matches[0].matched_text.contains("rsadata"),
            "the key body must be inside the redacted span"
        );
    }

    /// Scanning must resume past an unterminated header, or one truncated key
    /// at the top of a log conceals every well-formed key below it.
    #[test]
    fn an_unterminated_header_does_not_swallow_a_later_block() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\ntruncated\n\n-----BEGIN EC PRIVATE KEY-----\necdata\n-----END EC PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].matched_text.contains("ecdata"));
    }

    /// Two well-formed blocks in one payload are two separate matches, so
    /// redaction cannot collapse them into one span and leak the text between.
    #[test]
    fn two_well_formed_blocks_are_two_matches() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nrsadata\n-----END RSA PRIVATE KEY-----\nBETWEEN\n-----BEGIN EC PRIVATE KEY-----\necdata\n-----END EC PRIVATE KEY-----";
        let matches = rule().detect(text);
        assert_eq!(matches.len(), 2);
        assert!(!matches[0].matched_text.contains("BETWEEN"));
        assert!(!matches[1].matched_text.contains("BETWEEN"));
    }
}
