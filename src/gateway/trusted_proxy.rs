//! Reverse-proxy trust: resolve the *effective* client IP from the socket peer
//! and `X-Forwarded-For`, honoring an operator-configured allowlist of trusted
//! reverse-proxy addresses.
//!
//! # Why this exists (the security fix)
//!
//! The gateway grants zero-config **operator** trust to loopback connections
//! (the local desktop Panel). Every IP-keyed decision — the login-wall
//! short-circuit ([`crate::gateway::handlers::connect::resolve_connect_auth`]),
//! rate-limit / per-IP-cap exemptions, Desktop lane priority — keys off
//! `client_ip.is_loopback()`.
//!
//! Behind a reverse proxy **running on the same host** (the exact deployment we
//! want to make simple — "put Caddy/nginx in front for TLS"), every proxied
//! connection's socket peer is a loopback address. Without this module that
//! silently promotes *every remote client* to token-free operator — a privilege
//! escalation. openclaw and hermes-agent both solve it the same way: a
//! connection that arrived via a proxy can never inherit loopback trust; its
//! real origin is read from `X-Forwarded-For`, and the resolution is
//! **fail-closed** (a missing/unparseable header never falls back to a loopback
//! address).
//!
//! # Three overlapping defenses (all fail toward the login wall)
//!
//! 1. **Declared-proxy allowlist** — `[gateway] trusted_proxies`. A connection
//!    whose socket peer matches is proxied; its real client is read from
//!    `X-Forwarded-For`.
//! 2. **Both-family loopback expansion** — listing either `127.0.0.1` or `::1`
//!    covers both, because a same-host proxy may dial the gateway over whichever
//!    loopback family the OS resolver returns (`localhost` is frequently `::1`
//!    first). Without this, `trusted_proxies = ["127.0.0.1"]` + a proxy on `::1`
//!    would fail *open*.
//! 3. **Loopback-carrying-`X-Forwarded-For`** — a loopback socket peer that
//!    presents an `X-Forwarded-For` header is behind a proxy regardless of the
//!    allowlist (legitimate local clients — Panel, CLI, shell — never send it).
//!    This closes the "forgot to configure `trusted_proxies`" footgun.
//!
//! All addresses (socket peer, allowlist entries, XFF hops) are canonicalized
//! with [`std::net::IpAddr::to_canonical`] so an IPv4-mapped IPv6 address
//! (`::ffff:127.0.0.1`) matches its plain-IPv4 form.
//!
//! `X-Forwarded-For` is used only to *key* abuse protections to the real client
//! (rate limit / per-IP cap) — never to *grant* trust. The security invariant
//! ("a proxied connection is never loopback-trusted") holds regardless of what
//! the header claims.

use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Operator-configured set of trusted reverse-proxy addresses.
///
/// Built from `[gateway] trusted_proxies` — a list of plain IPs (`10.0.0.2`,
/// treated as a host route) or CIDR blocks (`10.0.0.0/8`). Empty ⇒ no proxy is
/// trusted. Any listed loopback host auto-expands to cover both IP families.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    nets: Vec<IpNet>,
}

impl TrustedProxies {
    /// Parse config entries into a matcher.
    ///
    /// Blank entries are skipped. An unparseable entry is dropped with a
    /// warning rather than defaulted — a malformed allowlist line must never
    /// silently *widen* trust. If any entry is a loopback host, both loopback
    /// families (`127.0.0.1` and `::1`) are added (defense #2 in the module docs).
    #[must_use]
    pub fn from_config(entries: &[String]) -> Self {
        let mut nets: Vec<IpNet> = entries
            .iter()
            .filter_map(|raw| {
                let s = raw.trim();
                if s.is_empty() {
                    return None;
                }
                match parse_net(s) {
                    Some(net) => Some(net),
                    None => {
                        tracing::warn!(
                            entry = %s,
                            "ignoring invalid [gateway] trusted_proxies entry (expected IP or CIDR)"
                        );
                        None
                    }
                }
            })
            .collect();

        // Both-family loopback expansion: if the operator trusts either loopback
        // family (as a host route), trust both — a same-host proxy may dial over
        // whichever the resolver picks.
        if nets.iter().any(is_loopback_host) {
            for lo in [
                IpNet::from(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                IpNet::from(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            ] {
                if !nets.contains(&lo) {
                    nets.push(lo);
                }
            }
        }

        Self { nets }
    }

    /// Whether no proxy is trusted (the default).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nets.is_empty()
    }

    /// Whether `ip` (canonicalized) is one of the configured trusted proxies.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.nets.iter().any(|net| net.contains(&ip))
    }
}

