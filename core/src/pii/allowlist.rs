//! PII allowlist — known non-PII values that should not trigger filtering

use regex::Regex;
use std::collections::HashSet;

/// Allowlist of known non-PII values
pub struct PiiAllowlist {
    /// Known test phone numbers
    pub test_phones: HashSet<String>,
    /// System/example email patterns
    pub system_email_patterns: Vec<Regex>,
    /// Known local/internal IPs
    pub local_ips: HashSet<String>,
}

impl Default for PiiAllowlist {
    fn default() -> Self {
        let test_phones: HashSet<String> = [
            "13800138000",
            "18888888888",
            "13900001111",
            "13800000000",
            "15800000000",
            "18900000000",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let system_email_patterns = vec![
            Regex::new(r"(?i)^noreply@").unwrap(),
            Regex::new(r"(?i)^no-reply@").unwrap(),
            Regex::new(r"(?i)^donotreply@").unwrap(),
            Regex::new(r"(?i)@(example|test|demo|sample|mock|localhost)\b").unwrap(),
            Regex::new(r"(?i)\.(example|test|local|internal|invalid)$").unwrap(),
        ];

        let local_ips: HashSet<String> = [
            "127.0.0.1",
            "0.0.0.0",
            "localhost",
            "192.168.0.1",
            "192.168.1.1",
            "10.0.0.1",
            "172.16.0.1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        Self {
            test_phones,
            system_email_patterns,
            local_ips,
        }
    }
}

impl PiiAllowlist {
    /// Check if a matched value should be excluded from PII detection
    pub fn is_allowed(&self, value: &str, rule_name: &str) -> bool {
        match rule_name {
            "phone" => self.test_phones.contains(value),
            "email" => self
                .system_email_patterns
                .iter()
                .any(|p| p.is_match(value)),
            "ip_address" => self.local_ips.contains(value),
            _ => false,
        }
    }
}
