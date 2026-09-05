//! `LeakDetector` - Bidirectional leak detection for sensitive data.
//!
//! Runs a full regex sweep over both outbound (to LLM) and inbound (from LLM)
//! content for:
//! - API keys (`OpenAI`, Anthropic, Google, AWS, GitHub, Slack)
//! - Private keys (PEM format)
//! - Bearer tokens

use regex::Regex;

/// Action to take when a leak is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakAction {
    /// Block the content entirely — must not be transmitted.
    Block,
    /// Redact the matched portion before transmission.
    Redact,
}

/// A pattern to detect potential leaked secrets.
pub(crate) struct LeakPattern {
    /// Human-readable name for this pattern.
    pub name: &'static str,
    /// Regex for full pattern matching.
    pub regex: Regex,
    /// Action to take when matched.
    pub action: LeakAction,
}

/// A single finding from a scan.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ScanFinding {
    /// Name of the pattern that matched.
    pub pattern_name: &'static str,
    /// Action recommended for this finding.
    pub action: LeakAction,
}

/// Result of scanning content for leaks.
#[derive(Debug)]
pub struct ScanResult {
    /// All findings from the scan.
    pub findings: Vec<ScanFinding>,
}

impl ScanResult {
    /// Returns true if any finding requires blocking.
    #[must_use]
    pub fn has_blocks(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Block)
    }

    /// Returns true if any finding requires redaction.
    ///
    /// A `Redact` finding is not a block, so consumers that only check
    /// `has_blocks()` would silently let the matched secret through. Callers
    /// must consult this and apply [`LeakDetector::redact`].
    #[must_use]
    pub fn has_redacts(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Redact)
    }
}

/// Bidirectional leak detector running a full regex sweep over content.
pub struct LeakDetector {
    /// Full regex patterns for detailed matching.
    patterns: Vec<LeakPattern>,
}

impl LeakDetector {
    /// Create a `LeakDetector` with default patterns for common secret types.
    #[must_use]
    pub fn default_patterns() -> Self {
        let assets = crate::exec::secret_patterns::leak_detector_assets();
        let patterns = assets
            .patterns
            .into_iter()
            .map(|p| LeakPattern {
                name: p.name,
                regex: p.regex,
                action: p.action,
            })
            .collect();
        Self { patterns }
    }

    /// Scan content for leaks (internal implementation).
    fn scan(&self, content: &str) -> ScanResult {
        // Always run the full regex sweep. The cost of `Regex::find` over a
        // short content string is negligible compared to the cost of a
        // secret-leak callback page.
        let mut findings = Vec::new();
        for pattern in &self.patterns {
            if pattern.regex.find(content).is_some() {
                findings.push(ScanFinding {
                    pattern_name: pattern.name,
                    action: pattern.action,
                });
            }
        }

        ScanResult { findings }
    }

    /// Scan outbound content (being sent to LLM or external service).
    #[must_use]
    pub fn scan_outbound(&self, content: &str) -> ScanResult {
        self.scan(content)
    }

    /// Scan inbound content (received from LLM or external service).
    #[must_use]
    pub fn scan_inbound(&self, content: &str) -> ScanResult {
        self.scan(content)
    }

    /// Replace every `Redact`-action pattern match with a redaction marker,
    /// returning the sanitized content.
    ///
    /// Only `Redact` patterns are rewritten; `Block` matches are left untouched
    /// because their disposition is the caller's policy decision (block the
    /// whole message). This is the missing "last mile" for
    /// `LeakAction::Redact`, which otherwise has no honoring consumer.
    #[must_use]
    pub fn redact(&self, content: &str) -> String {
        let mut current = content.to_string();
        for pattern in &self.patterns {
            if pattern.action == LeakAction::Redact {
                current = pattern
                    .regex
                    .replace_all(&current, "***REDACTED***")
                    .into_owned();
            }
        }
        current
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
        assert!(!result.findings.is_empty());
    }

    #[test]
    fn test_detects_github_token() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh1234");
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
        assert!(result.findings.is_empty());
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
        assert!(
            aws_finding.is_some(),
            "should have an aws_access_key finding"
        );
    }

    #[test]
    fn test_detects_private_key_block() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("-----BEGIN RSA PRIVATE KEY-----\nMIIE...");
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
        let result =
            detector.scan_inbound("Here is a key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz");
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
    fn test_bearer_token_is_redact_not_block() {
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
        assert!(!result.findings.is_empty(), "should detect bearer token");
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
    fn test_bearer_token_redact_removes_secret() {
        let detector = LeakDetector::default_patterns();
        let token = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        let scan = detector.scan_outbound(token);
        assert!(scan.has_redacts(), "bearer token should report a redact");
        assert!(!scan.has_blocks(), "bearer token is not a block");
        let redacted = detector.redact(token);
        assert!(
            !redacted.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"),
            "secret must be removed: {redacted}"
        );
        assert!(
            redacted.contains("***REDACTED***"),
            "redaction marker expected: {redacted}"
        );
    }

    #[test]
    fn test_redact_leaves_block_secrets_untouched() {
        // `redact` only rewrites Redact-action matches; Block secrets are the
        // caller's policy decision and must not be silently mutated here.
        let detector = LeakDetector::default_patterns();
        let content = "key=AKIAIOSFODNN7EXAMPLE";
        assert_eq!(detector.redact(content), content);
    }

    #[test]
    fn test_clean_content_yields_no_findings() {
        // Ordinary prose and numbers should not match any secret pattern.
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("The quick brown fox jumps over the lazy dog. 12345");
        assert!(result.findings.is_empty());
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
        let result = detector.scan_outbound("key=AIzaSyA1234567890abcdefghijklmnopqrstuv");
        assert!(result.has_blocks(), "should detect Google API key as block");
    }

    #[test]
    fn test_uppercase_bearer_is_detected() {
        // Regression: the `(?i)bearer` regex must match an uppercase `BEARER`
        // token (it once slipped through a case-sensitive prefix gate).
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("BEARER eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
        let bearer_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "bearer_token");
        assert!(
            bearer_finding.is_some(),
            "uppercase BEARER must be detected by the case-insensitive regex"
        );
    }

    #[test]
    fn test_github_fine_grained_pat_detection() {
        let detector = LeakDetector::default_patterns();
        let result = detector
            .scan_outbound("token=github_pat_11ABCDE0Y0abcdefghijklmnopqrstuvwxyz0123456789ABCDEF");
        assert!(
            result.has_blocks(),
            "should detect github fine-grained PAT as block"
        );
        let finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "github_fine_grained_pat");
        assert!(
            finding.is_some(),
            "should have a github_fine_grained_pat finding"
        );
    }
}
