//! Trusted reverse-proxy real-client-IP resolution.
//!
//! Aleph's abuse protections — the per-IP concurrent-connection cap, the
//! per-identity rate limiter, and the auth-failure lockout — key off the
//! client's IP address. When the daemon runs behind a reverse proxy (nginx,
//! Caddy, a cloud load balancer, or even a local `tailscale serve`), the
//! socket peer address is the *proxy*, not the client: every real client
//! collapses to one address, so a single abuser is indistinguishable from the
//! whole fleet and the caps protect nothing.
//!
//! The fix is the standard one: trust `X-Forwarded-For` **only** when the
//! socket peer is a configured trusted proxy. `X-Forwarded-For` is
//! client-spoofable, so an allowlist is the entire security boundary — an
//! empty allowlist (the default) means the header is never trusted and the
//! socket peer address is used verbatim, exactly as before this module
//! existed.
//!
//! CIDR matching uses the `ipnet` crate, already present in the dependency
//! tree (transitive), so this adds no meaningful build weight (redline R3).

use std::net::IpAddr;
use std::str::FromStr;

use axum::http::HeaderMap;
use ipnet::IpNet;

/// Parsed allowlist of trusted reverse-proxy addresses (exact IPs and CIDR
/// ranges). Construct once from config and share via `Arc`.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
}

impl TrustedProxies {
    /// Parse from config strings. Each entry may be a bare IP (`"10.0.0.5"`,
    /// `"::1"`) or a CIDR (`"10.0.0.0/24"`, `"fd00::/8"`). Unparseable entries
    /// are skipped with a warning so a single typo never silently disables the
    /// whole allowlist.
    pub fn from_config(entries: &[String]) -> Self {
        let mut nets = Vec::new();
        for raw in entries {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(net) = IpNet::from_str(raw) {
                nets.push(net);
            } else if let Ok(ip) = IpAddr::from_str(raw) {
                let prefix = if ip.is_ipv4() { 32 } else { 128 };
                if let Ok(net) = IpNet::new(ip, prefix) {
                    nets.push(net);
                }
            } else {
                tracing::warn!(entry = %raw, "ignoring unparseable trusted_proxies entry");
            }
        }
        Self { nets }
    }

    /// Whether the allowlist is empty (no proxies trusted).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    /// Whether `ip` is a configured trusted proxy.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        self.nets.iter().any(|n| n.contains(&ip))
    }

    /// Resolve the real client IP for a connection whose socket peer is `peer`.
    ///
    /// Returns `peer` unchanged when the allowlist is empty or `peer` is not a
    /// trusted proxy — `X-Forwarded-For` is never trusted from an unverified
    /// source. Otherwise walks the `X-Forwarded-For` chain from right (the hop
    /// closest to us) to left, skipping addresses that are themselves trusted
    /// proxies, and returns the first non-proxy address — the real client as
    /// seen just past our own proxy layer. Falls back to `peer` when the
    /// header is absent, malformed, or lists only proxy hops.
    #[must_use]
    pub fn real_client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        if self.nets.is_empty() || !self.contains(peer) {
            return peer;
        }
        let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
            return peer;
        };
        for hop in xff.rsplit(',') {
            let hop = hop.trim();
            // Tolerate empty segments from a trailing comma / stray whitespace.
            if hop.is_empty() {
                continue;
            }
            match IpAddr::from_str(hop) {
                // Our own proxy layer — keep walking left past it.
                Ok(ip) if self.contains(ip) => continue,
                // First hop past the trusted chain: the real client.
                Ok(ip) => return ip,
                // A non-empty, unparseable hop means the chain is corrupt past
                // this point. Do NOT skip it and trust whatever an attacker
                // injected further left — stop and fall back to the peer.
                Err(_) => return peer,
            }
        }
        peer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xff(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", value.parse().unwrap());
        h
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn empty_allowlist_never_trusts_xff() {
        let tp = TrustedProxies::from_config(&[]);
        assert!(tp.is_empty());
        let peer = ip("203.0.113.7");
        assert_eq!(tp.real_client_ip(peer, &xff("10.1.2.3")), peer);
    }

    #[test]
    fn untrusted_peer_keeps_socket_ip() {
        let tp = TrustedProxies::from_config(&["10.0.0.0/8".into()]);
        let peer = ip("203.0.113.7"); // not in 10.0.0.0/8
        assert_eq!(tp.real_client_ip(peer, &xff("8.8.8.8")), peer);
    }

    #[test]
    fn trusted_proxy_resolves_real_client() {
        let tp = TrustedProxies::from_config(&["10.0.0.1".into()]);
        let peer = ip("10.0.0.1");
        // Single client hop behind the proxy.
        assert_eq!(
            tp.real_client_ip(peer, &xff("198.51.100.23")),
            ip("198.51.100.23")
        );
    }

    #[test]
    fn skips_chained_trusted_proxies_right_to_left() {
        // Two trusted proxies in front; the real client is leftmost.
        let tp = TrustedProxies::from_config(&["10.0.0.0/24".into()]);
        let peer = ip("10.0.0.1");
        let h = xff("198.51.100.23, 10.0.0.9, 10.0.0.1");
        assert_eq!(tp.real_client_ip(peer, &h), ip("198.51.100.23"));
    }

    #[test]
    fn falls_back_to_peer_when_only_proxy_hops() {
        let tp = TrustedProxies::from_config(&["10.0.0.0/24".into()]);
        let peer = ip("10.0.0.1");
        // XFF lists only trusted proxies → no real client found.
        assert_eq!(tp.real_client_ip(peer, &xff("10.0.0.9, 10.0.0.1")), peer);
    }

    #[test]
    fn cidr_and_bare_ip_both_parse() {
        let tp = TrustedProxies::from_config(&[
            "192.168.1.0/24".into(),
            "10.0.0.5".into(),
            "not-an-ip".into(), // skipped
        ]);
        assert!(tp.contains(ip("192.168.1.200")));
        assert!(tp.contains(ip("10.0.0.5")));
        assert!(!tp.contains(ip("10.0.0.6")));
    }
}
