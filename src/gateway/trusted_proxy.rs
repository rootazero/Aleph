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
    ///
    /// This is an identity for **bucketing and audit** — the per-IP connection
    /// cap, the rate-limit key, the audit row. It is deliberately NOT the
    /// answer to "does this connection hold local authority": see
    /// [`ResolvedClient::local`].
    pub ip: IpAddr,
    /// Whether the client-facing leg was TLS, per a trusted proxy's
    /// `X-Forwarded-Proto: https`. Native in-process TLS is folded in by the
    /// caller (via `tls_enabled`), not here.
    pub secure: bool,
    /// Whether this connection is genuinely local: the transport peer reached
    /// us over loopback **and did not arrive through a trusted proxy hop**.
    ///
    /// # Why this is its own bit and not `ip.is_loopback()`
    ///
    /// Every loopback privilege in the gateway (zero-config operator at
    /// `connect`, the per-IP cap exemption, the rate-limit exemption, the
    /// desktop lane pool, "never kick this socket when the token rotates")
    /// used to be derived from `ip.is_loopback()`. That only works while `ip`
    /// is *certain*, and it is not: the documented same-host reverse-proxy
    /// tier puts the proxy on 127.0.0.1, and when a proxy is configured but
    /// emits no `X-Forwarded-For` — nginx does not unless told to — `ip` falls
    /// back to the proxy's own loopback address. "I could not determine the
    /// real client" then read as "the client is local", i.e. an
    /// unauthenticated internet client resolved to a full operator.
    ///
    /// The fix is not to change `ip`: bucketing and audit keying must keep
    /// falling back to the peer (a fabricated address would be worse), and
    /// `malformed_xff_falls_back_to_peer` pins that. One *derived* bit had
    /// been overloaded onto it. A connection that arrived through a trusted
    /// proxy hop is never local, whether or not the forwarding header parsed —
    /// the same fail-closed convention `server::handler::resolve_stamped_identity`
    /// already writes down for a lookup it could not actually perform.
    pub local: bool,
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
            local: peer.is_loopback(),
        };
    }
    // Note the asymmetry with `local` below: the peer fallback here is
    // deliberate (bucketing must stay stable when the header is missing), but
    // the authority bit must NOT inherit that fallback.
    let ip = last_forwarded_for(headers).unwrap_or(peer);
    let secure = forwarded_proto_https(headers);
    ResolvedClient {
        ip,
        secure,
        local: false,
    }
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
        // `ip` still falls back to the peer — that is the bucketing identity
        // and it must stay stable. What must NOT fall back is the authority
        // bit; see `a_trusted_proxy_hop_is_never_local`.
        assert_eq!(r.ip, ip("127.0.0.1"));
        assert!(!r.secure); // no proto header
        assert!(
            !r.local,
            "a hop through a trusted proxy is never local, even when the peer \
             fallback made `ip` loopback"
        );
    }

    /// The composition nothing tested: `resolve_client` for a loopback trusted
    /// proxy with NO forwarding header at all — the shape a same-host nginx
    /// produces out of the box, since it does not add `X-Forwarded-For` unless
    /// configured to. `ip` resolving to the proxy's own loopback address made
    /// every downstream loopback privilege true for an internet client.
    #[test]
    fn a_trusted_proxy_hop_is_never_local() {
        for headers in [
            HeaderMap::new(),                            // nginx default: no XFF
            hdrs(&[("x-forwarded-for", "not-an-ip")]),   // garbage XFF
            hdrs(&[("x-forwarded-for", "203.0.113.7")]), // well-formed XFF
        ] {
            let r = resolve_client(ip("127.0.0.1"), &headers, true, &[ip("127.0.0.1")]);
            assert!(
                !r.local,
                "a connection that arrived through a trusted proxy hop must never \
                 be treated as local, whatever X-Forwarded-For did or did not say"
            );
        }
    }

    /// The other half: a real loopback client (no proxy configured, or a peer
    /// that is not a trusted proxy) must still be local, or the fix would have
    /// turned the zero-config desktop Panel into a walled guest.
    #[test]
    fn a_direct_loopback_peer_is_still_local() {
        let direct = resolve_client(ip("127.0.0.1"), &HeaderMap::new(), false, &[]);
        assert!(direct.local, "proxy disabled: a loopback peer is local");

        // Enabled, but this peer is not one of the trusted proxies: its
        // forwarding headers are ignored and it is judged on its own address.
        let untrusted_but_loopback = resolve_client(
            ip("127.0.0.1"),
            &hdrs(&[("x-forwarded-for", "203.0.113.7")]),
            true,
            &[ip("10.0.0.1")],
        );
        assert!(untrusted_but_loopback.local);

        let remote = resolve_client(ip("198.51.100.9"), &HeaderMap::new(), false, &[]);
        assert!(!remote.local, "a remote peer is never local");
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
