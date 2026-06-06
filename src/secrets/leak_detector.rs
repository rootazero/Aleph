//! Bidirectional secret leak detection.
//!
//! Scans outbound requests and inbound responses for leaked secret values.
//! Uses two detection strategies:
//! 1. Pattern rules - known secret formats (sk-ant-*, AKIA*, etc.)
//! 2. Exact value detection - substring match of recently injected secrets

use std::hash::{Hash, Hasher};

use once_cell::sync::Lazy;
use regex::Regex;

use super::injection::InjectedSecret;

/// Result of a leak scan.
#[derive(Debug, Clone)]
pub enum LeakDecision {
    /// Content is safe to proceed.
    Allow,
    /// Content contains a leaked secret and must be blocked.
    Block {
        reason: String,
        redacted_content: String,
    },
}

impl LeakDecision {
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}

/// Source-of-truth secret regex strings for the byte-level sandbox scrubber.
///
/// NOTE: This list is intentionally independent from the `LEAK_PATTERNS`
/// below. The str-side detector pre-dates this list and uses different pattern
/// boundaries. Refactoring `LEAK_PATTERNS` to share this list would change
/// existing redaction behavior and break tests — kept separate by design.
///
/// Ordering matters: the specific prefixes (`sk_proj`/`sk_ant`) run before the
/// generic `openai_sk` catch-all so they win the named redaction tag; once a
/// match is redacted to `[REDACTED:NAME]` the generic rule no longer matches
/// it. The trailing three entries (generic OpenAI key, Google API key, PEM
/// private-key header) mirror the high-confidence `LEAK_PATTERNS` so that
/// sandbox stdout/stderr is scrubbed of the same secrets the str-side detector
/// blocks on the network path — closing a previously open leak path where a
/// command printing a classic `sk-…`, `AIza…`, or PEM key to stdout was
/// returned to the model un-redacted.
pub const SECRET_PATTERN_SOURCES: &[(&str, &str)] = &[
    ("sk_proj", r"sk-proj-[A-Za-z0-9_\-]{20,}"),
    ("sk_ant", r"sk-ant-[A-Za-z0-9_\-]{20,}"),
    ("aws_akia", r"AKIA[0-9A-Z]{16}"),
    ("github_pat", r"ghp_[A-Za-z0-9]{20,}"),
    ("gitlab_pat", r"glpat-[A-Za-z0-9_\-]{20,}"),
    ("openai_sk", r"sk-[a-zA-Z0-9\-]{20,}"),
    ("google_api", r"AIza[a-zA-Z0-9_\-]{35}"),
    ("private_key", r"-----BEGIN [A-Z ]+ PRIVATE KEY-----"),
];

/// Catastrophic ("block-class") secret pattern names — a curated subset of
/// [`SECRET_PATTERN_SOURCES`] whose appearance in **sandboxed command output**
/// is treated as fail-closed: the output is refused rather than merely redacted.
///
/// This is the shell-output analogue of clawshell's `DlpAction::Block` (versus
/// the default `Redact`). It is intentionally minimal — limited to categories
/// that have essentially no legitimate reason to be echoed to the model and a
/// near-zero false-positive rate. A PEM `PRIVATE KEY` block in command stdout
/// means a key file is being dumped; redacting the literal bytes still returns
/// the surrounding context to the model, so the worst class fails closed.
///
/// API-token shapes (`sk-…`, `ghp_…`, `AKIA…`) are deliberately **excluded** —
/// they can legitimately surface in `env`/config inspection and are handled by
/// redaction so as not to break ordinary workflows. Like risk.rs
/// `BLOCKED_PATTERNS`, this is a frozen hard-filter floor, not a config knob.
pub const BLOCK_CLASS_SECRETS: &[&str] = &["private_key"];

/// Whether a named secret pattern (from [`SECRET_PATTERN_SOURCES`]) is
/// block-class. Cheap linear scan over the tiny frozen [`BLOCK_CLASS_SECRETS`].
pub fn is_block_class_secret(name: &str) -> bool {
    BLOCK_CLASS_SECRETS.contains(&name)
}

/// Produce bytes-flavored regexes matching the same patterns as
/// `SECRET_PATTERN_SOURCES`, plus the shared high-confidence vendor catalog in
/// [`super::vendor_patterns::VENDOR_SECRET_PATTERNS`]. Used by `sandbox::scrub`
/// to redact raw stdout/stderr before any UTF-8 conversion.
///
/// The legacy `SECRET_PATTERN_SOURCES` entries run first so their named
/// redaction tags win for inputs they already matched (byte-identical scrub for
/// pre-existing patterns); the vendor catalog only widens coverage.
pub fn default_patterns_bytes() -> Vec<(&'static str, regex::bytes::Regex)> {
    SECRET_PATTERN_SOURCES
        .iter()
        .chain(super::vendor_patterns::VENDOR_SECRET_PATTERNS.iter())
        .map(|(name, src)| {
            (
                *name,
                regex::bytes::Regex::new(src).expect("static pattern compiles"),
            )
        })
        .collect()
}

