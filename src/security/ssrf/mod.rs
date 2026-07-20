//! SSRF protection engine.
//!
//! Validates URLs before outbound HTTP requests to prevent Server-Side Request Forgery attacks.
//! Blocks private networks, loopback addresses, cloud metadata endpoints, legacy IP encodings,
//! and performs DNS rebinding defense by resolving and validating all returned IPs.
//!
//! # Architecture
//!
//! - `policy` — Configuration struct controlling SSRF behavior
//! - `ip` — IPv4/IPv6 classification against blocked ranges
//! - `hostname` — Hostname blocklist, allowlist, legacy IP literal detection
//! - `dns` — Async DNS resolution with address pinning
//! - `fetch` — Full `safe_fetch` with redirect chain validation

pub(crate) mod dns;
pub mod fetch;
pub(crate) mod hostname;
pub(crate) mod ip;
pub mod policy;

// Re-export public API for backward compatibility
pub use fetch::{safe_fetch, SafeFetchRequest, SafeFetchResponse};
pub use policy::SsrfPolicy;

use std::net::IpAddr;

use url::Url;

use self::hostname::{
    has_url_credentials, is_allowlisted, is_blocked_hostname, is_blocklisted, is_legacy_ip_literal,
};
use self::ip::is_ip_blocked_by_policy;

/// Errors returned by SSRF validation.
#[derive(Debug, thiserror::Error)]
pub enum SsrfError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("blocked address: {0}")]
    BlockedAddress(String),

    #[error("DNS resolution failed for {host}: {reason}")]
    DnsResolutionFailed { host: String, reason: String },

    #[error("URL has no host")]
    NoHost,

    #[error("too many redirects (limit: {0})")]
    TooManyRedirects(u8),

    #[error("fetch failed: {0}")]
    FetchFailed(String),
}

fn validate_url_common(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    if is_legacy_ip_literal(host) {
        return Err(SsrfError::BlockedAddress(format!(
            "legacy IP literal: {host}"
        )));
    }

    if has_url_credentials(url_str) {
        return Err(SsrfError::InvalidUrl(
            "URL contains embedded credentials".to_string(),
        ));
    }

    if is_allowlisted(host, &policy.allowed_hosts) {
        return Ok(url);
    }

    if is_blocked_hostname(host) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }

    if is_blocklisted(host, &policy.blocked_hosts) {
        return Err(SsrfError::BlockedAddress(format!(
            "host in blocklist: {host}"
        )));
    }

    if let Some(ip) = match url.host() {
        Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
        _ => None,
    } {
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
    }

    Ok(url)
}

/// Validates a URL synchronously (no DNS resolution).
pub fn validate_url(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    if !policy.enabled {
        return Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()));
    }
    validate_url_common(url_str, policy)
}

