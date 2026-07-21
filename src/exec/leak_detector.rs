//! `LeakDetector` - Bidirectional leak detection for sensitive data.
//!
//! Uses Aho-Corasick for fast prefix scanning + regex for full pattern matching.
//! Scans both outbound (to LLM) and inbound (from LLM) content for:
//! - API keys (`OpenAI`, Anthropic, Google, AWS, GitHub, Slack)
//! - Private keys (PEM format)
//! - Bearer tokens

use aho_corasick::{AhoCorasick, AhoCorasickBuilder};
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
    #[must_use]
    pub fn has_blocks(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Block)
    }

    /// Returns true if any finding is a warning.
    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Warn)
    }

    /// Returns true if any finding requires redaction.
    ///
    /// A `Redact` finding is neither a block nor a warning, so consumers that
    /// only check `has_blocks()`/`has_warnings()` would silently let the matched
    /// secret through. Callers must consult this and apply [`LeakDetector::redact`].
    #[must_use]
    pub fn has_redacts(&self) -> bool {
        self.findings.iter().any(|f| f.action == LeakAction::Redact)
    }

    /// Returns true if no findings were detected.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
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
    /// Full regex patterns for detailed matching.
    patterns: Vec<LeakPattern>,
}

impl LeakDetector {
    /// Create a new `LeakDetector` with the given prefixes and patterns.
    #[must_use]
    pub fn new(prefixes: Vec<&'static str>, patterns: Vec<LeakPattern>) -> Self {
        // ASCII case-insensitive so the fast-path prefix gate matches the
        // case-insensitive `(?i)` regexes below. Without this, content like
        // `BEARER <token>` would fail the prefix gate and skip the regex scan
        // entirely, leaking a secret that the slow path would have caught.
        let ac = AhoCorasickBuilder::new()
            .ascii_case_insensitive(true)
            .build(&prefixes)
            .unwrap_or_else(|_| unreachable!("failed to build Aho-Corasick automaton"));
        Self { ac, patterns }
    }

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
        Self::new(assets.prefixes, patterns)
    }

    /// Scan content for leaks (internal implementation).
    fn scan(&self, content: &str) -> ScanResult {
        // The Aho-Corasick automaton is a SOFT hint, not a hard gate: many
        // regex patterns (custom vault tokens, raw JWTs without a "Bearer "
        // prefix, HMAC blobs, post-quantum secrets) carry no prefix the
        // automaton knows about. Gating the regex on the prefix match used
        // to let those high-value credentials escape every leak check
        // (`is_clean() == true` despite a real secret being present).
        // Now the prefix scan only chooses between "definitely run regex" and
        // "very-likely-but-still-run-regex" — the regex pass is always taken.
        let _ = self.ac.is_match(content);

        // Always run the full regex sweep. The cost of `Regex::find` over a
        // short content string is negligible compared to the cost of a
        // secret-leak callback page.
        let mut findings = Vec::new();
        for pattern in &self.patterns {
            if let Some(m) = pattern.regex.find(content) {
                let matched = m.as_str();
                let truncated = if matched.len() > 20 {
                    // Truncate to 20 chars, scanning backward from byte 20 to find
                    // the nearest valid UTF-8 character boundary so we don't split
                    // a multi-byte codepoint.
                    let byte_offset = matched
                        .char_indices()
                        .nth(20)
                        .map_or(matched.len(), |(i, _)| i);
                    format!("{}...", &matched[..byte_offset])
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
    /// Only `Redact` patterns are rewritten; `Block`/`Warn` matches are left
    /// untouched because their disposition is the caller's policy decision
    /// (block the whole message, or warn-and-pass). This is the missing "last
    /// mile" for `LeakAction::Redact`, which otherwise has no honoring consumer.
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
        assert!(!result.is_clean());
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
    fn test_matched_text_truncation() {
        let detector = LeakDetector::default_patterns();
        // A long key that exceeds 20 chars
        let result = detector.scan_outbound("sk-abcdefghijklmnopqrstuvwxyz1234567890abcdef");
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
        assert!(!result.is_clean(), "should detect bearer token");
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
        let result = detector.scan_outbound("key=AIzaSyA1234567890abcdefghijklmnopqrstuv");
        assert!(result.has_blocks(), "should detect Google API key as block");
    }

    #[test]
    fn test_uppercase_bearer_not_skipped_by_fast_path() {
        // Regression: the Aho-Corasick prefix gate must be case-insensitive so
        // an uppercase `BEARER` still reaches the `(?i)bearer` regex.
        let detector = LeakDetector::default_patterns();
        let result = detector.scan_outbound("BEARER eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9");
        let bearer_finding = result
            .findings
            .iter()
            .find(|f| f.pattern_name == "bearer_token");
        assert!(
            bearer_finding.is_some(),
            "uppercase BEARER must not bypass the fast-path prefix gate"
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
