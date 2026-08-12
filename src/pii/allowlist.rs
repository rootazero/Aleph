//! PII allowlist — known non-PII values that should not trigger filtering

use regex::Regex;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

static SYSTEM_EMAIL_PATTERNS: OnceLock<Arc<Vec<Regex>>> = OnceLock::new();

fn system_email_patterns() -> &'static Arc<Vec<Regex>> {
    SYSTEM_EMAIL_PATTERNS.get_or_init(|| {
        Arc::new(vec![
            Regex::new(r"(?i)^noreply@").expect("valid regex literal"),
            Regex::new(r"(?i)^no-reply@").expect("valid regex literal"),
            Regex::new(r"(?i)^donotreply@").expect("valid regex literal"),
            Regex::new(r"(?i)@(example|test|demo|sample|mock|localhost)\b")
                .expect("valid regex literal"),
            Regex::new(r"(?i)\.(example|test|local|internal|invalid)$")
                .expect("valid regex literal"),
        ])
    })
}

/// Known test phone numbers, stored once and looked up by `&str` to avoid
/// the per-default `String` allocation of the previous implementation.
static TEST_PHONES: OnceLock<HashSet<&'static str>> = OnceLock::new();

fn test_phones() -> &'static HashSet<&'static str> {
    TEST_PHONES.get_or_init(|| {
        [
            "13800138000",
            "18888888888",
            "13900001111",
            "13800000000",
            "15800000000",
            "18900000000",
        ]
        .into_iter()
        .collect()
    })
}

/// Known local/internal IPs, stored once for the same reason as `TEST_PHONES`.
static LOCAL_IPS: OnceLock<HashSet<&'static str>> = OnceLock::new();

fn local_ips() -> &'static HashSet<&'static str> {
    LOCAL_IPS.get_or_init(|| {
        [
            "127.0.0.1",
            "0.0.0.0",
            "192.168.0.1",
            "192.168.1.1",
            "10.0.0.1",
            "172.16.0.1",
        ]
        .into_iter()
        .collect()
    })
}

/// Allowlist of known non-PII values
pub struct PiiAllowlist {
    /// Known test phone numbers (borrowed from the static set)
    test_phones: &'static HashSet<&'static str>,
    /// System/example email patterns (shared via `Arc` to avoid cloning the
    /// regex `Vec` on every `Default::default()`)
    system_email_patterns: Arc<Vec<Regex>>,
    /// Known local/internal IPs (borrowed from the static set)
    local_ips: &'static HashSet<&'static str>,
}

impl Default for PiiAllowlist {
    fn default() -> Self {
        Self {
            test_phones: test_phones(),
            system_email_patterns: Arc::clone(system_email_patterns()),
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