pub async fn validate_url_async(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    if !policy.enabled {
        return Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()));
    }

    let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    if is_legacy_ip_literal(host) {
        return Err(SsrfError::BlockedAddress(format!(
            "legacy IP literal: {host}"
        )));
    }

    if has_url_credentials(url_str) {
        return Err(SsrfError::InvalidUrl(
            "URL contains embedded credentials".to_string(),
        ));
    }

    let allowlisted = is_allowlisted(host, &policy.allowed_hosts);

    if !allowlisted {
        if is_blocked_hostname(host) {
            return Err(SsrfError::BlockedAddress(host.to_string()));
        }
        if is_blocklisted(host, &policy.blocked_hosts) {
            return Err(SsrfError::BlockedAddress(format!(
                "host in blocklist: {host}"
            )));
        }
    }

    if matches!(
        url.host(),
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_))
    ) {
        if let Some(ip) = match url.host() {
            Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
            Some(url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
            _ => None,
        } {
            if is_ip_blocked_by_policy(ip, policy) {
                return Err(SsrfError::BlockedAddress(ip.to_string()));
            }
        }
        return Ok(url);
    }

    // DNS resolution — validate all returned IPs via dns module.
    // For allowlisted hosts, classify the resolved IP with the relaxed
    // `for_allowlisted_host()` policy so DNS rebinding cannot reach
    // 127.0.0.1 / 169.254.169.254 even when the name is on the allowlist.
    // This matches `safe_fetch::validate_url_full` semantics.
    let port = url.port_or_known_default().unwrap_or(80);
    let dns_policy = if allowlisted {
        &SsrfPolicy::for_allowlisted_host()
    } else {
        policy
    };
    dns::resolve_and_validate(host, port, dns_policy).await?;

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> SsrfPolicy {
        SsrfPolicy::default()
    }

    // --- Backward-compatible validate_url tests (matching original) ---

    #[test]
    fn allows_public_url() {
        let policy = default_policy();
        let result = validate_url("https://api.example.com/v1/data", &policy);
        assert!(result.is_ok(), "public URL should be allowed");
    }

    #[test]
    fn blocks_localhost() {
        let policy = default_policy();
        let result = validate_url("http://localhost:8080/api", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_loopback_ip() {
        let policy = default_policy();
        let result = validate_url("http://127.0.0.1/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_private_10_network() {
        let policy = default_policy();
        let result = validate_url("http://10.0.0.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_private_172_network() {
        let policy = default_policy();
        let result = validate_url("http://172.16.0.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_private_192_network() {
        let policy = default_policy();
        let result = validate_url("http://192.168.1.100/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_metadata_endpoint() {
        let policy = default_policy();
        let result = validate_url("http://169.254.169.254/latest/meta-data/", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_metadata_hostname() {
        let policy = default_policy();
        let result = validate_url(
            "http://metadata.google.internal/computeMetadata/v1/",
            &policy,
        );
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));

        let result2 = validate_url("http://metadata.internal/", &policy);
        assert!(matches!(result2, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_ipv6_loopback() {
        let policy = default_policy();
        let result = validate_url("http://[::1]/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6() {
        let policy = default_policy();
        let result = validate_url("http://[::ffff:127.0.0.1]/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_link_local() {
        let policy = default_policy();
        let result = validate_url("http://169.254.1.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn allowlist_exact() {
        let policy = SsrfPolicy {
            allowed_hosts: vec!["internal.corp.example.com".to_string()],
            ..Default::default()
        };
        let result = validate_url("http://internal.corp.example.com/api", &policy);
        assert!(result.is_ok(), "allowlisted exact host should be permitted");
    }

    #[test]
    fn allowlist_wildcard() {
        let policy = SsrfPolicy {
            allowed_hosts: vec!["*.example.com".to_string()],
            ..Default::default()
        };
        let r1 = validate_url("http://api.example.com/v1", &policy);
        assert!(r1.is_ok(), "wildcard subdomain should match");

        let r2 = validate_url("http://sub.api.example.com/v1", &policy);
        assert!(r2.is_ok(), "deeper subdomain should match *.example.com");

        let r3 = validate_url("http://example.com/v1", &policy);
        assert!(r3.is_ok(), "bare domain should match *.example.com");
    }

    #[test]
    fn allow_private_network_flag() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        let r1 = validate_url("http://192.168.1.1/api", &policy);
        assert!(
            r1.is_ok(),
            "private IP allowed when allow_private_network is true"
        );

        let r2 = validate_url("http://127.0.0.1/admin", &policy);
        assert!(
            matches!(r2, Err(SsrfError::BlockedAddress(_))),
            "loopback should still be blocked"
        );

        let r3 = validate_url("http://169.254.169.254/meta", &policy);
        assert!(
            matches!(r3, Err(SsrfError::BlockedAddress(_))),
            "cloud metadata should still be blocked"
        );

        let r4 = validate_url("http://localhost/api", &policy);
        assert!(
            matches!(r4, Err(SsrfError::BlockedAddress(_))),
            "localhost hostname still blocked"
        );
    }

    #[test]
    fn invalid_url() {
        let policy = default_policy();
        let result = validate_url("not-a-url", &policy);
        assert!(matches!(result, Err(SsrfError::InvalidUrl(_))));
    }

    #[test]
    fn blocks_cgnat() {
        let policy = default_policy();
        let result = validate_url("http://100.64.0.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    // --- New features ---

    #[test]
    fn blocks_legacy_hex_ip() {
        let policy = default_policy();
        let result = validate_url("http://0x7f000001/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_url_with_credentials() {
        let policy = default_policy();
        let result = validate_url("http://admin:secret@example.com/", &policy);
        assert!(matches!(result, Err(SsrfError::InvalidUrl(_))));
    }

    #[test]
    fn blocks_user_blocklist() {
        let policy = SsrfPolicy {
            blocked_hosts: vec!["evil.com".to_string()],
            ..Default::default()
        };
        let result = validate_url("http://evil.com/malware", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn disabled_policy_allows_everything() {
        let policy = SsrfPolicy::disabled();
        assert!(validate_url("http://localhost/admin", &policy).is_ok());
        assert!(validate_url("http://127.0.0.1/secret", &policy).is_ok());
    }

    #[test]
    fn blocks_localhost_localdomain() {
        let policy = default_policy();
        let result = validate_url("http://localhost.localdomain/api", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn blocks_local_suffix() {
        let policy = default_policy();
        let result = validate_url("http://printer.local/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    // --- Async tests ---

    #[tokio::test]
    async fn async_allows_public_url() {
        let policy = default_policy();
        let result = validate_url_async("http://8.8.8.8/", &policy).await;
        assert!(result.is_ok(), "public IP should be allowed");
    }

    #[tokio::test]
    async fn async_blocks_localhost() {
        let policy = default_policy();
        let result = validate_url_async("http://localhost/admin", &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[tokio::test]
    async fn async_blocks_loopback_ip() {
        let policy = default_policy();
        let result = validate_url_async("http://127.0.0.1/admin", &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }
}
