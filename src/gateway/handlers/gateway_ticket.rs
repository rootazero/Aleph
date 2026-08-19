//! Gateway bootstrap ticket handler.
//!
//! `gateway.ticket.create` generates a short-lived, single-use bootstrap ticket
//! that a remote Panel can exchange for a per-device token during the
//! WebSocket `connect` handshake. This keeps the long-lived shared Gateway token
//! out of URLs and QR codes.
//!
//! Authorization: the caller must already be authorized (operator role) or the
//! connection must be loopback. The WS login wall enforces this because the
//! method is unreachable to unauthorized callers.
//!
//! The response also carries ready-to-use pairing **URLs**. They must be built
//! server-side: the Panel used to assemble the link from its own
//! `window.location`, which produces `http://127.0.0.1:<port>/?bt=…` whenever the
//! operator generates the QR from the local desktop App — i.e. a QR that cannot
//! work in the single most common case (pair my phone with the core running on
//! this machine). Only the server knows which addresses it is actually reachable
//! on. Mirrors openclaw's `resolveAdvertisedLanHost` /
//! `pickMatchingExternalInterfaceAddress`.

use std::net::IpAddr;

use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::DeviceTokenManager;
use crate::sync_primitives::Arc;

/// Upper bound on advertised pairing URLs. A multi-homed host (docker bridges,
/// VPN adapters, VM NATs) can expose a dozen addresses; past a handful the list
/// stops being a choice and becomes noise. Matches openclaw's cap.
const MAX_PAIRING_URLS: usize = 8;

/// Context required by the ticket handler.
#[derive(Clone)]
pub struct TicketHandlerContext {
    pub device_token_mgr: Arc<DeviceTokenManager>,
    /// `[gateway] host` — the bind address, which decides whether *any* LAN
    /// address is reachable at all.
    pub bind_host: String,
    /// `[gateway] port`.
    pub port: u16,
    /// `[gateway.tls] enabled` — picks the URL scheme. A trusted-proxy TLS
    /// deployment terminates elsewhere and is not auto-discoverable, so those
    /// operators keep using the Panel's own origin (see `pairing_urls`).
    pub tls_enabled: bool,
}

/// Hosts this gateway is reachable on from another machine, given its bind
/// address and the discovered interface IPs.
///
/// Fail-honest rather than fail-helpful: a loopback-only bind advertises
/// **nothing**, because `[gateway] host` still defaults to `127.0.0.1` and
/// printing a LAN URL there would hand the operator a link that can never
/// connect. A wildcard bind fans out to every usable interface; an explicit
/// address advertises exactly itself.
#[must_use]
pub fn reachable_hosts(bind_host: &str, discovered: &[IpAddr]) -> Vec<String> {
    let bind = bind_host.trim();
    match bind.parse::<IpAddr>() {
        // Wildcard: reachable on every interface the box has. IPv4 first — it is
        // what a phone camera and a human retyping a URL both cope with;
        // discovery order is preserved within each family.
        Ok(ip) if ip.is_unspecified() => {
            let usable = discovered.iter().copied().filter(|ip| !ip.is_loopback());
            let (v4, v6): (Vec<IpAddr>, Vec<IpAddr>) = usable.partition(IpAddr::is_ipv4);
            v4.into_iter()
                .chain(dedupe_v6_by_prefix(v6))
                .map(format_host)
                .collect()
        }
        // Loopback-only bind: the LAN cannot reach this core at all.
        Ok(ip) if ip.is_loopback() => Vec::new(),
        // Pinned to one address.
        Ok(ip) => vec![format_host(ip)],
        // Not an IP: a hostname (or "localhost"). Take it at face value except
        // for the loopback name, which is the same lie as 127.0.0.1.
        Err(_) if bind.is_empty() || bind.eq_ignore_ascii_case("localhost") => Vec::new(),
        Err(_) => vec![bind.to_string()],
    }
}

/// Keep one address per IPv6 /64. A host with SLAAC privacy extensions carries
/// half a dozen addresses on the *same* subnet (one stable + several rotating
/// temporaries); listing them all pushes the useful IPv4 entries out of the
/// capped list and offers the operator six identical-looking choices. First
/// address per prefix wins — `if_addrs` reports the stable one first.
fn dedupe_v6_by_prefix(addrs: Vec<IpAddr>) -> Vec<IpAddr> {
    let mut seen = std::collections::HashSet::new();
    addrs
        .into_iter()
        .filter(|ip| match ip {
            IpAddr::V6(v6) => {
                let prefix = v6.segments();
                seen.insert([prefix[0], prefix[1], prefix[2], prefix[3]])
            }
            IpAddr::V4(_) => true,
        })
        .collect()
}

/// Bracket IPv6 literals so they can sit in a URL authority.
fn format_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    }
}

