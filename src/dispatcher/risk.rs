//! Risk evaluation for tool execution
//!
//! This module provides risk assessment based on text pattern matching,
//! used to determine whether user confirmation is required.

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
            Regex::new(r"(?i)(api|http|request|fetch|curl|wget)").expect("hardcoded regex is valid"),
            // Execution patterns
            Regex::new(r"(?i)(execute|run|eval|shell|command|exec)").expect("hardcoded regex is valid"),
            // File modification patterns
            Regex::new(r"(?i)(write|delete|remove|modify|create)\s*(file|文件)").expect("hardcoded regex is valid"),
            // Send/Upload patterns
            Regex::new(r"(?i)(send|post|upload|publish|发送|上传)").expect("hardcoded regex is valid"),
            // Financial patterns
            Regex::new(r"(?i)(pay|purchase|transaction|transfer|支付|购买|转账)").expect("hardcoded regex is valid"),
            // Chinese API patterns
            Regex::new(r"(?i)(调用|请求|接口)").expect("hardcoded regex is valid"),
        ]
    })
}

impl RiskEvaluator {
    /// Create a new risk evaluator
    pub fn new() -> Self {
        Self { use_patterns: true }
    }

    /// Check if text matches any high-risk pattern
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
