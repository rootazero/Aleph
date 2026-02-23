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

/// Known secret format patterns.
static LEAK_PATTERNS: Lazy<Vec<(&str, Regex)>> = Lazy::new(|| {
    vec![
        ("Anthropic API Key", Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").unwrap()),
        ("OpenAI API Key", Regex::new(r"sk-[a-zA-Z0-9\-]{20,}").unwrap()),
        ("Google API Key", Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").unwrap()),
        ("AWS Access Key", Regex::new(r"AKIA[A-Z0-9]{16}").unwrap()),
        ("GitHub Token", Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap()),
        ("Private Key Block", Regex::new(r"-----BEGIN [A-Z ]+ PRIVATE KEY-----").unwrap()),
    ]
});

/// Bidirectional leak detector for secret values.
pub struct LeakDetector {
    injected_hashes: std::collections::HashSet<u64>,
    injected_values: Vec<String>,
}

impl LeakDetector {
    pub fn new() -> Self {
        Self {
            injected_hashes: std::collections::HashSet::new(),
            injected_values: Vec::new(),
        }
    }

    /// Register secrets that were injected in the current request.
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

    /// Scan outbound content for known secret patterns.
    pub fn scan_outbound(&self, content: &str) -> LeakDecision {
        for (label, pattern) in LEAK_PATTERNS.iter() {
            if pattern.is_match(content) {
                let redacted = pattern.replace_all(content, "***LEAKED_REDACTED***");
                return LeakDecision::Block {
                    reason: format!("Outbound leak detected: {}", label),
                    redacted_content: redacted.to_string(),
                };
            }
        }
        LeakDecision::Allow
    }

    /// Scan inbound content for echoed secret values.
    pub fn scan_inbound(&self, content: &str) -> LeakDecision {
        // Check known patterns first
        for (label, pattern) in LEAK_PATTERNS.iter() {
            if pattern.is_match(content) {
                let redacted = pattern.replace_all(content, "***LEAKED_REDACTED***");
                return LeakDecision::Block {
                    reason: format!("Inbound leak detected: {}", label),
                    redacted_content: redacted.to_string(),
                };
            }
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
                let mut hasher = siphasher::sip::SipHasher::new();
                word.hash(&mut hasher);
                let hash = hasher.finish();
                if self.injected_hashes.contains(&hash) {
                    return LeakDecision::Block {
                        reason: "Inbound response contains hash-matched injected secret".to_string(),
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
                let mut h = siphasher::sip::SipHasher::new();
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
        if let LeakDecision::Block { redacted_content, .. } = detector.scan_outbound(content) {
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
}
