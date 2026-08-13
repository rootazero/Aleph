//! DNS resolution with SSRF validation and address pinning.
//!
//! Resolves hostnames via tokio DNS and validates every returned IP address
//! against the SSRF blocklist. Returns a single pinned `SocketAddr` for use
//! with reqwest's `.resolve()` to prevent DNS rebinding attacks.

use std::net::{IpAddr, SocketAddr};

use super::ip::is_ip_blocked_by_policy;
use super::policy::SsrfPolicy;
use super::SsrfError;

#[cfg(test)]
pub(crate) mod test_hook {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    static RESOLVER: Mutex<Option<HashMap<String, Vec<IpAddr>>>> = Mutex::new(None);

    static RESOLVER_GUARD: Mutex<()> = Mutex::new(());

    pub(crate) struct ResolverScope {
        _guard: MutexGuard<'static, ()>,
        previous: Option<HashMap<String, Vec<IpAddr>>>,
    }

    impl ResolverScope {
        pub(crate) fn install(map: HashMap<String, Vec<IpAddr>>) -> Self {
            let guard = RESOLVER_GUARD.lock().unwrap_or_else(|e| e.into_inner());
            let previous = {
                let mut resolver = RESOLVER.lock().unwrap_or_else(|e| e.into_inner());
                let prev = resolver.take();
                *resolver = Some(map);
                prev
            };
            Self {
                _guard: guard,
                previous,
            }
        }
    }

    impl Drop for ResolverScope {
        fn drop(&mut self) {
            let mut resolver = RESOLVER.lock().unwrap_or_else(|e| e.into_inner());
            *resolver = self.previous.take();
        }
    }

    pub(crate) fn lookup(host: &str) -> Option<Vec<IpAddr>> {
        RESOLVER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
                let collected: Vec<SocketAddr> = ips
                    .into_iter()
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect();
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

    #[test]
    fn resolver_scope_installs_map_and_restores_on_drop() {
        assert!(
            test_hook::lookup("pinned.example").is_none(),
            "RESOLVER starts empty — sanity check"
        );
        let mut map = std::collections::HashMap::new();
        map.insert(
            "pinned.example".to_string(),
            vec!["1.1.1.1".parse::<IpAddr>().unwrap()],
        );
        let scope = test_hook::ResolverScope::install(map);
        assert_eq!(
            test_hook::lookup("pinned.example")
                .as_ref()
                .map(|v| v.len()),
            Some(1)
        );
        drop(scope);
        assert!(
            test_hook::lookup("pinned.example").is_none(),
            "scope Drop must restore RESOLVER to its prior state"
        );
    }

    #[test]
    fn resolver_scope_saves_and_restores_previous_map() {
        let mut first = std::collections::HashMap::new();
        first.insert(
            "first.example".to_string(),
            vec!["1.1.1.1".parse::<IpAddr>().unwrap()],
        );
        let scope_a = test_hook::ResolverScope::install(first);
        assert!(test_hook::lookup("first.example").is_some());

        drop(scope_a);
        assert!(
            test_hook::lookup("first.example").is_none(),
            "first map must be cleared before next install"
        );

        let mut second = std::collections::HashMap::new();
        second.insert(
            "second.example".to_string(),
            vec!["8.8.8.8".parse::<IpAddr>().unwrap()],
        );
        let scope_b = test_hook::ResolverScope::install(second);
        assert!(
            test_hook::lookup("first.example").is_none(),
            "old map must remain cleared after sequential scopes"
        );
        assert!(test_hook::lookup("second.example").is_some());
        drop(scope_b);
        assert!(test_hook::lookup("second.example").is_none());
    }

    #[tokio::test]
    async fn resolver_scope_overrides_then_restores_for_resolve_and_validate() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "pinned.example".to_string(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()],
        );
        let _scope = test_hook::ResolverScope::install(map);

        let policy = SsrfPolicy::default();
        let result = resolve_and_validate("pinned.example", 80, &policy).await;
        assert!(
            matches!(result, Err(SsrfError::BlockedAddress(_))),
            "resolver-scope override must be honored by resolve_and_validate — got {result:?}"
        );
    }

    #[test]
    fn resolver_scope_restores_even_when_scope_body_panics() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "panic.example".to_string(),
            vec!["1.1.1.1".parse::<IpAddr>().unwrap()],
        );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let scope = test_hook::ResolverScope::install(map);
            assert!(test_hook::lookup("panic.example").is_some());
            let _ = scope;
            panic!("forced panic — Drop must still run during unwind");
        }));
        assert!(result.is_err(), "panic must propagate out of catch_unwind");
        assert!(
            test_hook::lookup("panic.example").is_none(),
            "scope Drop must restore RESOLVER even when scope body panics"
        );
    }
}
