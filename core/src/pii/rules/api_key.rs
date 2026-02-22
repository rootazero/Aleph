//! API key and token detection
//!
//! Uses prefix-based matching only to avoid URL slug false positives.
//! Patterns cover major API providers with known prefixes.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static API_KEY_RE: OnceLock<Regex> = OnceLock::new();

fn api_key_regex() -> &'static Regex {
    API_KEY_RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            (?:
                sk-[a-zA-Z0-9\-_]{20,}           # OpenAI / Anthropic style
                | ghp_[a-zA-Z0-9]{36,}            # GitHub Personal Access Token
                | gho_[a-zA-Z0-9]{36,}            # GitHub OAuth
                | github_pat_[a-zA-Z0-9_]{82}     # GitHub Fine-grained PAT
                | AKIA[A-Z0-9]{16}                # AWS Access Key ID
                | xox[bpras]-[a-zA-Z0-9\-]{10,}  # Slack tokens
                | tvly-[a-zA-Z0-9\-_]{20,}        # Tavily
                | Bearer\s+[a-zA-Z0-9._\-]{20,}  # Generic Bearer token
            )
            ",
        )
        .unwrap()
    })
}

pub struct ApiKeyRule;

impl ApiKeyRule {
    pub fn new() -> Self {
        Self
    }
}

impl PiiRule for ApiKeyRule {
    fn name(&self) -> &str {
        "api_key"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Critical
    }
    fn placeholder(&self) -> &str {
        "[REDACTED]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = api_key_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start: m.start(),
                end: m.end(),
                matched_text: m.as_str().to_string(),
                severity: self.severity(),
                placeholder: self.placeholder().to_string(),
            });
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> ApiKeyRule {
        ApiKeyRule::new()
    }

    #[test]
    fn test_detect_openai_key() {
        let matches = rule().detect("key: sk-1234567890abcdefghijklmnopqrstuvwxyz123456");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_anthropic_key() {
        let matches = rule().detect("ANTHROPIC_API_KEY=sk-ant-api03-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_github_pat() {
        let matches = rule().detect("token: ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_aws_key() {
        let matches = rule().detect("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_short_sk() {
        // sk- prefix but too short
        let matches = rule().detect("sk-short");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_detect_bearer_token() {
        let matches = rule().detect("Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9");
        assert_eq!(matches.len(), 1);
    }
}