/// Known secret format patterns.
///
/// The legacy high-confidence entries below run first; the shared vendor
/// catalog ([`super::vendor_patterns::VENDOR_SECRET_PATTERNS`]) is appended so
/// the network egress guard blocks the same distinctive vendor credentials the
/// byte-level sandbox scrubber redacts. Inputs the legacy entries already
/// matched keep byte-identical labels (first match wins the redaction tag).
static LEAK_PATTERNS: Lazy<Vec<(&str, Regex)>> = Lazy::new(|| {
    let mut patterns = vec![
        (
            "Anthropic API Key",
            Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").expect("static Anthropic pattern compiles"),
        ),
        (
            "OpenAI API Key",
            Regex::new(r"sk-[a-zA-Z0-9\-]{20,}").expect("static OpenAI pattern compiles"),
        ),
        (
            "Google API Key",
            Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").expect("static Google pattern compiles"),
        ),
        (
            "AWS Access Key",
            Regex::new(r"AKIA[A-Z0-9]{16}").expect("static AWS pattern compiles"),
        ),
        (
            "GitHub Token",
            Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").expect("static GitHub pattern compiles"),
        ),
        (
            // GitLab personal/group/project access tokens — mirrors SECRET_PATTERN_SOURCES entry
            "GitLab Token",
            Regex::new(r"glpat-[A-Za-z0-9_\-]{20,}").expect("static GitLab pattern compiles"),
        ),
        (
            "Private Key Block",
            Regex::new(r"-----BEGIN [A-Z ]+ PRIVATE KEY-----")
                .expect("static private key pattern compiles"),
        ),
    ];
    patterns.extend(
        super::vendor_patterns::VENDOR_SECRET_PATTERNS
            .iter()
            .map(|(label, src)| {
                (
                    *label,
                    Regex::new(src).expect("static vendor pattern compiles"),
                )
            }),
    );
    patterns
});

/// Compiled custom leak pattern for runtime use.
struct CompiledCustomPattern {
    name: String,
    regex: Regex,
}

/// Bidirectional leak detector for secret values.
pub struct LeakDetector {
    injected_hashes: std::collections::HashSet<u64>,
    injected_values: Vec<String>,
    custom_patterns: Vec<CompiledCustomPattern>,
}

impl LeakDetector {
    pub fn new() -> Self {
        Self {
            injected_hashes: std::collections::HashSet::new(),
            injected_values: Vec::new(),
            custom_patterns: Vec::new(),
        }
    }

    /// Create a leak detector with custom patterns from config.
    ///
    /// Built-in patterns are always active. Custom patterns are additive.
    /// Invalid regex patterns are logged and skipped.
    pub fn with_custom_patterns(custom: &[crate::config::types::CustomLeakPattern]) -> Self {
        let mut detector = Self::new();
        for pattern in custom {
            match crate::security::safe_regex::bounded_builder(&pattern.pattern).build() {
                Ok(regex) => detector.custom_patterns.push(CompiledCustomPattern {
                    name: pattern.name.clone(),
                    regex,
                }),
                Err(e) => {
                    tracing::warn!(
                        name = %pattern.name,
                        pattern = %pattern.pattern,
                        error = %e,
                        "Skipping invalid custom leak pattern"
                    );
                }
            }
        }
        detector
    }

    /// Register secrets that were injected in the current request.
    ///
    /// Only values with length >= 8 are tracked for exact substring matching.
    /// Shorter values are skipped to reduce false positives with common words.
    pub fn register_injected(&mut self, secrets: &[InjectedSecret], values: &[&str]) {
        for secret in secrets {
            self.injected_hashes.insert(secret.value_hash);
        }
        for value in values {
            if value.len() >= 8 {
                self.injected_values.push(value.to_string());
            }
        }
    }

