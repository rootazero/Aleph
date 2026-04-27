//! Custom regex-based PII detection rules
//!
//! User-defined rules loaded from configuration. Each rule uses a regex
//! pattern to detect PII and produces matches with configurable severity
//! and placeholder replacement.

use crate::config::types::CustomPiiRule;
use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;

/// A PII rule backed by a user-configured regex pattern.
pub struct CustomRegexRule {
    config: CustomPiiRule,
    regex: Regex,
}

impl CustomRegexRule {
    /// Create a custom rule from configuration.
    ///
    /// Returns `Err(regex::Error)` if the pattern is invalid.
    pub fn new(config: CustomPiiRule) -> Result<Self, regex::Error> {
        let regex = Regex::new(&config.pattern)?;
        Ok(Self { config, regex })
    }
}

impl PiiRule for CustomRegexRule {
    fn name(&self) -> &str {
        &self.config.name
    }

    fn severity(&self) -> PiiSeverity {
        self.config.severity.into()
    }

    fn placeholder(&self) -> &str {
        &self.config.placeholder
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let mut results = Vec::new();

        for m in self.regex.find_iter(text) {
            results.push(PiiMatch {
                rule_name: self.config.name.clone(),
                start: m.start(),
                end: m.end(),
                matched_text: m.as_str().to_string(),
                severity: self.config.severity.into(),
                placeholder: self.config.placeholder.clone(),
            });
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{CustomPiiRule, CustomPiiSeverity, PiiAction};

    fn make_rule(name: &str, pattern: &str, placeholder: &str) -> CustomRegexRule {
        CustomRegexRule::new(CustomPiiRule {
            name: name.to_string(),
            pattern: pattern.to_string(),
            placeholder: placeholder.to_string(),
            severity: CustomPiiSeverity::High,
            action: PiiAction::Block,
        })
        .unwrap()
    }

    #[test]
    fn test_detect_custom_pattern() {
        let rule = make_rule("internal_token", r"IT-[A-Z0-9]{16}", "[IT_TOKEN]");
        let matches = rule.detect("Token: IT-ABCD1234EFGH5678");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "IT-ABCD1234EFGH5678");
        assert_eq!(matches[0].placeholder, "[IT_TOKEN]");
    }

    #[test]
    fn test_no_match() {
        let rule = make_rule("employee_id", r"EMP-[0-9]{6}", "[EMP_ID]");
        let matches = rule.detect("No employee id here");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_multiple_matches() {
        let rule = make_rule("token", r"\btok_[a-z]{4}\b", "[TOKEN]");
        let matches = rule.detect("tok_abcd and tok_efgh");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_invalid_regex_fails() {
        let result = CustomRegexRule::new(CustomPiiRule {
            name: "bad".to_string(),
            pattern: "[invalid".to_string(),
            placeholder: "[X]".to_string(),
            severity: CustomPiiSeverity::Low,
            action: PiiAction::Block,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_severity_conversion() {
        let rule = CustomRegexRule::new(CustomPiiRule {
            name: "test".to_string(),
            pattern: r"\d+".to_string(),
            placeholder: "[N]".to_string(),
            severity: CustomPiiSeverity::Critical,
            action: PiiAction::Block,
        })
        .unwrap();
        assert_eq!(rule.severity(), PiiSeverity::Critical);
    }
}
