//! Workspace-layer DNS pre-resolution for `NetworkPolicy::AllowHosts`.
//!
//! Sits between the approval gate and `OsSandboxDriverTrait::profile_for` in
//! the `WorkspaceSandbox` pipeline (spec SP-4 § 2). Walks every entry of
//! `AllowHosts.hosts`, leaves IP literals untouched, and resolves hostnames
//! via the system resolver (`tokio::net::lookup_host`). The resolved IPs
//! replace the hostnames in-place so the downstream driver sees an IP-only
//! allowlist — which is what every OS-level matcher (macOS Seatbelt
//! `(remote ip ...)`, Linux iptables, Windows WFP) actually understands.
//!
//! Fail-closed: any DNS failure → `SandboxError::DnsResolutionFailed`. The
//! command is refused rather than running with an empty / partial allowlist
//! that would silently deny all outbound traffic.
//!
//! Per spec SP-4 § 1, only macOS currently has an IP-level matcher wired;
//! Linux/Windows still return `UnsupportedPolicy` from their drivers.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use tokio::net::lookup_host;
use tokio::time::timeout;

use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::SandboxError;

const DNS_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) async fn resolve_hosts_in_capabilities(
    caps: &mut SandboxCapabilities,
) -> Result<(), SandboxError> {
    let hosts = match &mut caps.network {
        NetworkPolicy::None | NetworkPolicy::AllowAll => return Ok(()),
        NetworkPolicy::AllowHosts { hosts } => hosts,
    };
    if hosts.is_empty() {
        return Ok(());
    }

    let mut resolved: BTreeSet<String> = BTreeSet::new();
    for host in hosts.iter() {
        if is_ip_literal(host) {
            // rust-doctor-disable-next-line excessive-clone
            resolved.insert(host.clone());
            continue;
        }
        let lookup_target = lookup_target_for(host);
        let lookup = lookup_host(lookup_target.as_str());
        let addrs = match timeout(DNS_TIMEOUT, lookup).await {
            Ok(Ok(addrs)) => addrs,
            Ok(Err(e)) => {
                return Err(SandboxError::DnsResolutionFailed {
                    // rust-doctor-disable-next-line excessive-clone
                    hostname: host.clone(),
                    source: e,
                });
            }
            Err(_) => {
                return Err(SandboxError::DnsResolutionFailed {
                    // rust-doctor-disable-next-line excessive-clone
                    hostname: host.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("DNS lookup exceeded {}s", DNS_TIMEOUT.as_secs()),
                    ),
                });
            }
        };
        let before = resolved.len();
        for sa in addrs {
            resolved.insert(sa.ip().to_string());
        }
        if resolved.len() == before {
            return Err(SandboxError::DnsResolutionFailed {
                // rust-doctor-disable-next-line excessive-clone
                hostname: host.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "DNS resolution returned no addresses",
                ),
            });
        }
    }

    *hosts = resolved.into_iter().collect();
    Ok(())
}

fn is_ip_literal(host: &str) -> bool {
    if IpAddr::from_str(host).is_ok() {
        return true;
    }
    if let Some(rest) = host.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return IpAddr::from_str(&rest[..end]).is_ok();
        }
        return false;
    }
    if host.matches(':').count() == 1 {
        if let Some((addr, _port)) = host.split_once(':') {
            return IpAddr::from_str(addr).is_ok();
        }
    }
    false
}

/// `tokio::net::lookup_host` requires a `host:port` string. We don't care
/// about the port at this layer — Seatbelt's `(remote ip ...)` matcher
/// filters by IP only — so we synthesize port 0 when the caller didn't
/// supply one. When the caller did supply one (e.g. `"github.com:443"`),
/// preserve it so `lookup_host` accepts it as-is.
fn lookup_target_for(host: &str) -> String {
    if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(hosts: &[&str]) -> SandboxCapabilities {
        SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: hosts.iter().map(|s| s.to_string()).collect(),
            },
            ..SandboxCapabilities::strict()
        }
    }

    fn extract_hosts(caps: &SandboxCapabilities) -> Vec<String> {
        match &caps.network {
            NetworkPolicy::AllowHosts { hosts } => hosts.clone(),
            other => panic!("expected AllowHosts, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ip_literal_v4_skips_dns() {
        let mut caps = allow(&["1.2.3.4"]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(extract_hosts(&caps), vec!["1.2.3.4".to_string()]);
    }

    #[tokio::test]
    async fn ip_literal_v6_bracketed_skips_dns() {
        let mut caps = allow(&["[::1]"]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(extract_hosts(&caps), vec!["[::1]".to_string()]);
    }

    #[tokio::test]
    async fn ip_literal_v6_bare_skips_dns() {
        let mut caps = allow(&["2606:2800:220:1:248:1893:25c8:1946"]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(
            extract_hosts(&caps),
            vec!["2606:2800:220:1:248:1893:25c8:1946".to_string()]
        );
    }

    #[tokio::test]
    async fn ip_literal_v4_with_port_skips_dns() {
        let mut caps = allow(&["1.2.3.4:443"]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(extract_hosts(&caps), vec!["1.2.3.4:443".to_string()]);
    }

    #[tokio::test]
    async fn hostname_localhost_resolves() {
        let mut caps = allow(&["localhost"]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        let resolved = extract_hosts(&caps);
        assert!(
            resolved.iter().any(|h| h == "127.0.0.1" || h == "::1"),
            "expected localhost to resolve to 127.0.0.1 or ::1, got {resolved:?}"
        );
        // Hostname must be gone — the whole point of the SP-4 pass.
        assert!(!resolved.iter().any(|h| h == "localhost"));
    }

    #[tokio::test]
    async fn hostname_invalid_returns_dns_resolution_failed() {
        let mut caps = allow(&["nonexistent-host-for-aleph-sp4-test-12345.invalid"]);
        let err = resolve_hosts_in_capabilities(&mut caps).await.unwrap_err();
        assert!(
            matches!(err, SandboxError::DnsResolutionFailed { ref hostname, .. }
                if hostname == "nonexistent-host-for-aleph-sp4-test-12345.invalid"),
            "expected DnsResolutionFailed for .invalid hostname, got {err:?}"
        );
    }

    #[tokio::test]
    async fn empty_hosts_is_noop() {
        let mut caps = allow(&[]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(extract_hosts(&caps), Vec::<String>::new());
    }

    #[tokio::test]
    async fn network_none_is_noop() {
        let mut caps = SandboxCapabilities::strict();
        assert_eq!(caps.network, NetworkPolicy::None);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(caps.network, NetworkPolicy::None);
    }

    #[tokio::test]
    async fn network_allow_all_is_noop() {
        let mut caps = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        };
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        assert_eq!(caps.network, NetworkPolicy::AllowAll);
    }

    #[tokio::test]
    async fn mixed_ip_and_hostname() {
        let mut caps = allow(&["1.2.3.4", "localhost"]);
        resolve_hosts_in_capabilities(&mut caps).await.unwrap();
        let resolved = extract_hosts(&caps);
        assert!(resolved.contains(&"1.2.3.4".to_string()));
        assert!(
            resolved.iter().any(|h| h == "127.0.0.1" || h == "::1"),
            "expected localhost to expand to 127.0.0.1 or ::1, got {resolved:?}"
        );
        // No hostname should survive.
        assert!(!resolved.iter().any(|h| h == "localhost"));
    }
}
