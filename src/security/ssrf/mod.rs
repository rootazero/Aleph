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

/// Validates the URL scheme (only http and https are allowed).
pub(crate) fn validate_scheme(url: &Url) -> Result<(), SsrfError> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(SsrfError::InvalidUrl(format!(
            "unsupported scheme: {other}"
        ))),
    }
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

    let url = Url::parse(url_str).map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

    validate_scheme(&url)?;

    if !policy.enabled {
        return Ok((url, None));
    }

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
