//! DNS resolution with SSRF validation and address pinning.
//!
//! Resolves hostnames via tokio DNS and validates every returned IP address
//! against the SSRF blocklist. Returns a single pinned `SocketAddr` for use
//! with reqwest's `.resolve()` to prevent DNS rebinding attacks.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use super::ip::is_ip_blocked_by_policy;
use super::policy::SsrfPolicy;
use super::SsrfError;

#[cfg(test)]
pub(crate) mod test_hook {
    use super::*;

    static RESOLVER: Mutex<Option<HashMap<String, Vec<IpAddr>>>> = Mutex::new(None);

    pub(crate) fn install(map: HashMap<String, Vec<IpAddr>>) {
        *RESOLVER.lock().unwrap() = Some(map);
    }

    pub(crate) fn clear() {
        *RESOLVER.lock().unwrap() = None;
    }

    pub(crate) fn lookup(host: &str) -> Option<Vec<IpAddr>> {
        RESOLVER
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(host).cloned())
    }
}

/// Resolves a host:port pair and validates all returned IPs against the policy.
///
/// If the host is already an IP literal, validates it directly without DNS lookup.
/// Otherwise performs async DNS resolution and checks every returned address.
/// Returns the first valid `SocketAddr` for connection pinning.
pub(crate) async fn resolve_and_validate(
    host: &str,
    port: u16,
    policy: &SsrfPolicy,
) -> Result<SocketAddr, SsrfError> {
    // If host is already an IP literal, validate directly
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_ip_blocked_by_policy(ip, policy) {
            return Err(SsrfError::BlockedAddress(ip.to_string()));
        }
        return Ok(SocketAddr::new(ip, port));
    }

    // Test-only resolver override — bypasses `tokio::net::lookup_host` so a
    // deterministic hostname→IP mapping can be supplied without touching
    // public DNS (and without waiting for network). Production behavior is
    // untouched: outside `cfg(test)` the hook is absent and the OS resolver
    // path runs as before.
    let addrs: Vec<SocketAddr> = {
        #[cfg(test)]
        {
            if let Some(ips) = test_hook::lookup(host) {
                let collected: Vec<SocketAddr> =
                    ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect();
                collected
            } else {
                lookup(host, port).await?
            }
        }
        #[cfg(not(test))]
        {
            lookup(host, port).await?
        }
    };

    if addrs.is_empty() {
        return Err(SsrfError::DnsResolutionFailed {
            host: host.to_string(),
            reason: "no addresses returned".to_string(),
        });
    }

    // Validate ALL returned IPs — if any is blocked, reject the entire request
    for addr in &addrs {
        if is_ip_blocked_by_policy(addr.ip(), policy) {
            return Err(SsrfError::BlockedAddress(addr.ip().to_string()));
        }
    }

    // Return the first valid address for pinning
    Ok(addrs[0])
}

async fn lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, SsrfError> {
    let lookup_addr = format!("{host}:{port}");
    tokio::net::lookup_host(&lookup_addr)
        .await
        .map_err(|e| SsrfError::DnsResolutionFailed {
            host: host.to_string(),
            reason: e.to_string(),
        })
        .map(|stream| stream.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ip_literal_passthrough_public() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("8.8.8.8", 80, &policy).await;
        assert!(result.is_ok());
        let addr = result.unwrap();
        assert_eq!(addr.ip(), "8.8.8.8".parse::<IpAddr>().unwrap());
        assert_eq!(addr.port(), 80);
    }

    #[tokio::test]
    async fn blocks_private_ip_literal() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("10.0.0.1", 443, &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[tokio::test]
    async fn blocks_loopback_ip_literal() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("127.0.0.1", 80, &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[tokio::test]
    async fn blocks_ipv6_loopback_literal() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("::1", 80, &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[tokio::test]
    async fn blocks_cloud_metadata_ip() {
        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("169.254.169.254", 80, &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }

    #[tokio::test]
    async fn allows_private_with_policy() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        let result = resolve_and_validate("192.168.1.1", 80, &policy).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn policy_allow_private_still_blocks_loopback() {
        let policy = SsrfPolicy {
            allow_private_network: true,
            ..Default::default()
        };
        let result = resolve_and_validate("127.0.0.1", 80, &policy).await;
        assert!(matches!(result, Err(SsrfError::BlockedAddress(_))));
    }
}