/// A `/32` or `/128` host route whose address is loopback.
fn is_loopback_host(net: &IpNet) -> bool {
    net.addr().is_loopback() && net.prefix_len() == net.max_prefix_len()
}

/// Parse a single allowlist entry: `10.0.0.0/8` (CIDR) or `10.0.0.2` (host).
fn parse_net(s: &str) -> Option<IpNet> {
    if s.contains('/') {
        s.parse::<IpNet>().ok()
    } else {
        s.parse::<IpAddr>()
            .ok()
            .map(|ip| IpNet::from(ip.to_canonical()))
    }
}

/// Resolve the effective client IP for trust + abuse-keying decisions.
///
/// See the module docs for the full contract. In short: not-proxied ⇒ socket
/// peer (canonicalized) verbatim; proxied ⇒ the real client from
/// `X-Forwarded-For`, fail-closed to a non-loopback unspecified address so
/// loopback trust is never inherited.
#[must_use]
pub fn resolve_effective_client_ip(
    socket_peer: IpAddr,
    xff: Option<&str>,
    trusted: &TrustedProxies,
) -> IpAddr {
    let peer = socket_peer.to_canonical();
    let has_xff = xff.is_some_and(|h| !h.trim().is_empty());

    // A connection is "proxied" if it came from a declared proxy, OR it is a
    // loopback peer that carries forwarded headers (a proxy is in front even if
    // it was not declared — legitimate local clients never send X-Forwarded-For).
    let proxied = trusted.contains(peer) || (peer.is_loopback() && has_xff);
    if !proxied {
        return peer;
    }

    // Proxied: never inherit loopback (operator) trust.
    match real_client_from_xff(xff, trusted) {
        Some(ip) if !ip.is_loopback() => ip,
        _ => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
    }
}

