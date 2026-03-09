// SSRF (Server-Side Request Forgery) protection for browser navigation.
// Validates URLs against private network ranges and domain blocklists.

use std::fmt;
use std::net::IpAddr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

/// Configuration for SSRF protection policy.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SsrfConfig {
    /// Block requests to private/loopback networks (default: true).
    #[serde(default = "default_true")]
    pub block_private: bool,

    /// Glob patterns of domains to block (e.g. "*.malware.com", "evil.org").
    #[serde(default)]
    pub blocked_domains: Vec<String>,

    /// If non-empty, only these domains (glob patterns) are allowed (whitelist mode).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            block_private: true,
            blocked_domains: Vec::new(),
            allowed_domains: Vec::new(),
        }
    }
}

/// Reasons a URL can be rejected by SSRF policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyViolation {
    /// The host resolves to a private/loopback network address.
    PrivateNetwork(String),
    /// The domain matches a blocked pattern.
    BlockedDomain(String),
    /// The domain is not in the allowed whitelist.
    NotInAllowlist(String),
    /// The URL could not be parsed.
    InvalidUrl(String),
}

impl fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyViolation::PrivateNetwork(host) => {
                write!(f, "blocked: host '{host}' resolves to a private network")
            }
            PolicyViolation::BlockedDomain(domain) => {
                write!(f, "blocked: domain '{domain}' matches a block pattern")
            }
            PolicyViolation::NotInAllowlist(domain) => {
                write!(f, "blocked: domain '{domain}' is not in the allowlist")
            }
            PolicyViolation::InvalidUrl(reason) => {
                write!(f, "invalid URL: {reason}")
            }
        }
    }
}

impl std::error::Error for PolicyViolation {}

/// SSRF protection policy that validates URLs before navigation.
#[derive(Debug, Clone, Default)]
pub struct SsrfPolicy {
    config: SsrfConfig,
}

impl SsrfPolicy {
    pub fn new(config: SsrfConfig) -> Self {
        Self { config }
    }

    /// Validate a URL against the SSRF policy.
    pub fn check_url(&self, url_str: &str) -> Result<(), PolicyViolation> {
        let parsed = Url::parse(url_str)
            .map_err(|e| PolicyViolation::InvalidUrl(e.to_string()))?;

        let host_str = parsed
            .host_str()
            .ok_or_else(|| PolicyViolation::InvalidUrl("no host in URL".to_string()))?;

        // Check private network blocking
        if self.config.block_private && self.is_private_host(host_str) {
            return Err(PolicyViolation::PrivateNetwork(host_str.to_string()));
        }

        // Allowlist mode: if non-empty, domain must match at least one pattern
        if !self.config.allowed_domains.is_empty() {
            let matched = self
                .config
                .allowed_domains
                .iter()
                .any(|pat| domain_matches(pat, host_str));
            if !matched {
                return Err(PolicyViolation::NotInAllowlist(host_str.to_string()));
            }
        }

        // Blocklist: reject if any pattern matches
        for pattern in &self.config.blocked_domains {
            if domain_matches(pattern, host_str) {
                return Err(PolicyViolation::BlockedDomain(host_str.to_string()));
            }
        }

        Ok(())
    }

    /// Check whether a host string refers to a private/loopback address.
    fn is_private_host(&self, host: &str) -> bool {
        // Strip IPv6 brackets if present
        let bare = host.trim_start_matches('[').trim_end_matches(']');

        // Well-known loopback hostnames
        if bare.eq_ignore_ascii_case("localhost") {
            return true;
        }

        // Try parsing as IP address
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return is_private_ip(ip);
        }

        false
    }
}

/// Returns true if the IP address belongs to a private or loopback range.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 127.0.0.0/8 loopback
            octets[0] == 127
            // 10.0.0.0/8 private
            || octets[0] == 10
            // 172.16.0.0/12 private
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            // 192.168.0.0/16 private
            || (octets[0] == 192 && octets[1] == 168)
            // 0.0.0.0
            || v4.is_unspecified()
            // 169.254.0.0/16 link-local
            || (octets[0] == 169 && octets[1] == 254)
        }
        IpAddr::V6(v6) => {
            // ::1 loopback
            v6.is_loopback()
            // :: unspecified
            || v6.is_unspecified()
            // IPv4-mapped addresses (::ffff:x.x.x.x)
            || v6.to_ipv4_mapped().is_some_and(|v4| is_private_ip(IpAddr::V4(v4)))
        }
    }
}

