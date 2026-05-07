// SSRF (Server-Side Request Forgery) protection for browser navigation.
// Thin wrapper over the core SSRF engine (`crate::security::ssrf`).

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::security::ssrf::{self, SsrfPolicy as CoreSsrfPolicy};

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

impl From<ssrf::SsrfError> for PolicyViolation {
    fn from(err: ssrf::SsrfError) -> Self {
        match &err {
            ssrf::SsrfError::InvalidUrl(_) | ssrf::SsrfError::NoHost => {
                PolicyViolation::InvalidUrl(err.to_string())
            }
            ssrf::SsrfError::BlockedAddress(addr) => {
                // Distinguish private-network blocks from domain blocks.
                // The core engine prefixes blocklist hits with "host in blocklist: ".
                if let Some(domain) = addr.strip_prefix("host in blocklist: ") {
                    PolicyViolation::BlockedDomain(domain.to_string())
                } else {
                    PolicyViolation::PrivateNetwork(addr.clone())
                }
            }
            ssrf::SsrfError::DnsResolutionFailed { host, .. } => {
                PolicyViolation::PrivateNetwork(host.clone())
            }
            ssrf::SsrfError::TooManyRedirects(_) | ssrf::SsrfError::FetchFailed(_) => {
                PolicyViolation::InvalidUrl(err.to_string())
            }
        }
    }
}

/// SSRF protection guard for browser navigation.
/// Delegates to the core SSRF engine for IP/hostname validation.
#[derive(Debug, Clone, Default)]
pub struct BrowserSsrfGuard {
    config: SsrfConfig,
}

impl BrowserSsrfGuard {
    pub fn new(config: SsrfConfig) -> Self {
        Self { config }
    }

    /// Validate a URL against the SSRF policy.
    pub fn check_url(&self, url_str: &str) -> Result<(), PolicyViolation> {
        // Build core policy from browser config.
        // When block_private is false, disable SSRF protection entirely so that
        // loopback, localhost, and private ranges are all reachable (useful for
        // local development and self-hosted deployments).
        let core_policy = if !self.config.block_private
            && self.config.blocked_domains.is_empty()
            && self.config.allowed_domains.is_empty()
        {
            CoreSsrfPolicy::disabled()
        } else {
            CoreSsrfPolicy {
                enabled: true,
                allow_private_network: !self.config.block_private,
                allowed_hosts: self.config.allowed_domains.clone(),
                blocked_hosts: self.config.blocked_domains.clone(),
                ..CoreSsrfPolicy::default()
            }
        };

        // Delegate to core engine (sync validation)
        ssrf::validate_url(url_str, &core_policy).map_err(PolicyViolation::from)?;

        // Additional browser-specific: allowlist-only mode
        // (when allowed_domains is non-empty, ONLY those domains are permitted)
        if !self.config.allowed_domains.is_empty() {
            let url =
                url::Url::parse(url_str).map_err(|e| PolicyViolation::InvalidUrl(e.to_string()))?;
            if let Some(host) = url.host_str() {
                // The core engine's allowlist BYPASSES blocks, but browser needs allowlist-ONLY mode.
                // Check if host matches any allowed domain pattern.
                let matched = self.config.allowed_domains.iter().any(|pat| {
                    let pat_lower = pat.to_ascii_lowercase();
                    let host_lower = host.to_ascii_lowercase();
                    if let Some(base) = pat_lower.strip_prefix("*.") {
                        host_lower == base || host_lower.ends_with(&format!(".{base}"))
                    } else {
                        host_lower == pat_lower
                    }
                });
                if !matched {
                    return Err(PolicyViolation::NotInAllowlist(host.to_string()));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocks_localhost() {
        let policy = BrowserSsrfGuard::default();

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
        let policy = BrowserSsrfGuard::default();

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
        let policy = BrowserSsrfGuard::default();

        assert!(policy.check_url("https://example.com/page").is_ok());
        assert!(policy.check_url("https://8.8.8.8/dns").is_ok());
        assert!(policy.check_url("https://172.32.0.1/").is_ok()); // 172.32 is NOT private
    }

    #[test]
    fn test_blocked_domain_patterns() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec!["*.malware.com".to_string(), "evil.org".to_string()],
            allowed_domains: vec![],
        });

        // Subdomain match
        assert!(
            policy.check_url("https://payload.malware.com/x").is_err(),
            "subdomain of blocked wildcard should be blocked"
        );
        // Bare domain match for wildcard
        assert!(
            policy.check_url("https://malware.com/x").is_err(),
            "bare domain of blocked wildcard should be blocked"
        );
        // Exact match
        assert!(
            policy.check_url("https://evil.org/").is_err(),
            "exact blocked domain should be blocked"
        );
        // Non-matching domain is fine
        assert!(policy.check_url("https://safe.com/").is_ok());
    }

    #[test]
    fn test_allowed_domains_whitelist() {
        let policy = BrowserSsrfGuard::new(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec!["*.trusted.com".to_string(), "api.example.org".to_string()],
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
        let policy = BrowserSsrfGuard::new(SsrfConfig {
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
        let policy = BrowserSsrfGuard::default();

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
