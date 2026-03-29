//! SSRF protection engine.
//!
//! Validates URLs before outbound HTTP requests to prevent Server-Side Request Forgery attacks.
//! Blocks private networks, loopback addresses, cloud metadata endpoints, and performs
//! DNS rebinding defense by resolving and validating all returned IPs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use thiserror::Error;
use url::Url;

/// Policy controlling SSRF validation behavior.
#[derive(Debug, Clone)]
pub struct SsrfPolicy {
    /// Whether to allow requests to private/internal IP ranges.
    /// Even when true, cloud metadata endpoints and loopback remain blocked.
    pub allow_private_network: bool,
    /// Hosts that bypass the blocklist check. Supports exact matches
    /// (e.g., "api.example.com") and wildcard subdomain matches (e.g., "*.example.com").
    pub allowed_hosts: Vec<String>,
}

impl Default for SsrfPolicy {
    fn default() -> Self {
        Self {
            allow_private_network: false,
            allowed_hosts: Vec::new(),
        }
    }
}

/// Errors returned by SSRF validation.
#[derive(Debug, Error)]
pub enum SsrfError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("blocked address: {0}")]
    BlockedAddress(String),

    #[error("DNS resolution failed for {host}: {reason}")]
    DnsResolutionFailed { host: String, reason: String },

    #[error("URL has no host")]
    NoHost,
}

// --- IP classification helpers ---

/// Returns true if the IPv4 address falls in a private, loopback, link-local,
/// CGNAT, or otherwise non-routable range.
fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_private() || ip.is_link_local() {
        return true;
    }
    // CGNAT: 100.64.0.0/10
    let octets = ip.octets();
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return true;
    }
    false
}

/// Returns true if the IPv6 address is loopback, link-local, or unique-local.
fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    let segments = ip.segments();
    // Link-local: fe80::/10
    if (segments[0] & 0xFFC0) == 0xFE80 {
        return true;
    }
    // Unique local: fc00::/7
    if (segments[0] & 0xFE00) == 0xFC00 {
        return true;
    }
    false
}

/// Returns true if the IP address should be blocked (private ranges, cloud metadata, etc.).
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if is_private_ipv4(v4) {
                return true;
            }
            // Cloud metadata: 169.254.169.254
            if v4 == Ipv4Addr::new(169, 254, 169, 254) {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if is_private_ipv6(v6) {
                return true;
            }
            // IPv4-mapped IPv6 (e.g., ::ffff:127.0.0.1)
            if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                if is_private_ipv4(mapped_v4) || mapped_v4 == Ipv4Addr::new(169, 254, 169, 254) {
                    return true;
                }
            }
            false
        }
    }
}

// --- Policy-aware IP check ------------------------------------------------

/// Returns true if the IP should be blocked under the given policy.
///
/// When `allow_private_network` is true, only loopback and cloud metadata
/// (169.254.169.254) are blocked.  Otherwise the full `is_blocked_ip` check
/// applies.
fn is_ip_blocked_by_policy(ip: IpAddr, policy: &SsrfPolicy) -> bool {
    if policy.allow_private_network {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback() || v4 == Ipv4Addr::new(169, 254, 169, 254)
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.to_ipv4_mapped().map_or(false, |m| {
                        m.is_loopback() || m == Ipv4Addr::new(169, 254, 169, 254)
                    })
            }
        }
    } else {
        is_blocked_ip(ip)
    }
}

// --- Allowlist matching ---

/// Returns true if the hostname matches any entry in the allowlist.
/// Supports exact case-insensitive match and wildcard subdomain (*.example.com).
fn is_allowlisted(hostname: &str, allowed_hosts: &[String]) -> bool {
    let hostname_lower = hostname.to_lowercase();
    for pattern in allowed_hosts {
        let pattern_lower = pattern.to_lowercase();
        if pattern_lower.starts_with("*.") {
            // Wildcard: *.example.com matches example.com and sub.example.com
            let base = &pattern_lower[2..]; // strip "*."
            if hostname_lower == base || hostname_lower.ends_with(&format!(".{}", base)) {
                return true;
            }
        } else if hostname_lower == pattern_lower {
            return true;
        }
    }
    false
}

