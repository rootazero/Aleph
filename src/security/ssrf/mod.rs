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

    #[error("hostname requires DNS resolution; use validate_url_async instead")]
    RequiresDnsResolution(String),

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
///
/// Only performs IP-literal / scheme / credential / hostname blocklist checks.
/// For `Host::Domain` (i.e. names that need DNS to be evaluated against
/// blocked-IP ranges) this returns `SsrfError::RequiresDnsResolution` —
/// callers MUST switch to `validate_url_async` to close the
/// hostname→private-IP bypass via DNS rebinding.
pub fn validate_url(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    if !policy.enabled {
        return Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()));
    }
    let url = validate_url_common(url_str, policy)?;

    let host = url.host_str().ok_or(SsrfError::NoHost)?;
    let allowlisted = is_allowlisted(host, &policy.allowed_hosts);
    if !allowlisted && matches!(url.host(), Some(url::Host::Domain(_))) {
        return Err(SsrfError::RequiresDnsResolution(host.to_string()));
    }
    Ok(url)
}

/// Validates a URL with DNS resolution and returns the pinned `SocketAddr`.
///
/// Performs full SSRF validation including DNS resolution to check IP addresses
/// against the policy. Returns `(Url, Option<SocketAddr>)` where the pinned
/// address MUST be used on the outbound HTTP client (via reqwest's
/// `.resolve(host, addr)`) to close the DNS rebinding TOCTOU window between
/// validation and fetch. Returns `None` for IP-literal URLs and when
/// `policy.enabled == false`.
///
/// Callers that perform outbound HTTP after validation should either pass the
/// pinned address to their HTTP client, or use [`safe_fetch`] which handles
/// pinning internally.
pub async fn validate_url_async(
    url_str: &str,
    policy: &SsrfPolicy,
) -> Result<(Url, Option<std::net::SocketAddr>), SsrfError> {
    validate_url_with_pinned(url_str, policy).await
}

/// Validates a URL and (when a hostname lookup was performed) returns the
/// `SocketAddr` that must be pinned on the outbound client to close the DNS
/// rebinding window between validation and `reqwest`'s own resolver. Returns
/// `(Url, None)` for IP-literal URLs and for `policy.enabled == false`;
/// hostname URLs return `(Url, Some(SocketAddr))` whose address has been
/// verified against the policy via [`super::dns::resolve_and_validate`].
pub async fn validate_url_with_pinned(
    url_str: &str,
    policy: &SsrfPolicy,
) -> Result<(Url, Option<std::net::SocketAddr>), SsrfError> {
    use std::net::SocketAddr;

    if !policy.enabled {
        let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;
        return Ok((url, None));
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
        return Ok((url, None));
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
    let pinned: SocketAddr = dns::resolve_and_validate(host, port, dns_policy).await?;

    Ok((url, Some(pinned)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> SsrfPolicy {
        SsrfPolicy::default()
    }

    // --- Backward-compatible validate_url tests (matching original) ---

    #[tokio::test]
    async fn allows_public_url() {
        let policy = default_policy();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "api.example.com".to_string(),
            vec!["8.8.8.8".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let result = validate_url_async("https://api.example.com/v1/data", &policy).await;
        assert!(result.is_ok(), "public URL should be allowed: {:?}", result);
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

    #[test]
    fn sync_requires_dns_resolution_for_hostnames() {
        // Hostname URLs without DNS rebinding protection MUST be rejected by
        // the sync path. This is the fail-closed contract that closes the
        // SSRF bypass where a hostname resolves to a private IP after sync
        // validation has already returned Ok.
        let policy = default_policy();
        let result = validate_url("https://attacker-controlled.test/foo", &policy);
        assert!(
            matches!(result, Err(SsrfError::RequiresDnsResolution(_))),
            "sync validate_url must require DNS for non-allowlisted hostnames: {:?}",
            result
        );
    }

    #[test]
    fn sync_allows_allowlisted_hostname_without_dns() {
        let policy = SsrfPolicy {
            allowed_hosts: vec!["cdn.example.com".to_string()],
            ..Default::default()
        };
        let result = validate_url("https://cdn.example.com/asset.js", &policy);
        assert!(
            result.is_ok(),
            "allowlisted hostname is exempt from DNS requirement: {:?}",
            result
        );
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

    #[tokio::test]
    async fn validate_url_with_pinned_returns_none_for_ip_literal() {
        let policy = default_policy();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "api.example.com".to_string(),
            vec!["8.8.8.8".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let (url, pinned) = validate_url_with_pinned("https://8.8.8.8/path", &policy)
            .await
            .expect("public IP literal must validate");
        assert_eq!(url.host_str(), Some("8.8.8.8"));
        assert!(
            pinned.is_none(),
            "IP literal needs no DNS pin — got {pinned:?}"
        );
    }

    #[tokio::test]
    async fn validate_url_with_pinned_returns_pinned_addr_for_hostname() {
        let policy = default_policy();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "api.example.com".to_string(),
            vec!["8.8.8.8".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let (url, pinned) = validate_url_with_pinned("https://api.example.com/v1/data", &policy)
            .await
            .expect("public hostname must validate");
        assert_eq!(url.host_str(), Some("api.example.com"));
        let addr = pinned.expect("hostname must produce a pinned SocketAddr");
        assert_eq!(addr.ip(), "8.8.8.8".parse::<std::net::IpAddr>().unwrap());
        assert_eq!(addr.port(), 443, "https default port must be used");
    }

    #[tokio::test]
    async fn validate_url_with_pinned_rejects_hostname_resolving_to_blocked_ip() {
        let policy = default_policy();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "evil.example".to_string(),
            vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let result = validate_url_with_pinned("http://evil.example/admin", &policy).await;
        assert!(
            matches!(result, Err(SsrfError::BlockedAddress(_))),
            "hostname → loopback must fail-closed — got {result:?}"
        );
    }

    #[tokio::test]
    async fn validate_url_with_pinned_returns_none_when_policy_disabled() {
        let policy = SsrfPolicy::disabled();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "anything.example".to_string(),
            vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let (url, pinned) = validate_url_with_pinned("http://anything.example/", &policy)
            .await
            .expect("disabled policy bypasses validation");
        assert_eq!(url.host_str(), Some("anything.example"));
        assert!(
            pinned.is_none(),
            "disabled policy yields no pin — caller uses normal client"
        );
    }

    #[tokio::test]
    async fn validate_url_with_pinned_blocks_benchmark_range() {
        let policy = default_policy();
        let mut map = std::collections::HashMap::new();
        map.insert(
            "bench.example".to_string(),
            vec!["198.18.0.5".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);
        let result = validate_url_with_pinned("http://bench.example/x", &policy).await;
        assert!(
            matches!(result, Err(SsrfError::BlockedAddress(_))),
            "198.18.0.0/15 resolution must remain blocked — got {result:?}"
        );
    }
}
