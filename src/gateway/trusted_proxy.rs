//! Trusted reverse-proxy client resolution.
//!
//! Behind a reverse proxy the transport peer is the proxy, so IP-keyed
//! protections (per-IP cap, rate-limit, audit) and the connect-auth loopback
//! test would all collapse onto the proxy address. When the immediate peer is a
//! configured trusted proxy, this restores the real client from
//! `X-Forwarded-For` and the client-leg TLS status from `X-Forwarded-Proto`.
//! An untrusted peer's forwarding headers are ignored entirely, so they can
//! never be spoofed. v1 trusts a single proxy hop (browser → proxy → aleph).

use std::net::IpAddr;

use axum::http::HeaderMap;

/// The effective client identity after honoring a trusted proxy's headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedClient {
    /// Effective client IP: the forwarded client behind a trusted proxy, else
    /// the raw transport peer.
    pub ip: IpAddr,
    /// Whether the client-facing leg was TLS, per a trusted proxy's
    /// `X-Forwarded-Proto: https`. Native in-process TLS is folded in by the
    /// caller (via `tls_enabled`), not here.
    pub secure: bool,
}

/// Resolve the effective client for an inbound WS upgrade. See module docs.
#[must_use]
pub fn resolve_client(
    peer: IpAddr,
    headers: &HeaderMap,
    enabled: bool,
    trusted_ips: &[IpAddr],
) -> ResolvedClient {
    if !enabled || !trusted_ips.contains(&peer) {
        return ResolvedClient {
            ip: peer,
            secure: false,
        };
    }
    let ip = last_forwarded_for(headers).unwrap_or(peer);
    let secure = forwarded_proto_https(headers);
    ResolvedClient { ip, secure }
}

/// The last (rightmost) valid IP in `X-Forwarded-For` — the address the trusted
/// proxy itself appended. `None` on absent/garbage input (caller falls back).
fn last_forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    let raw = headers.get("x-forwarded-for")?.to_str().ok()?;
    raw.rsplit(',')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .and_then(|s| s.parse::<IpAddr>().ok())
}

fn forwarded_proto_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("https"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn hdrs(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn trusted_peer_uses_last_xff_and_proto() {
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[
                ("x-forwarded-for", "203.0.113.7"),
                ("x-forwarded-proto", "https"),
            ]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("203.0.113.7"));
        assert!(r.secure);
    }

    #[test]
    fn untrusted_peer_ignores_xff_no_spoof() {
        // Peer is NOT in trusted_ips → XFF is ignored, raw peer wins, not secure.
        let r = resolve_client(
            ip("198.51.100.9"),
            &hdrs(&[
                ("x-forwarded-for", "127.0.0.1"),
                ("x-forwarded-proto", "https"),
            ]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("198.51.100.9"));
        assert!(!r.secure);
    }

    #[test]
    fn disabled_always_raw_peer() {
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[
                ("x-forwarded-for", "203.0.113.7"),
                ("x-forwarded-proto", "https"),
            ]),
            false,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("127.0.0.1"));
        assert!(!r.secure);
    }

    #[test]
    fn malformed_xff_falls_back_to_peer() {
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "not-an-ip")]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("127.0.0.1"));
        assert!(!r.secure); // no proto header
    }

    #[test]
    fn last_entry_of_multi_hop_xff() {
        // v1 single-hop: the trusted proxy appended the rightmost entry.
        let r = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "10.0.0.5, 203.0.113.7")]),
            true,
            &[ip("127.0.0.1")],
        );
        assert_eq!(r.ip, ip("203.0.113.7"));
    }
}