// --- Blocked hostname check ---

/// Returns true if the hostname string itself is on the blocklist.
fn is_blocked_hostname(hostname: &str) -> bool {
    let lower = hostname.to_lowercase();
    matches!(
        lower.as_str(),
        "localhost"
            | "metadata.google.internal"
            | "metadata.internal"
    )
}

// --- Public API ---

/// Validates a URL synchronously (no DNS resolution).
///
/// Checks:
/// 1. URL is parseable.
/// 2. URL has a host.
/// 3. Host is not on the hostname blocklist (unless allowlisted).
/// 4. If the host is a literal IP, it is not in a blocked range (unless `allow_private_network`
///    and it is not a cloud-metadata or loopback address).
pub fn validate_url(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    let url = Url::parse(url_str)
        .map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    // Allowlist check — allowlisted hosts bypass all further validation.
    if is_allowlisted(host, &policy.allowed_hosts) {
        return Ok(url);
    }

    // Hostname blocklist (hardcoded names like "localhost", cloud metadata).
    if is_blocked_hostname(host) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }

    // If the host is a literal IP address, validate it immediately.
    // Use url.host() for proper IPv6 parsing (host_str() strips brackets but
    // parse::<IpAddr>() may still fail on IPv4-mapped forms like ::ffff:127.0.0.1).
    let ip_from_url = match url.host() {
        Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
        _ => None,
    };

    if let Some(ip) = ip_from_url {
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
    }

    Ok(url)
}