    /// Scan content for known secret patterns (built-in + custom).
    ///
    /// Returns `(found_labels, redacted_content)`. Labels are borrowed from
    /// `LEAK_PATTERNS` (static) or `self.custom_patterns` (owned), hence the
    /// mixed lifetime `Vec<&str>`.
    fn scan_patterns<'a>(&'a self, content: &str) -> (Vec<&'a str>, String) {
        let mut redacted = content.to_string();
        let mut found_labels = Vec::new();

        // Check built-in patterns first
        for (label, pattern) in LEAK_PATTERNS.iter() {
            if pattern.is_match(&redacted) {
                found_labels.push(*label);
                redacted = pattern
                    .replace_all(&redacted, "***LEAKED_REDACTED***")
                    .to_string();
            }
        }

        // Check custom patterns (additive)
        for pattern in &self.custom_patterns {
            if pattern.regex.is_match(&redacted) {
                found_labels.push(&pattern.name);
                redacted = pattern
                    .regex
                    .replace_all(&redacted, "***LEAKED_REDACTED***")
                    .to_string();
            }
        }

        (found_labels, redacted)
    }

    /// Scan outbound content for known secret patterns.
    pub fn scan_outbound(&self, content: &str) -> LeakDecision {
        let (found_labels, redacted) = self.scan_patterns(content);

        if found_labels.is_empty() {
            LeakDecision::Allow
        } else {
            LeakDecision::Block {
                reason: format!("Outbound leak detected: {}", found_labels.join(", ")),
                redacted_content: redacted,
            }
        }
    }

    /// Scan inbound content for echoed secret values.
    pub fn scan_inbound(&self, content: &str) -> LeakDecision {
        let (found_labels, redacted) = self.scan_patterns(content);

        if !found_labels.is_empty() {
            return LeakDecision::Block {
                reason: format!("Inbound leak detected: {}", found_labels.join(", ")),
                redacted_content: redacted,
            };
        }

        // Check exact injected value matches
        for injected_value in &self.injected_values {
            if content.contains(injected_value.as_str()) {
                let redacted = content.replace(injected_value.as_str(), "***INJECTED_REDACTED***");
                return LeakDecision::Block {
                    reason: "Inbound response echoed an injected secret value".to_string(),
                    redacted_content: redacted,
                };
            }
        }

        // Check hash-based detection for content fragments
        for word in content.split_whitespace() {
            if word.len() >= 8 {
                // Use same keys as InjectedSecret::from_value
                let mut hasher = siphasher::sip::SipHasher::new_with_keys(
                    0x517c_c1b7_2722_0a95,
                    0x6c62_272e_07bb_0142,
                );
                word.hash(&mut hasher);
                let hash = hasher.finish();
                if self.injected_hashes.contains(&hash) {
                    return LeakDecision::Block {
                        reason: "Inbound response contains hash-matched injected secret"
                            .to_string(),
                        redacted_content: content.replace(word, "***HASH_MATCHED_REDACTED***"),
                    };
                }
            }
        }

        LeakDecision::Allow
    }

    /// Clear all tracked injected secrets.
    pub fn clear(&mut self) {
        self.injected_hashes.clear();
        self.injected_values.clear();
    }
}

