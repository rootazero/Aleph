//! PII allowlist — known non-PII values that should not trigger filtering

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

static SYSTEM_EMAIL_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn system_email_patterns() -> &'static Vec<Regex> {
    SYSTEM_EMAIL_PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?i)^noreply@").expect("valid regex literal"),
            Regex::new(r"(?i)^no-reply@").expect("valid regex literal"),
            Regex::new(r"(?i)^donotreply@").expect("valid regex literal"),
            Regex::new(r"(?i)@(example|test|demo|sample|mock|localhost)\b")
                .expect("valid regex literal"),
            Regex::new(r"(?i)\.(example|test|local|internal|invalid)$")
                .expect("valid regex literal"),
        ]
    })
}

fn test_phones() -> HashSet<String> {
    [
        "13800138000",
        "18888888888",
        "13900001111",
        "13800000000",
        "15800000000",
        "18900000000",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn local_ips() -> HashSet<String> {
    [
        "127.0.0.1",
        "0.0.0.0",
        "192.168.0.1",
        "192.168.1.1",
        "10.0.0.1",
        "172.16.0.1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Allowlist of known non-PII values
pub struct PiiAllowlist {
    /// Known test phone numbers
    pub(crate) test_phones: HashSet<String>,
    /// System/example email patterns
    pub(crate) system_email_patterns: Vec<Regex>,
    /// Known local/internal IPs
    pub(crate) local_ips: HashSet<String>,
}

impl Default for PiiAllowlist {
    fn default() -> Self {
        Self {
            test_phones: test_phones(),
            system_email_patterns: system_email_patterns().clone(),
            local_ips: local_ips(),
        }
    }
}

impl PiiAllowlist {
    /// Check if a matched value should be excluded from PII detection
    #[must_use]
    pub fn is_allowed(&self, value: &str, rule_name: &str) -> bool {
        match rule_name {
            "phone" => self.test_phones.contains(value),
            "email" => self.system_email_patterns.iter().any(|p| p.is_match(value)),
            "ip_address" => self.local_ips.contains(value),
            _ => false,
        }
    }
}
