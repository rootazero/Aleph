//! Email address detection

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static EMAIL_RE: OnceLock<Regex> = OnceLock::new();

fn email_regex() -> &'static Regex {
    EMAIL_RE.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").unwrap()
    })
}

pub struct EmailRule;

impl EmailRule {
    pub fn new() -> Self {
        Self
    }
}

impl PiiRule for EmailRule {
    fn name(&self) -> &str {
        "email"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Medium
    }
    fn placeholder(&self) -> &str {
        "[EMAIL]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = email_regex();
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

    fn rule() -> EmailRule {
        EmailRule::new()
    }

    #[test]
    fn test_detect_simple_email() {
        let matches = rule().detect("Contact user@example.com for help");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].matched_text, "user@example.com");
    }

    #[test]
    fn test_detect_email_with_plus() {
        let matches = rule().detect("Email: user+tag@domain.co.uk");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_missing_at() {
        let matches = rule().detect("notanemail.com");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_match_missing_tld() {
        let matches = rule().detect("user@domain");
        assert_eq!(matches.len(), 0);
    }
}