impl Default for LeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outbound_blocks_known_api_key() {
        let detector = LeakDetector::new();
        let content = "Use this key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let decision = detector.scan_outbound(content);
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Anthropic API Key"));
        }
    }

    #[test]
    fn test_outbound_allows_normal_content() {
        let detector = LeakDetector::new();
        let content = "Please search for 'rust async programming'";
        let decision = detector.scan_outbound(content);
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_inbound_blocks_echoed_injected_value() {
        let mut detector = LeakDetector::new();
        let injected = InjectedSecret {
            name: "my_key".to_string(),
            value_hash: {
                let mut h = siphasher::sip::SipHasher::new_with_keys(
                    0x517c_c1b7_2722_0a95,
                    0x6c62_272e_07bb_0142,
                );
                "sk-ant-my-super-secret-key-12345678".hash(&mut h);
                h.finish()
            },
            value_len: 35,
        };
        detector.register_injected(&[injected], &["sk-ant-my-super-secret-key-12345678"]);

        let response = "Your API key is sk-ant-my-super-secret-key-12345678, stored.";
        let decision = detector.scan_inbound(response);
        assert!(decision.is_blocked());
    }

    #[test]
    fn test_inbound_allows_safe_response() {
        let mut detector = LeakDetector::new();
        let injected = InjectedSecret {
            name: "key".to_string(),
            value_hash: 12345,
            value_len: 20,
        };
        detector.register_injected(&[injected], &["some-long-secret-value-here"]);

        let response = "Request processed successfully. Status: 200 OK.";
        let decision = detector.scan_inbound(response);
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_inbound_blocks_known_pattern_even_without_injection() {
        let detector = LeakDetector::new();
        let response = "Here's a token: sk-proj-abcdefghijklmnopqrstuvwxyz12345678";
        let decision = detector.scan_inbound(response);
        assert!(decision.is_blocked());
    }

    #[test]
    fn test_clear_resets_state() {
        let mut detector = LeakDetector::new();
        detector.register_injected(
            &[InjectedSecret {
                name: "k".to_string(),
                value_hash: 999,
                value_len: 10,
            }],
            &["abcdefghij"],
        );
        assert!(!detector.injected_hashes.is_empty());
        assert!(!detector.injected_values.is_empty());

        detector.clear();
        assert!(detector.injected_hashes.is_empty());
        assert!(detector.injected_values.is_empty());
    }

    #[test]
    fn test_redacted_content_in_block_decision() {
        let detector = LeakDetector::new();
        let content = "Key: sk-abcdefghijklmnopqrstuvwxyz123456789012345678";
        if let LeakDecision::Block {
            redacted_content, ..
        } = detector.scan_outbound(content)
        {
            assert!(redacted_content.contains("***LEAKED_REDACTED***"));
            assert!(!redacted_content.contains("abcdefgh"));
        } else {
            panic!("Expected Block");
        }
    }

    #[test]
    fn test_short_values_not_tracked() {
        let mut detector = LeakDetector::new();
        detector.register_injected(&[], &["short"]);
        assert!(detector.injected_values.is_empty());
    }

    #[test]
    fn test_custom_pattern_blocks_outbound() {
        use crate::config::types::CustomLeakPattern;

        let custom = vec![CustomLeakPattern {
            name: "Internal Token".to_string(),
            pattern: r"internal-[a-z0-9]{8}".to_string(),
        }];
        let detector = LeakDetector::with_custom_patterns(&custom);

        let decision = detector.scan_outbound("Token: internal-abc12345");
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Internal Token"));
        }
    }

    #[test]
    fn test_custom_pattern_blocks_inbound() {
        use crate::config::types::CustomLeakPattern;

        let custom = vec![CustomLeakPattern {
            name: "Service Key".to_string(),
            pattern: r"svc-[A-Z]{6}".to_string(),
        }];
        let detector = LeakDetector::with_custom_patterns(&custom);

        let decision = detector.scan_inbound("Key: svc-ABCDEF");
        assert!(decision.is_blocked());
    }

    #[test]
    fn test_outbound_blocks_vendor_slack_token() {
        let detector = LeakDetector::new();
        let decision = detector.scan_outbound("SLACK_TOKEN=xoxb-1234567890-abcdefghijklmnop");
        assert!(
            decision.is_blocked(),
            "slack bot token should block on egress"
        );
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Slack Token"));
        }
    }

    #[test]
    fn test_outbound_blocks_vendor_huggingface_token() {
        let detector = LeakDetector::new();
        let decision = detector.scan_outbound("export HF=hf_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(
            decision.is_blocked(),
            "hugging face token should block on egress"
        );
    }

    #[test]
    fn test_outbound_blocks_vendor_stripe_key() {
        let detector = LeakDetector::new();
        let decision = detector.scan_outbound("key: sk_live_abcdefghijklmnopqrstuvwx1234");
        assert!(
            decision.is_blocked(),
            "stripe secret key should block on egress"
        );
    }

    #[test]
    fn test_vendor_patterns_redact_in_byte_scrubber() {
        // The shared vendor catalog must also feed the byte-level scrubber so
        // sandbox stdout is scrubbed of the same secrets the egress guard blocks.
        let patterns = default_patterns_bytes();
        let has_groq = patterns.iter().any(|(name, _)| *name == "Groq API Key");
        assert!(
            has_groq,
            "vendor catalog should be wired into byte scrubber"
        );
        let groq = patterns
            .iter()
            .find(|(name, _)| *name == "Groq API Key")
            .map(|(_, re)| re)
            .unwrap();
        assert!(groq.is_match(b"gsk_abcdefghijklmnopqrstuvwxyz0123456789ABCD"));
    }

    #[test]
    fn test_outbound_allows_non_vendor_prefixed_text() {
        // Distinctive prefixes must not over-block ordinary prose / identifiers.
        let detector = LeakDetector::new();
        let decision =
            detector.scan_outbound("The sky was clear and the report listed 42 findings.");
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_custom_pattern_invalid_regex_skipped() {
        use crate::config::types::CustomLeakPattern;

        let custom = vec![
            CustomLeakPattern {
                name: "Invalid".to_string(),
                pattern: "[invalid".to_string(),
            },
            CustomLeakPattern {
                name: "Valid".to_string(),
                pattern: r"test-\d{4}".to_string(),
            },
        ];
        let detector = LeakDetector::with_custom_patterns(&custom);

        // Invalid pattern is skipped, but valid one works
        let decision = detector.scan_outbound("Code: test-1234");
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Valid"));
        }
    }
}