/// Assemble full pairing URLs (`<scheme>://<host>:<port>/?bt=<ticket>`) —
/// exactly what a QR encodes and what a human can retype.
///
/// Empty when the gateway is loopback-bound; the Panel then falls back to its
/// own origin, which is correct precisely in the case it is correct (the
/// operator is already browsing over the LAN address).
#[must_use]
pub fn pairing_urls(hosts: &[String], port: u16, tls_enabled: bool, ticket: &str) -> Vec<String> {
    let scheme = if tls_enabled { "https" } else { "http" };
    hosts
        .iter()
        .take(MAX_PAIRING_URLS)
        .map(|host| format!("{scheme}://{host}:{port}/?bt={ticket}"))
        .collect()
}

/// `gateway.ticket.create` — generate a short-lived bootstrap ticket.
///
/// Request params (all optional):
/// - `ttl_seconds`: ticket lifetime in seconds (default 300, min 60)
/// - `user_id`: bind the ticket to a user — the device that exchanges it
///   inherits the binding. Validated here (exists AND active), mirroring
///   `aleph-server pair --user`, because the two id-binding producers must ask
///   the same question. The paragraph that used to stand here argued
///   validation was unnecessary since `connect` walls a dangling binding off
///   anyway; that is the argument `pair.rs` explicitly rejects — an invitation
///   that mints, prints a URL, and silently cannot work is worse than a
///   refusal at the point of the typo, where the operator is still looking.
///
///   Omit for an UNBOUND ticket. Note what unbound means: the redeeming
///   device defaults to the owner, i.e. whoever scans it becomes the operator.
///   That is the right default for pairing your own phone and the wrong one
///   for inviting a colleague, which is why `pair --user` prints a warning and
///   why the Panel now offers a principal picker instead of only sending
///   `{}`.
///
/// Response:
/// - `ticket`: the bootstrap ticket string (`aleph-bt-<uuid>`)
/// - `expires_at`: Unix timestamp in milliseconds
/// - `urls`: ready-to-open pairing URLs, LAN-reachable ones first. Empty when
///   the gateway is loopback-bound (nothing to advertise) — the Panel then falls
///   back to its own origin.
pub async fn handle_ticket_create(
    request: JsonRpcRequest,
    ctx: Arc<TicketHandlerContext>,
) -> JsonRpcResponse {
    let ttl_seconds: Option<u64> = request
        .params
        .as_ref()
        .and_then(|p| p.get("ttl_seconds").and_then(serde_json::Value::as_u64));
    let user_id: Option<String> = request
        .params
        .as_ref()
        .and_then(|p| p.get("user_id").and_then(serde_json::Value::as_str))
        .map(String::from);

    // Same question `pair --user` asks, against the same table `connect`
    // resolves against — reached through the manager's own store rather than a
    // second `SecurityStore` handle on this context, so the two cannot drift.
    //
    // Active-only for the same reason `channel.pairing.approve` is: a
    // deactivated principal is walled everywhere else, and a ticket bound to
    // them mints, prints a URL, pairs the phone, and then refuses every frame
    // with nothing on either end explaining why.
    if let Some(ref uid) = user_id {
        match ctx.device_token_mgr.store().get_user(uid) {
            Ok(Some(u)) if u.status == crate::gateway::security::store::UserStatus::Active => {}
            Ok(Some(_)) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "user {uid} is deactivated — a ticket bound to a walled principal \
                         pairs successfully and then refuses every frame"
                    ),
                );
            }
            Ok(None) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!(
                        "no such user: {uid} — a ticket bound to a dangling id gives the \
                         device an identity nobody can grant, revoke or list"
                    ),
                );
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to resolve user {uid}: {e}"),
                );
            }
        }
    }

    // Clamp caller-supplied ttl to a sane bounded range before the *1000 (raw
    // `s as i64 * 1000` overflows i64 for huge values) — and pass the SAME clamped
    // value used for expires_at into the manager so the reported expiry matches
    // what is stored (the manager applies a 60s floor of its own).
    let ttl_ms = ttl_seconds.map(|s| s.clamp(60, 86_400) as i64 * 1000);

    // Opportunistic hygiene — clear expired tickets / tokens on this chokepoint
    // so there is no dedicated prune daemon. Best-effort, never fails the call.
    if let Err(e) = ctx.device_token_mgr.prune_now() {
        tracing::debug!("ticket.create: prune failed: {e}");
    }

    match ctx
        .device_token_mgr
        .create_bootstrap_ticket(ttl_ms, user_id.as_deref())
    {
        Ok(ticket) => {
            // Authority-change audit (round-5 ⑦): a bootstrap ticket IS an
            // unpaired device credential — whoever redeems an UNBOUND one
            // becomes the owner, which is exactly why the binding (or its
            // absence) is what gets recorded. The ticket string itself is
            // credential material and never enters the log.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    match user_id.as_deref() {
                        Some(uid) => format!("gateway.ticket.create: bound to {uid}"),
                        None => {
                            "gateway.ticket.create: UNBOUND (redeemer becomes owner)".to_string()
                        }
                    },
                ));
            }
            // Expiration is 5 minutes from now by default; compute client-facing value.
            let ttl_ms = ttl_ms.unwrap_or(5 * 60 * 1000);
            let expires_at = current_timestamp_ms() + ttl_ms;
            // Resolved per call, not cached at boot: a laptop that moved between
            // networks must not hand out yesterday's address.
            let hosts = reachable_hosts(
                &ctx.bind_host,
                &crate::gateway::tls::discover_interface_ips(),
            );
            JsonRpcResponse::success(
                request.id,
                json!({
                    "ticket": ticket,
                    "expires_at": expires_at,
                    "urls": pairing_urls(&hosts, ctx.port, ctx.tls_enabled, &ticket),
                }),
            )
        }
        // Minting a ticket is a single INSERT: `Storage` is the only reachable
        // failure. Enumerating the other variants here only invited them to rot
        // into wrong copy ("invalid device token configuration" never described
        // anything this call could produce).
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ticket creation failed: {e}"),
        ),
    }
}

