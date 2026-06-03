//! Upstream dialing with DNS-pinning + SSRF classification.
//!
//! The allowlist gate ([`AllowList::permits`]) matches on the **unresolved**
//! hostname. Re-resolving at connect time via `TcpStream::connect((host, port))`
//! re-runs DNS and connects to whatever the resolver returns *right then* — the
//! classic DNS-rebinding TOCTOU: an allowlisted name whose DNS is attacker- (or
//! TTL-) controlled can point at a loopback / link-local / RFC1918 / cloud-
//! metadata address, and the proxy would dial it. The OS layer is collapsed to
//! "loopback only" (see [`super`] threat model), so the proxy is the *only*
//! gate — it must not be fooled into reaching internal addresses.
//!
//! This helper resolves the host **once**, drops any resolved IP that falls in
//! a blocked SSRF range (reusing [`crate::security::ssrf::ip::is_blocked_ip`],
//! which also decodes IPv4-mapped-IPv6 / NAT64 / 6to4 / Teredo embedded-IPv4
//! forms), and connects to a surviving `SocketAddr`. Because it dials the
//! resolved address — not the name — there is no second resolution to rebind
//! against.
//!
//! Escape hatch: if a resolved IP is itself an explicit allowlist entry (an
//! operator who deliberately allowlisted an internal IP literal), it is dialed
//! even when blocked — that is intentional operator configuration, not a
//! rebinding attack. A *hostname* allowlist entry that resolves to a blocked IP
//! is rejected (fail-closed), which is the desired default for SSRF defense.

use std::io;
use std::net::SocketAddr;

use tokio::net::{lookup_host, TcpStream};

use super::allowlist::AllowList;
use crate::security::ssrf::ip::is_blocked_ip;

/// Resolve `host:port`, drop SSRF-blocked addresses (unless that exact IP is an
/// explicit allowlist entry), and connect to the first reachable survivor.
///
/// Returns [`io::ErrorKind::PermissionDenied`] when every resolved address is
/// blocked (so callers can map it to a policy-denial status distinct from a
/// network failure), [`io::ErrorKind::NotFound`] when resolution is empty, and
/// the underlying connect error otherwise.
pub(super) async fn dial_validated(
    host: &str,
    port: u16,
    allowlist: &AllowList,
) -> io::Result<TcpStream> {
    let candidates: Vec<SocketAddr> = lookup_host((host, port)).await?.collect();
    if candidates.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no addresses resolved for upstream host",
        ));
    }

    let mut last_err: Option<io::Error> = None;
    let mut had_permitted_candidate = false;
    for addr in candidates {
        let ip = addr.ip();
        // Reject blocked ranges (loopback / private / link-local / cloud
        // metadata / embedded-IPv4-in-IPv6 forms) — unless the operator
        // explicitly allowlisted this exact IP literal as an intentional
        // internal target.
        if is_blocked_ip(ip) && !allowlist.permits(&ip.to_string()) {
            tracing::warn!(
                target = "sandbox.proxy",
                %host,
                resolved = %ip,
                "upstream address blocked by SSRF classifier (possible DNS rebinding)"
            );
            continue;
        }
        had_permitted_candidate = true;
        match TcpStream::connect(addr).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = Some(e),
        }
    }

    if !had_permitted_candidate {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "all resolved upstream addresses blocked by SSRF policy",
        ));
    }
    Err(last_err.unwrap_or_else(|| io::Error::other("no upstream connection established")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_rebinding_to_loopback() {
        // Allowlist holds a hostname; a target that resolves to loopback (here
        // the IP-literal form simulates a rebound resolution) must be refused
        // because 127.0.0.1 is not itself an allowlist entry.
        let allow = AllowList::new(["api.example.com"]);
        let err = dial_validated("127.0.0.1", 80, &allow).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn rejects_rebinding_to_cloud_metadata() {
        let allow = AllowList::new(["api.example.com"]);
        let err = dial_validated("169.254.169.254", 80, &allow)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn rejects_private_range() {
        let allow = AllowList::new(["api.example.com"]);
        let err = dial_validated("10.0.0.1", 80, &allow).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn rejects_ipv4_mapped_ipv6_loopback() {
        // ::ffff:127.0.0.1 must be decoded and blocked, not treated as a
        // distinct public address.
        let allow = AllowList::new(["api.example.com"]);
        let err = dial_validated("::ffff:127.0.0.1", 80, &allow)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn allows_explicitly_allowlisted_internal_ip() {
        // An operator who deliberately allowlists an internal IP literal keeps
        // access: the SSRF block is bypassed only for that exact entry. We
        // can't assert a successful connect (nothing is listening), but the
        // error must be a connection failure, never the PermissionDenied SSRF
        // refusal.
        let allow = AllowList::new(["127.0.0.1"]);
        match dial_validated("127.0.0.1", 1, &allow).await {
            // Connected (gate bypassed) — fine, that is the point.
            Ok(_) => {}
            // Any error is acceptable EXCEPT the SSRF policy refusal.
            Err(e) => assert_ne!(e.kind(), io::ErrorKind::PermissionDenied),
        }
    }
}