/// Validates a URL asynchronously, including DNS rebinding defense.
///
/// Performs all sync checks, then resolves the hostname via DNS and validates
/// every returned IP address against the blocklist.
pub async fn validate_url_async(url_str: &str, policy: &SsrfPolicy) -> Result<Url, SsrfError> {
    let url = Url::parse(url_str)
        .map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

    let host = url.host_str().ok_or(SsrfError::NoHost)?;

    // Allowlist check — skip DNS validation for explicitly allowed hosts.
    if is_allowlisted(host, &policy.allowed_hosts) {
        return Ok(url);
    }

    // Hostname blocklist.
    if is_blocked_hostname(host) {
        return Err(SsrfError::BlockedAddress(host.to_string()));
    }

    // If the host is a literal IP, validate it immediately (no DNS needed).
    let ip_from_url = match url.host() {
        Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
        _ => None,
    };

    if let Some(ip) = ip_from_url {
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
        return Ok(url);
    }

    // DNS resolution — check all returned IPs.
    let port = url.port_or_known_default().unwrap_or(80);
    let lookup_addr = format!("{}:{}", host, port);
    let addrs = tokio::net::lookup_host(lookup_addr)
        .await
        .map_err(|e| SsrfError::DnsResolutionFailed {
            host: host.to_string(),
            reason: e.to_string(),
        })?;

    for socket_addr in addrs {
        let ip = socket_addr.ip();
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
    }

    Ok(url)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> SsrfPolicy {
        SsrfPolicy::default()
    }

    #[test]
    fn test_allows_public_url() {
        let policy = default_policy();
        let result = validate_url("https://api.example.com/v1/data", &policy);
        assert!(result.is_ok(), "public URL should be allowed");
    }

    #[test]
    fn test_blocks_localhost() {
        let policy = default_policy();
        let result = validate_url("http://localhost:8080/api", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_loopback_ip() {
        let policy = default_policy();
        let result = validate_url("http://127.0.0.1/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_private_10_network() {
        let policy = default_policy();
        let result = validate_url("http://10.0.0.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_private_172_network() {
        let policy = default_policy();
        let result = validate_url("http://172.16.0.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_private_192_network() {
        let policy = default_policy();
        let result = validate_url("http://192.168.1.100/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_metadata_endpoint() {
        let policy = default_policy();
        let result = validate_url("http://169.254.169.254/latest/meta-data/", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_metadata_hostname() {
        let policy = default_policy();
        let result = validate_url("http://metadata.google.internal/computeMetadata/v1/", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));

        let result2 = validate_url("http://metadata.internal/", &policy);
        assert!(matches!(result2, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_ipv6_loopback() {
        let policy = default_policy();
        let result = validate_url("http://[::1]/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_ipv4_mapped_ipv6() {
        let policy = default_policy();
        let result = validate_url("http://[::ffff:127.0.0.1]/admin", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_blocks_link_local() {
        let policy = default_policy();
        // IPv4 link-local
        let result = validate_url("http://169.254.1.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[test]
    fn test_allowlist_exact() {
        let policy = SsrfPolicy {
            allow_private_network: false,
            allowed_hosts: vec!["internal.corp.example.com".to_string()],
        };
        // Exact match on allowlisted host bypasses blocklist.
        let result = validate_url("http://internal.corp.example.com/api", &policy);
        assert!(result.is_ok(), "allowlisted exact host should be permitted");

        // Other hosts still blocked.
        let blocked = validate_url("http://other.internal.com/api", &policy);
        assert!(blocked.is_ok(), "public host not on allowlist should be allowed normally");
    }

    #[test]
    fn test_allowlist_wildcard() {
        let policy = SsrfPolicy {
            allow_private_network: false,
            allowed_hosts: vec!["*.example.com".to_string()],
        };
        // Subdomain match.
        let r1 = validate_url("http://api.example.com/v1", &policy);
        assert!(r1.is_ok(), "wildcard subdomain should match");

        // Deeper nesting.
        let r2 = validate_url("http://sub.api.example.com/v1", &policy);
        assert!(r2.is_ok(), "deeper subdomain should also match *.example.com");

        // Bare domain.
        let r3 = validate_url("http://example.com/v1", &policy);
        assert!(r3.is_ok(), "bare domain should match *.example.com");

        // Non-matching.
        let r4 = validate_url("http://other.org/v1", &policy);
        assert!(r4.is_ok(), "other.org is a public URL and should pass normally");
    }

    #[test]
    fn test_allow_private_network_flag() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            allowed_hosts: vec![],
        };
        // Private IP should now be allowed.
        let r1 = validate_url("http://192.168.1.1/api", &policy);
        assert!(r1.is_ok(), "private IP allowed when allow_private_network is true");

        // But loopback is still blocked.
        let r2 = validate_url("http://127.0.0.1/admin", &policy);
        assert!(
            matches!(r2, Err(SsrfError::BlockedAddress(_))),
            "loopback should still be blocked even with allow_private_network"
        );

        // And cloud metadata is still blocked.
        let r3 = validate_url("http://169.254.169.254/meta", &policy);
        assert!(
            matches!(r3, Err(SsrfError::BlockedAddress(_))),
            "cloud metadata should still be blocked even with allow_private_network"
        );

        // Blocked hostname still blocked.
        let r4 = validate_url("http://localhost/api", &policy);
        assert!(
            matches!(r4, Err(SsrfError::BlockedAddress(_))),
            "localhost hostname still blocked with allow_private_network"
        );
    }

    #[test]
    fn test_invalid_url() {
        let policy = default_policy();
        let result = validate_url("not-a-url", &policy);
        assert!(matches!(result, Err(SsrfError::InvalidUrl(_))));
    }

    #[test]
    fn test_blocks_cgnat() {
        let policy = default_policy();
        // 100.64.0.1 is in CGNAT range 100.64.0.0/10
        let result = validate_url("http://100.64.0.1/internal", &policy);
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[tokio::test]
    async fn test_async_allows_public_url() {
        let policy = default_policy();
        // Use an IP directly to avoid real DNS in tests.
        // 8.8.8.8 is a public Google DNS IP.
        let result = validate_url_async("http://8.8.8.8/", &policy).await;
        assert!(result.is_ok(), "public IP should be allowed in async validation");
    }

    #[tokio::test]
    async fn test_async_blocks_localhost() {
        let policy = default_policy();
        let result = validate_url_async("http://localhost/admin", &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }
}
