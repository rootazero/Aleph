//! LeakDetector - Bidirectional leak detection for sensitive data.
//!
//! Uses Aho-Corasick for fast prefix scanning + regex for full pattern matching.
//! Scans both outbound (to LLM) and inbound (from LLM) content for:
//! - API keys (OpenAI, Anthropic, Google, AWS, GitHub, Slack)
//! - Private keys (PEM format)
//! - Bearer tokens

use aho_corasick::AhoCorasick;
use regex::Regex;

/// Action to take when a leak is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakAction {
    /// Block the content entirely — must not be transmitted.
    Block,
    /// Redact the matched portion before transmission.
    Redact,
    /// Allow transmission but emit a warning.
    Warn,
}

/// A pattern to detect potential leaked secrets.
pub struct LeakPattern {
    /// Human-readable name for this pattern.
    pub name: &'static str,
    /// Regex for full pattern matching.
    pub regex: Regex,
    /// Action to take when matched.
    pub action: LeakAction,
}

/// A single finding from a scan.
#[derive(Debug, Clone)]
pub struct ScanFinding {
    /// Name of the pattern that matched.
    pub pattern_name: &'static str,
    /// Action recommended for this finding.
    pub action: LeakAction,
    /// The matched text, truncated to 20 characters.
    pub matched_text: String,
}

/// Result of scanning content for leaks.
#[derive(Debug)]
pub struct ScanResult {
    /// All findings from the scan.
    pub findings: Vec<ScanFinding>,
}

impl ScanResult {
    /// Returns true if any finding requires blocking.
    pub fn has_blocks(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Block)
    }

    /// Returns true if any finding is a warning.
    pub fn has_warnings(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Warn)
    }

    /// Returns true if no findings were detected.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Bidirectional leak detector using Aho-Corasick + regex.
///
/// Fast path: Aho-Corasick scans for known prefixes first.
/// Slow path: Only if a prefix matches, run full regex patterns.
pub struct LeakDetector {
    /// Aho-Corasick automaton for fast prefix scanning.
    ac: AhoCorasick,
    /// Prefix strings used to build the automaton (for reference).
    #[allow(dead_code)]
    prefixes: Vec<&'static str>,
    /// Full regex patterns for detailed matching.
    patterns: Vec<LeakPattern>,
}

impl LeakDetector {
    /// Create a new LeakDetector with the given prefixes and patterns.
    pub fn new(prefixes: Vec<&'static str>, patterns: Vec<LeakPattern>) -> Self {
        let ac = AhoCorasick::new(&prefixes).expect("failed to build Aho-Corasick automaton");
        Self {
            ac,
            prefixes,
            patterns,
        }
    }

    /// Create a LeakDetector with default patterns for common secret types.
    pub fn default_patterns() -> Self {
        let prefixes = vec![
            "sk-",
            "AIza",
            "AKIA",
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "ghr_",
            "xoxb-",
            "xoxa-",
            "xoxp-",
            "xoxr-",
            "xoxs-",
            "-----BEGIN",
            "bearer ",
            "Bearer ",
        ];

        let patterns = vec![
            LeakPattern {
                name: "openai_key",
                regex: Regex::new(r"sk-[a-zA-Z0-9]{20,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "anthropic_key",
                regex: Regex::new(r"sk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "google_api_key",
                regex: Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "aws_access_key",
                regex: Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "github_token",
                regex: Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "slack_token",
                regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "private_key",
                regex: Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----").unwrap(),
                action: LeakAction::Block,
            },
            LeakPattern {
                name: "bearer_token",
                regex: Regex::new(r"(?i)bearer\s+[a-zA-Z0-9\-._~+/]+=*").unwrap(),
                action: LeakAction::Redact,
            },
        ];

        Self::new(prefixes, patterns)
    }

    /// Scan content for leaks (internal implementation).
    fn scan(&self, content: &str) -> ScanResult {
        // Fast path: if no prefix matches, content is clean
        if !self.ac.is_match(content) {
            return ScanResult {
                findings: Vec::new(),
            };
        }

        // Slow path: check each regex pattern
        let mut findings = Vec::new();
        for pattern in &self.patterns {
            if let Some(m) = pattern.regex.find(content) {
                let matched = m.as_str();
                let truncated = if matched.len() > 20 {
                    format!("{}...", &matched[..20])
                } else {
                    matched.to_string()
                };
                findings.push(ScanFinding {
                    pattern_name: pattern.name,
                    action: pattern.action,
                    matched_text: truncated,
                });
            }
        }

        ScanResult { findings }
    }

    /// Scan outbound content (being sent to LLM or external service).
    pub fn scan_outbound(&self, content: &str) -> ScanResult {
        self.scan(content)
    }

    /// Scan inbound content (received from LLM or external service).
    pub fn scan_inbound(&self, content: &str) -> ScanResult {
        self.scan(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detects_openai_key_in_outbound() {
        let detector = LeakDetector::default_patterns();
        let result =
            detector.scan_outbound("Authorization: Bearer sk-abc123def456ghi789jklmnopqrstuvwx");
        assert!(result.has_blocks(), "should detect OpenAI key as block");
        assert!(!result.is_clean());
    }

    #[test]
    fn test_detects_github_token() {
        let detector = LeakDetector::default_patterns();
        let result = detector
            .scan_outbound("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh1234");
        assert!(result.has_blocks(), "should detect GitHub token as block");
        // Verify the finding references github_token
        let github_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "github_token");
        assert!(
            github_finding.is_some(),
            "should have a github_token finding"
        );
    }

    #[test]
    fn test_clean_content_passes() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("Hello world, this is normal text");
        assert!(!result.has_blocks());
        assert!(!result.has_warnings());
        assert!(result.is_clean());
    }

    #[test]
    fn test_detects_aws_key() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("key=AKIAIOSFODNN7EXAMPLE");
        assert!(result.has_blocks(), "should detect AWS key as block");
        let aws_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "aws_access_key");
        assert!(aws_finding.is_some(), "should have an aws_access_key finding");
    }