/// Match a domain against a glob-like pattern.
/// Supports `*.example.com` (matches any subdomain) and exact matches.
fn domain_matches(pattern: &str, domain: &str) -> bool {
    let pattern_lower = pattern.to_ascii_lowercase();
    let domain_lower = domain.to_ascii_lowercase();

    if let Some(suffix) = pattern_lower.strip_prefix("*.") {
        // *.example.com matches example.com itself and any subdomain
        domain_lower == suffix || domain_lower.ends_with(&format!(".{suffix}"))
    } else {
        domain_lower == pattern_lower
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_localhost() {
        let policy = SsrfPolicy::default();

        assert!(matches!(
            policy.check_url("http://localhost/path"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        assert!(matches!(
            policy.check_url("http://127.0.0.1:8080/api"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        assert!(matches!(
            policy.check_url("http://[::1]/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
    }

    #[test]
    fn test_blocks_private_networks() {
        let policy = SsrfPolicy::default();

        // 10.x.x.x
        assert!(matches!(
            policy.check_url("http://10.0.0.1/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 172.16.x.x
        assert!(matches!(
            policy.check_url("http://172.16.0.1/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 172.31.x.x (upper bound)
        assert!(matches!(
            policy.check_url("http://172.31.255.255/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
        // 192.168.x.x
        assert!(matches!(
            policy.check_url("http://192.168.1.1/"),
            Err(PolicyViolation::PrivateNetwork(_))
        ));
    }

    #[test]
    fn test_allows_public_urls() {
        let policy = SsrfPolicy::default();

        assert!(policy.check_url("https://example.com/page").is_ok());
        assert!(policy.check_url("https://8.8.8.8/dns").is_ok());
        assert!(policy.check_url("https://172.32.0.1/").is_ok()); // 172.32 is NOT private
    }

    #[test]
    fn test_blocked_domain_patterns() {
        let policy = SsrfPolicy::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![
                "*.malware.com".to_string(),
                "evil.org".to_string(),
            ],
            allowed_domains: vec![],
        });

        // Subdomain match
        assert!(matches!(
            policy.check_url("https://payload.malware.com/x"),
            Err(PolicyViolation::BlockedDomain(_))
        ));
        // Bare domain match for wildcard
        assert!(matches!(
            policy.check_url("https://malware.com/x"),
            Err(PolicyViolation::BlockedDomain(_))
        ));
        // Exact match
        assert!(matches!(
            policy.check_url("https://evil.org/"),
            Err(PolicyViolation::BlockedDomain(_))
        ));
        // Non-matching domain is fine
        assert!(policy.check_url("https://safe.com/").is_ok());
    }

    #[test]
    fn test_allowed_domains_whitelist() {
        let policy = SsrfPolicy::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![
                "*.trusted.com".to_string(),
                "api.example.org".to_string(),
            ],
        });

        // Allowed
        assert!(policy.check_url("https://app.trusted.com/").is_ok());
        assert!(policy.check_url("https://api.example.org/v1").is_ok());

        // Not in allowlist
        assert!(matches!(
            policy.check_url("https://random.com/"),
            Err(PolicyViolation::NotInAllowlist(_))
        ));
    }

    #[test]
    fn test_disabled_ssrf_allows_everything() {
        let policy = SsrfPolicy::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
        });

        assert!(policy.check_url("http://localhost/").is_ok());
        assert!(policy.check_url("http://10.0.0.1/").is_ok());
        assert!(policy.check_url("http://192.168.1.1/").is_ok());
        assert!(policy.check_url("https://example.com/").is_ok());
    }

    #[test]
    fn test_invalid_url() {
        let policy = SsrfPolicy::default();

        assert!(matches!(
            policy.check_url("not-a-url"),
            Err(PolicyViolation::InvalidUrl(_))
        ));
    }

    #[test]
    fn test_policy_violation_display() {
        let v = PolicyViolation::PrivateNetwork("127.0.0.1".to_string());
        assert!(v.to_string().contains("private network"));

        let v = PolicyViolation::BlockedDomain("evil.com".to_string());
        assert!(v.to_string().contains("block pattern"));

        let v = PolicyViolation::NotInAllowlist("random.com".to_string());
        assert!(v.to_string().contains("allowlist"));

        let v = PolicyViolation::InvalidUrl("missing scheme".to_string());
        assert!(v.to_string().contains("invalid URL"));
    }
}
