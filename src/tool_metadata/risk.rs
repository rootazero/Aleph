//! Risk evaluation for tool execution
//!
//! ⚠️ **R8 NOTE**: This module's pattern-matching risk heuristic is an
//! architectural redline violation — Aleph's redlines forbid regex-based
//! classification of user intent (R8 reserves regex for machine formats
//! like JSON / URLs / file globs; intent classification belongs to the
//! LLM via prompt). Patterns like `\b(api|http|request)\b` mis-fire on
//! benign text ("Read a book" / "run a marathon"), producing both
//! false-negatives (high-risk requests that don't match any keyword) and
//! false-positives (low-risk requests that do). This file is kept
//! because `tests/world/dispatcher_ctx.rs` still references
//! `RiskEvaluator`; new consumers MUST call the LLM for risk gating
//! instead of using `matches_high_risk_pattern`. Follow-up: remove this
//! module + its test once `dispatcher_ctx` is rewritten.

use regex::Regex;
use std::sync::OnceLock;

/// Risk level for task execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    /// Low risk - can be executed automatically
    Low,
    /// High risk - requires user confirmation
    High,
}

/// Risk evaluator using text pattern matching
#[derive(Debug, Clone)]
pub struct RiskEvaluator {
    /// Whether to use pattern-based evaluation
    use_patterns: bool,
}

/// Get high-risk patterns (lazily initialized)
fn get_high_risk_patterns() -> &'static Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // Network/API patterns
            Regex::new(r"(?i)\b(api|http|request|fetch|curl|wget)\b")
                .expect("hardcoded regex is valid"),
            // Execution patterns
            Regex::new(r"(?i)\b(execute|run|eval|shell|command|exec)\b")
                .expect("hardcoded regex is valid"),
            // File modification patterns
            Regex::new(r"(?i)\b(write|delete|remove|modify|create)\s*(file|文件)\b")
                .expect("hardcoded regex is valid"),
            // Send/Upload patterns
            Regex::new(r"(?i)\b(send|post|upload|publish|发送|上传)\b")
                .expect("hardcoded regex is valid"),
            // Financial patterns
            Regex::new(r"(?i)\b(pay|purchase|transaction|transfer|支付|购买|转账)\b")
                .expect("hardcoded regex is valid"),
            // Chinese API patterns
            Regex::new(r"(?i)(调用|请求|接口)").expect("hardcoded regex is valid"),
        ]
    })
}

impl RiskEvaluator {
    /// Create a new risk evaluator
    #[must_use]
    pub const fn new() -> Self {
        Self { use_patterns: true }
    }

    /// Check if text matches any high-risk pattern
    #[must_use]
    pub fn matches_high_risk_pattern(&self, text: &str) -> bool {
        if !self.use_patterns {
            return false;
        }
        get_high_risk_patterns()
            .iter()
            .any(|pattern| pattern.is_match(text))
    }
}

impl Default for RiskEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_risk_patterns() {
        let evaluator = RiskEvaluator::new();

        assert!(evaluator.matches_high_risk_pattern("Call external API"));
        assert!(evaluator.matches_high_risk_pattern("Make HTTP request"));
        assert!(evaluator.matches_high_risk_pattern("调用第三方接口"));
        assert!(evaluator.matches_high_risk_pattern("Execute command"));
        assert!(evaluator.matches_high_risk_pattern("send email"));

        // Low risk text
        assert!(!evaluator.matches_high_risk_pattern("Summarize this document"));
        assert!(!evaluator.matches_high_risk_pattern("Read a book"));
    }
}
