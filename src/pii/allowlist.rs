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
            // The three reserved example domains (RFC 6761 §6).
            //
            // This used to be `@(example|test|demo|sample|mock|localhost)\b`,
            // whose `\b` matches after the FIRST LABEL of any domain — so
            // `alice@sample.com`, `alice@demo.com` and `alice@mock.com`
            // were exempt although they are ordinary registered domains.
            // An allowlisted match is not even counted as a warning, so the
            // operator got no signal at all that the address was let through.
            // Only the names RFC 2606/6761/6762 actually reserve are exempt.
            Regex::new(r"(?i)@(example\.(com|net|org))$").expect("valid regex literal"),
            // Reserved TLDs: RFC 2606 reserves `.test`, `.example`,
            // `.invalid` and `.localhost`; RFC 6762 reserves `.local`.
            // `.internal` was removed — it is not an IANA special-use TLD,
            // so `alice@corp.internal` is an ordinary address and must be
            // filtered like any other.
            Regex::new(r"(?i)\.(test|example|invalid|localhost|local)$")
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

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist() -> PiiAllowlist {
        PiiAllowlist::default()
    }

    #[test]
    fn reserved_names_are_allowlisted() {
        let allowlist = allowlist();
        // RFC 6761 reserved example domains.
        assert!(allowlist.is_allowed("alice@example.com", "email"));
        assert!(allowlist.is_allowed("alice@example.net", "email"));
        assert!(allowlist.is_allowed("alice@example.org", "email"));
        // RFC 2606 / RFC 6762 reserved TLDs.
        assert!(allowlist.is_allowed("alice@acme.test", "email"));
        assert!(allowlist.is_allowed("alice@acme.example", "email"));
        assert!(allowlist.is_allowed("alice@acme.invalid", "email"));
        assert!(allowlist.is_allowed("alice@acme.localhost", "email"));
        assert!(allowlist.is_allowed("alice@acme.local", "email"));
        // No-reply style local parts.
        assert!(allowlist.is_allowed("noreply@acme.test", "email"));
        assert!(allowlist.is_allowed("no-reply@acme.test", "email"));
        assert!(allowlist.is_allowed("donotreply@acme.test", "email"));
    }

    #[test]
    fn ordinary_registered_domains_are_not_allowlisted() {
        // The old pattern `@(example|test|demo|sample|mock|localhost)\b`
        // matched the FIRST LABEL of any domain, so these ordinary
        // registered addresses were exempt — silently, without even a
        // warning. They must be filtered like any other address.
        let allowlist = allowlist();
        assert!(!allowlist.is_allowed("alice@sample.com", "email"));
        assert!(!allowlist.is_allowed("alice@demo.com", "email"));
        assert!(!allowlist.is_allowed("alice@mock.com", "email"));
        assert!(!allowlist.is_allowed("alice@test.com", "email"));
        // `.internal` is not an IANA special-use TLD.
        assert!(!allowlist.is_allowed("alice@corp.internal", "email"));
    }
}