    #[test]
    fn test_detects_private_key_block() {
        let detector = LeakDetector::default_patterns();
        let result =
            detector.scan_outbound("-----BEGIN RSA PRIVATE KEY-----\nMIIE...");
        assert!(
            result.has_blocks(),
            "should detect private key block as block"
        );
        let pk_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "private_key");
        assert!(pk_finding.is_some(), "should have a private_key finding");
    }

    #[test]
    fn test_scan_inbound_also_works() {
        let detector = LeakDetector::default_patterns();
        let result = detector
            .scan_inbound("Here is a key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz");
        assert!(
            result.has_blocks(),
            "inbound scan should detect Anthropic key"
        );
        let anthropic_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "anthropic_key");
        assert!(
            anthropic_finding.is_some(),
            "should have an anthropic_key finding"
        );
    }

    #[test]
    fn test_matched_text_truncation() {
        let detector = LeakDetector::default_patterns();
        // A long key that exceeds 20 chars
        let result = detector
            .scan_outbound("sk-abcdefghijklmnopqrstuvwxyz1234567890abcdef");
        let finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "openai_key")
            .expect("should detect openai key");
        // Truncated to 20 chars + "..."
        assert!(
            finding.matched_text.len() <= 23,
            "matched_text should be truncated: {}",
            finding.matched_text
        );
        assert!(
            finding.matched_text.ends_with("..."),
            "truncated text should end with '...'"
        );
    }

    #[test]
    fn test_bearer_token_is_redact_not_block() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
        assert!(
            !result.is_clean(),
            "should detect bearer token"
        );
        let bearer_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "bearer_token")
            .expect("should have a bearer_token finding");
        assert_eq!(
            bearer_finding.action,
            LeakAction::Redact,
            "bearer token should be Redact, not Block"
        );
    }

    #[test]
    fn test_fast_path_skips_regex_on_clean_content() {
        // This test verifies the fast path behavior:
        // content with no prefixes should not trigger regex scanning at all.
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("The quick brown fox jumps over the lazy dog. 12345");
        assert!(result.is_clean());
        assert_eq!(result.findings.len(), 0);
    }

    #[test]
    fn test_slack_token_detection() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("SLACK_TOKEN=xoxb-1234567890-abcdefghij");
        assert!(result.has_blocks(), "should detect Slack token as block");
        let slack_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "slack_token");
        assert!(slack_finding.is_some(), "should have a slack_token finding");
    }

    #[test]
    fn test_google_api_key_detection() {
        let detector = LeakDetector::default_patterns();
        let result = detector
            .scan_outbound("key=AIzaSyA1234567890abcdefghijklmnopqrstuv");
        assert!(
            result.has_blocks(),
            "should detect Google API key as block"
        );
    }
}