/// Walk `X-Forwarded-For` right-to-left, skipping hops that are themselves
/// trusted proxies, and return the first remaining (untrusted) hop — the real
/// client as seen at the outermost proxy. `None` when the header is absent or
/// contains no parseable untrusted hop.
fn real_client_from_xff(xff: Option<&str>, trusted: &TrustedProxies) -> Option<IpAddr> {
    xff?.split(',')
        .rev()
        .filter_map(|part| part.trim().parse::<IpAddr>().ok())
        .map(|ip| ip.to_canonical())
        .find(|ip| !trusted.contains(*ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn empty_allowlist_returns_socket_verbatim() {
        let t = TrustedProxies::default();
        // Loopback stays loopback ⇒ direct local Panel keeps auto-operator.
        assert_eq!(
            resolve_effective_client_ip(ip("127.0.0.1"), None, &t),
            ip("127.0.0.1")
        );
        assert_eq!(
            resolve_effective_client_ip(ip("192.168.1.9"), None, &t),
            ip("192.168.1.9")
        );
    }

    #[test]
    fn untrusted_socket_ignores_forwarded_header() {
        // A direct NON-loopback client is judged on its own socket address; it
        // cannot inject XFF to spoof a different origin (not a trusted proxy).
        let t = TrustedProxies::from_config(&["10.0.0.0/8".to_string()]);
        assert_eq!(
            resolve_effective_client_ip(ip("203.0.113.7"), Some("8.8.8.8"), &t),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn trusted_proxy_resolves_real_remote_client() {
        let t = TrustedProxies::from_config(&["127.0.0.1".to_string()]);
        assert_eq!(
            resolve_effective_client_ip(ip("127.0.0.1"), Some("203.0.113.7"), &t),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn same_host_proxy_never_grants_loopback_trust() {
        // THE core regression: a proxy on 127.0.0.1 forwarding a loopback (or
        // forged-loopback) client must NOT resolve to a loopback IP.
        let t = TrustedProxies::from_config(&["127.0.0.1".to_string()]);
        let r = resolve_effective_client_ip(ip("127.0.0.1"), Some("127.0.0.1"), &t);
        assert!(!r.is_loopback(), "proxied loopback must not stay loopback");
        assert_eq!(r, ip("0.0.0.0"));
    }

    #[test]
    fn cross_family_loopback_proxy_is_fail_closed() {
        // REGRESSION for the review's CRITICAL: operator lists only the v4
        // loopback, but the same-host proxy dials over ::1 (localhost → ::1).
        // Both-family expansion means ::1 is treated as the trusted proxy, so a
        // missing/loopback XFF fails closed instead of granting ::1 operator.
        let t = TrustedProxies::from_config(&["127.0.0.1".to_string()]);
        assert!(t.contains(ip("::1")), "listing 127.0.0.1 must also trust ::1");
        let no_xff = resolve_effective_client_ip(ip("::1"), None, &t);
        assert!(!no_xff.is_loopback());
        let real = resolve_effective_client_ip(ip("::1"), Some("203.0.113.7"), &t);
        assert_eq!(real, ip("203.0.113.7"));
        // Symmetric: listing ::1 also trusts 127.0.0.1.
        let t2 = TrustedProxies::from_config(&["::1".to_string()]);
        assert!(t2.contains(ip("127.0.0.1")));
    }

    #[test]
    fn loopback_peer_with_xff_is_proxied_even_without_config() {
        // Defense #3: a loopback peer carrying X-Forwarded-For is behind a proxy
        // even when trusted_proxies was never configured — deny operator.
        let t = TrustedProxies::default();
        let r = resolve_effective_client_ip(ip("127.0.0.1"), Some("203.0.113.7"), &t);
        assert_eq!(r, ip("203.0.113.7"));
        assert!(!r.is_loopback());
        // But a loopback peer with NO forwarded header is a genuine local client.
        assert_eq!(
            resolve_effective_client_ip(ip("127.0.0.1"), None, &t),
            ip("127.0.0.1")
        );
    }

    #[test]
    fn ipv4_mapped_v6_peer_is_canonicalized() {
        // ::ffff:127.0.0.1 must be treated as 127.0.0.1 for both matching and
        // the loopback check.
        let t = TrustedProxies::from_config(&["127.0.0.1".to_string()]);
        assert!(t.contains(ip("::ffff:127.0.0.1")));
        let r = resolve_effective_client_ip(ip("::ffff:192.168.1.9"), None, &t);
        assert_eq!(r, ip("192.168.1.9"));
    }

    #[test]
    fn trusted_proxy_missing_xff_is_fail_closed() {
        let t = TrustedProxies::from_config(&["10.0.0.2".to_string()]);
        let r = resolve_effective_client_ip(ip("10.0.0.2"), None, &t);
        assert_eq!(r, ip("0.0.0.0"));
    }

    #[test]
    fn trusted_proxy_garbage_xff_is_fail_closed() {
        let t = TrustedProxies::from_config(&["10.0.0.2".to_string()]);
        let r = resolve_effective_client_ip(ip("10.0.0.2"), Some("not-an-ip"), &t);
        assert!(!r.is_loopback());
    }

    #[test]
    fn chained_trusted_proxies_walk_to_real_client() {
        // XFF: client, edge-proxy, inner-proxy   (socket = inner-proxy)
        let t = TrustedProxies::from_config(&["10.0.0.0/8".to_string()]);
        let r = resolve_effective_client_ip(
            ip("10.0.0.3"),
            Some("203.0.113.7, 10.0.0.2, 10.0.0.3"),
            &t,
        );
        assert_eq!(r, ip("203.0.113.7"));
    }

    #[test]
    fn cidr_and_host_entries_match() {
        let t = TrustedProxies::from_config(&["10.0.0.0/8".to_string(), "192.168.1.5".to_string()]);
        assert!(t.contains(ip("10.255.1.1")));
        assert!(t.contains(ip("192.168.1.5")));
        assert!(!t.contains(ip("192.168.1.6")));
        assert!(!t.contains(ip("8.8.8.8")));
    }

    #[test]
    fn invalid_entries_are_skipped_not_widening() {
        let t = TrustedProxies::from_config(&[
            "not-a-cidr".to_string(),
            "".to_string(),
            "10.0.0.0/8".to_string(),
        ]);
        assert!(t.contains(ip("10.1.2.3")));
        assert!(!t.contains(ip("127.0.0.1")));
        assert!(!t.contains(ip("8.8.8.8")));
    }

    #[test]
    fn ipv6_proxy_resolution() {
        let t = TrustedProxies::from_config(&["2001:db8::/32".to_string()]);
        assert!(t.contains(ip("2001:db8::5")));
        // Real client is outside the trusted /32, so the walk returns it.
        let r = resolve_effective_client_ip(ip("2001:db8::5"), Some("2001:db9::9"), &t);
        assert_eq!(r, ip("2001:db9::9"));
        // A forwarded hop *inside* the trusted range is skipped (fail-closed).
        let inside = resolve_effective_client_ip(ip("2001:db8::5"), Some("2001:db8:1::9"), &t);
        assert!(inside.is_unspecified());
    }
}