fn current_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn wildcard_bind_advertises_every_interface_ipv4_first() {
        let hosts = reachable_hosts("0.0.0.0", &ips(&["fd00::1", "192.168.1.20", "10.8.0.3"]));
        assert_eq!(hosts, vec!["192.168.1.20", "10.8.0.3", "[fd00::1]"]);
    }

    #[test]
    fn slaac_privacy_addresses_collapse_to_one_per_subnet() {
        // A macOS/Linux box with privacy extensions reports the stable address
        // plus several rotating temporaries on the same /64. Listing all of them
        // pushed the actually-useful IPv4 entries out of the capped list.
        let hosts = reachable_hosts(
            "::",
            &ips(&[
                "192.168.1.20",
                "2409:8a00:60a4:3a80::8d4",
                "2409:8a00:60a4:3a80:1c54:7a0d:6cef:2ee",
                "2409:8a00:60a4:3a80:c163:307a:56f0:6917",
                "fd12:3456:789a:1::5",
            ]),
        );
        assert_eq!(
            hosts,
            vec![
                "192.168.1.20",
                "[2409:8a00:60a4:3a80::8d4]",
                "[fd12:3456:789a:1::5]",
            ]
        );
    }

    #[test]
    fn loopback_bind_advertises_nothing() {
        // The default bind. Printing a LAN URL here would hand the operator a
        // link that can never connect — worse than printing none.
        for bind in ["127.0.0.1", "::1", "localhost", ""] {
            assert!(
                reachable_hosts(bind, &ips(&["192.168.1.20"])).is_empty(),
                "bind {bind:?} must advertise nothing"
            );
        }
    }

    #[test]
    fn explicit_bind_advertises_only_itself() {
        assert_eq!(
            reachable_hosts("192.168.1.20", &ips(&["192.168.1.20", "10.8.0.3"])),
            vec!["192.168.1.20"]
        );
        assert_eq!(reachable_hosts("core.lan", &[]), vec!["core.lan"]);
    }

    #[test]
    fn loopback_interface_ips_are_never_advertised() {
        assert!(reachable_hosts("0.0.0.0", &ips(&["127.0.0.1", "::1"])).is_empty());
    }

    #[test]
    fn urls_carry_the_ticket_and_follow_the_tls_scheme() {
        let hosts = vec!["192.168.1.20".to_string(), "[fd00::1]".to_string()];
        assert_eq!(
            pairing_urls(&hosts, 3033, false, "aleph-bt-x"),
            vec![
                "http://192.168.1.20:3033/?bt=aleph-bt-x",
                "http://[fd00::1]:3033/?bt=aleph-bt-x",
            ]
        );
        assert_eq!(
            pairing_urls(&hosts[..1], 3033, true, "aleph-bt-x"),
            vec!["https://192.168.1.20:3033/?bt=aleph-bt-x"]
        );
    }

    #[test]
    fn url_list_is_capped() {
        let hosts: Vec<String> = (0..30).map(|i| format!("10.0.0.{i}")).collect();
        assert_eq!(
            pairing_urls(&hosts, 3033, false, "t").len(),
            MAX_PAIRING_URLS
        );
    }

    #[tokio::test]
    async fn ticket_create_returns_a_ticket_and_url_list() {
        let ctx = Arc::new(TicketHandlerContext {
            device_token_mgr: Arc::new(DeviceTokenManager::new(Arc::new(
                crate::gateway::security::SecurityStore::in_memory().unwrap(),
            ))),
            // Loopback bind ⇒ no advertised URLs, but the ticket still mints.
            bind_host: "127.0.0.1".to_string(),
            port: 3033,
            tls_enabled: false,
        });
        let req = JsonRpcRequest::with_id("gateway.ticket.create", None, json!(1));
        let resp = handle_ticket_create(req, ctx).await;
        assert!(resp.is_success(), "{resp:?}");
        let r = resp.result.unwrap();
        assert!(r["ticket"].as_str().unwrap().starts_with("aleph-bt-"));
        assert!(r["expires_at"].as_i64().unwrap() > current_timestamp_ms());
        assert_eq!(r["urls"].as_array().unwrap().len(), 0);
    }
}
