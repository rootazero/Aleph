//! Paired-device management handlers.
//!
//! `gateway.devices.list` returns the remote Panel devices that have paired via
//! the bootstrap-ticket flow; `gateway.devices.revoke` kicks one of them by
//! invalidating its device record and tokens. Together they give an operator a
//! visible, revocable inventory of remote devices — the per-device counterpart
//! to `gateway.token.rotate`'s "revoke all remotes" hammer.
//!
//! Scope guard: both operate only on `device_type = "panel"` rows, never on
//! cluster nodes (`role = "node"`), which are managed through `cluster.rs`.
//!
//! Authorization: reachable only by an authorized (operator / loopback)
//! connection — the WS login wall refuses every non-`connect` method to an
//! unauthorized caller, so no extra per-method gate is needed.
//!
//! Revocation is immediate, not deferred to the next handshake: [`revoke_device_and_kick`]
//! writes the store revoke, then (only when that write actually revoked
//! something) downgrades the device's live sessions to the login wall and
//! publishes `DeviceRevoked` to close their sockets — demote-before-kick, see
//! `gateway/CLAUDE.md` mine 2. That function is the single source for "what
//! does revoking a device actually do": `handle_devices_revoke` calls it, and
//! so does `users.update`'s deactivation path (revokes every device owned by a
//! newly-deactivated user) — one pipeline, never a second copy.

use aleph_protocol::devices::{PairedDeviceList, PairedDeviceRow};
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::gateway::presence::PresenceTracker;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use crate::gateway::security::{DeviceTokenError, DeviceTokenManager};
use crate::gateway::server::ConnectionState;
use crate::sync_primitives::Arc;

/// Context required by the device-management handlers.
#[derive(Clone)]
pub struct DevicesHandlerContext {
    pub device_token_mgr: Arc<DeviceTokenManager>,
    /// Live-connection roster, used to mark which paired devices are online
    /// right now. Presence entries carry the `device_id` latched at the
    /// `connect` handshake, so this is a join, not a guess.
    pub presence: Arc<PresenceTracker>,
    /// Live connection map — `revoke_device_and_kick` downgrades any open
    /// session bound to the revoked device to the login wall.
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    /// Event bus — `revoke_device_and_kick` publishes `DeviceRevoked` on it to
    /// close the device's live sockets.
    pub event_bus: Arc<GatewayEventBus>,
}

/// Revoke one paired Panel device's store record and, if that write actually
/// revoked something (an unknown id, a cluster node, or an already-revoked
/// device is a no-op), kick its live sessions: downgrade any open connections
/// to the login wall, then publish `DeviceRevoked` to close their sockets.
///
/// Order matters — mirrors openclaw's `device.pair.remove`: invalidate first
/// so anything already pipelined on that socket fails the login wall, then
/// publish the close (gateway/CLAUDE.md mine 2). The RPC response itself is
/// written by the same connection loop arm that dispatched this call, so a
/// device revoking *itself* still receives its reply before the close frame
/// is polled — no extra handling needed here for the self-revocation case.
///
/// Shared by [`handle_devices_revoke`] and `users.update`'s deactivation path
/// — the single "revoke one device" pipeline, never duplicated.
pub(crate) async fn revoke_device_and_kick(
    device_token_mgr: &DeviceTokenManager,
    connections: &Arc<RwLock<HashMap<String, ConnectionState>>>,
    event_bus: &GatewayEventBus,
    device_id: &str,
) -> Result<bool, DeviceTokenError> {
    // Read the binding BEFORE the write: the audit line below wants to name
    // the principal whose credential this was, and "which person did this cut
    // off" must not depend on whether revocation leaves the column behind.
    // A store error folds to `None` — "we could not tell" — never to a claim
    // that the device was unbound.
    let bound_user = device_token_mgr
        .store()
        .device_user(device_id)
        .unwrap_or(None);
    // Hold the connection-map write guard across the store write AND the
    // downgrade: the `connect` handshake finalizes its verdict under this same
    // lock, so it either runs before (and is cleared below) or after (and sees
    // the device revoked) — never between and restores a verified identity.
    // The store calls are synchronous, so no store lock spans an await; the
    // guard is dropped before the audit `.await`.
    let mut conns = connections.write().await;
    let revoked = device_token_mgr.revoke_panel_device(device_id)?;
    let downgraded = if revoked {
        crate::gateway::server::invalidate_device_sessions_locked(&mut conns, device_id)
    } else {
        0
    };
    drop(conns);
    if revoked {
        // Authority change: minting a device credential
        // (`gateway.ticket.create`) has been audited since round-5 and its
        // inverse was not — the `AuthorityChange` doc names "device revoked"
        // in its own list of covered writes, so this was a producer the
        // variant claimed and never had.
        //
        // This lives in the shared pipeline rather than in
        // `handle_devices_revoke`, because that is not the only face of this
        // verb: `users.update`'s deactivation revokes in bulk through here
        // too, and a per-face producer is the shape that leaves the second
        // face silent. A bulk deactivation therefore writes one line per
        // credential actually cut, which is what "which credentials did that
        // deactivation reach" needs in order to be answerable at all.
        //
        // Only on the transition. `revoked == false` means unknown id,
        // already revoked, or not a Panel device — nothing changed, so there
        // is no decision to record.
        if let Some(log) = crate::security::audit::global() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                crate::gateway::caller_identity::current_caller_user(),
                format!(
                    "devices.revoke: {} (principal {})",
                    device_id,
                    bound_user.as_deref().unwrap_or("unbound")
                ),
            ))
            .await;
        }
        if downgraded > 0 {
            tracing::info!(
                device_id = %device_id,
                sessions = downgraded,
                "device revoked: live sessions downgraded to the login wall"
            );
        }
        let _ = event_bus.publish_frame(&GatewayEventFrame::DeviceRevoked {
            device_id: device_id.to_string(),
        });
    }
    Ok(revoked)
}

/// Revoke every paired Panel device through the single-device pipeline
/// ([`revoke_device_and_kick`]) so each one also clears its live sessions
/// (loopback-paired included) and publishes `DeviceRevoked`. The "revoke all
/// remotes" face of the verb — `gateway.token.rotate` — uses this instead of the
/// raw store `revoke_all_panel_devices`, which would leave live verified
/// sessions standing. Cluster nodes stay untouched (`list_panel_devices`
/// excludes them). Returns how many devices were actually revoked.
pub(crate) async fn revoke_all_panel_devices_and_kick(
    device_token_mgr: &DeviceTokenManager,
    connections: &Arc<RwLock<HashMap<String, ConnectionState>>>,
    event_bus: &GatewayEventBus,
) -> Result<usize, DeviceTokenError> {
    let mut revoked = 0usize;
    for device in device_token_mgr.list_panel_devices()? {
        if revoke_device_and_kick(device_token_mgr, connections, event_bus, &device.device_id)
            .await?
        {
            revoked += 1;
        }
    }
    Ok(revoked)
}

/// `gateway.devices.list` — list paired remote Panel devices.
///
/// Response: [`PairedDeviceList`], which this handler **constructs** — the
/// field list is not restated here, because a hand-kept copy of a wire shape
/// is the half that goes stale silently.
///
/// `connected` is a live join against the presence roster (mirrors openclaw's
/// `device.pair.list` `connected` flag) — without it "Last seen 3 minutes ago"
/// is the only signal an operator has, and revoking is now a session-killing
/// action that deserves to say so up front. It is required on the wire for the
/// reason [`PairedDeviceRow::connected`] gives: a defaulted absence would read
/// as a claim of "offline".
/// Also opportunistically prunes expired bootstrap tickets / device tokens (no
/// dedicated daemon task).
pub async fn handle_devices_list(
    request: JsonRpcRequest,
    ctx: Arc<DevicesHandlerContext>,
) -> JsonRpcResponse {
    // Opportunistic hygiene — cheap, best-effort, never fails the request.
    if let Err(e) = ctx.device_token_mgr.prune_now() {
        tracing::debug!("devices.list: prune failed: {e}");
    }

    let devices = match ctx.device_token_mgr.list_panel_devices() {
        Ok(rows) => rows,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                crate::gateway::protocol::INTERNAL_ERROR,
                format!("failed to list devices: {e}"),
            );
        }
    };

    // device_id -> oldest live connection start (epoch ms). Membership is the
    // `connected` flag; the value is `connected_since`. One join, one map: the
    // two fields cannot disagree about which devices are online.
    let mut online: HashMap<String, i64> = HashMap::new();
    for entry in ctx.presence.list() {
        let Some(device_id) = entry.device_id else {
            continue;
        };
        let at = entry.connected_at.timestamp_millis();
        online
            .entry(device_id)
            .and_modify(|since| *since = (*since).min(at))
            .or_insert(at);
    }

    // Built FROM the shared contract type, not hand-assembled next to it:
    // parsing a literal can only ever prove the server sends a superset, which
    // is how `created_at` — emitted since this list shipped, rendered by no
    // client in any language — stayed invisible until the key set was pinned.
    // A rename is now a compile error on both sides instead of a thinner list.
    let list: Vec<PairedDeviceRow> = devices
        .into_iter()
        .map(|d| PairedDeviceRow {
            connected: online.contains_key(&d.device_id),
            connected_since: online.get(&d.device_id).copied(),
            device_id: d.device_id,
            device_name: d.device_name,
            last_seen_at: d.last_seen_at,
            // Which principal this device speaks as. SECURITY.md calls this
            // list "the inventory", and until 2026-08-13 it was an inventory
            // with no owners: five members' phones showed as five rows named
            // "iPhone", so revoking the right one was guesswork and verifying
            // that a deactivation actually cut someone's devices was
            // impossible from any surface. `display_name` resolves through the
            // same directory projection the channel-pairing list and the room
            // bubbles use, so an operator reads a name rather than a `u-` id.
            display_name: d
                .user_id
                .as_deref()
                .and_then(crate::scope::directory::display_name),
            user_id: d.user_id,
        })
        .collect();

    match serde_json::to_value(PairedDeviceList::new(list)) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            crate::gateway::protocol::INTERNAL_ERROR,
            format!("failed to encode devices: {e}"),
        ),
    }
}

/// `gateway.devices.revoke` — revoke one paired Panel device by id.
///
/// Request params: `{ "device_id": "device-…" }` (required).
/// Response: `{ "revoked": bool, "device_id": "device-…" }` — `revoked` is
/// `false` when the id is unknown, already revoked, or is not a Panel device
/// (e.g. a cluster node). The store write and the live-session kick both
/// happen inside [`revoke_device_and_kick`] before this handler responds, so
/// `revoked: true` means the device is already gone by the time the caller
/// sees the reply.
pub async fn handle_devices_revoke(
    request: JsonRpcRequest,
    ctx: Arc<DevicesHandlerContext>,
) -> JsonRpcResponse {
    let device_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("device_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let Some(device_id) = device_id else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "missing required param: device_id".to_string(),
        );
    };

    match revoke_device_and_kick(
        &ctx.device_token_mgr,
        &ctx.connections,
        &ctx.event_bus,
        device_id,
    )
    .await
    {
        Ok(revoked) => JsonRpcResponse::success(
            request.id,
            json!({ "revoked": revoked, "device_id": device_id }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            crate::gateway::protocol::INTERNAL_ERROR,
            format!("failed to revoke device: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::SecurityStore;

    fn ctx() -> Arc<DevicesHandlerContext> {
        Arc::new(DevicesHandlerContext {
            device_token_mgr: Arc::new(DeviceTokenManager::new(Arc::new(
                SecurityStore::in_memory().unwrap(),
            ))),
            presence: Arc::new(PresenceTracker::new()),
            connections: Arc::new(RwLock::new(HashMap::new())),
            event_bus: Arc::new(GatewayEventBus::new()),
        })
    }

    #[tokio::test]
    async fn list_returns_only_paired_panels() {
        let ctx = ctx();
        let ticket = ctx
            .device_token_mgr
            .create_bootstrap_ticket(None, None)
            .unwrap();
        ctx.device_token_mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();

        let req = JsonRpcRequest::with_id("gateway.devices.list", None, json!(1));
        let resp = handle_devices_list(req, ctx).await;
        assert!(resp.is_success(), "{resp:?}");
        let devices = resp.result.unwrap();
        let arr = devices.get("devices").and_then(|v| v.as_array()).unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].get("device_id").unwrap(), "panel-1");
    }

    #[tokio::test]
    async fn connected_flag_joins_the_presence_roster() {
        let ctx = ctx();
        let ticket = ctx
            .device_token_mgr
            .create_bootstrap_ticket(None, None)
            .unwrap();
        for id in ["panel-online", "panel-offline"] {
            let t = if id == "panel-online" {
                ticket.clone()
            } else {
                ctx.device_token_mgr
                    .create_bootstrap_ticket(None, None)
                    .unwrap()
            };
            ctx.device_token_mgr
                .exchange_bootstrap_ticket(&t, Some(id.to_string()), None, None)
                .unwrap();
        }
        ctx.presence.upsert(
            "conn-1".to_string(),
            crate::gateway::presence::PresenceEntry {
                conn_id: "conn-1".to_string(),
                device_id: Some("panel-online".to_string()),
                device_name: "phone".to_string(),
                platform: "ios".to_string(),
                role: crate::gateway::presence::ConnectionRole::User,
                connected_at: chrono::Utc::now(),
                last_heartbeat: chrono::Utc::now(),
            },
        );

        let req = JsonRpcRequest::with_id("gateway.devices.list", None, json!(1));
        let resp = handle_devices_list(req, ctx).await;
        let devices = resp.result.unwrap();
        let arr = devices.get("devices").and_then(|v| v.as_array()).unwrap();
        let flag = |id: &str| {
            arr.iter()
                .find(|d| d.get("device_id").unwrap() == id)
                .and_then(|d| d.get("connected"))
                .and_then(serde_json::Value::as_bool)
                .unwrap()
        };
        assert!(
            flag("panel-online"),
            "an open session must read as connected"
        );
        assert!(!flag("panel-offline"));
    }

    /// `connected_since` is the MINIMUM presence `connected_at` over the
    /// device's live connections, and null when none is live. A strictly
    /// greater value on a later read is therefore proof that every earlier
    /// connection closed and a newer one is bound — the reconnect witness a
    /// sampled `connected` flag misses when the gap is shorter than the poll.
    #[tokio::test]
    async fn connected_since_is_the_oldest_live_connection_and_null_when_offline() {
        use crate::gateway::presence::{ConnectionRole, PresenceEntry};
        use chrono::TimeZone;
        let ctx = ctx();
        for id in ["panel-online", "panel-offline"] {
            let t = ctx
                .device_token_mgr
                .create_bootstrap_ticket(None, None)
                .unwrap();
            ctx.device_token_mgr
                .exchange_bootstrap_ticket(&t, Some(id.to_string()), None, None)
                .unwrap();
        }
        let entry = |conn: &str, device: &str, at_ms: i64| PresenceEntry {
            conn_id: conn.to_string(),
            device_id: Some(device.to_string()),
            device_name: "panel".to_string(),
            platform: "desktop".to_string(),
            role: ConnectionRole::User,
            connected_at: chrono::Utc.timestamp_millis_opt(at_ms).unwrap(),
            last_heartbeat: chrono::Utc.timestamp_millis_opt(at_ms).unwrap(),
        };
        // Two live connections for the same device: the older one wins.
        ctx.presence
            .upsert("conn-new".into(), entry("conn-new", "panel-online", 2_000));
        ctx.presence
            .upsert("conn-old".into(), entry("conn-old", "panel-online", 1_000));

        let since = |ctx: Arc<DevicesHandlerContext>| async move {
            let req = JsonRpcRequest::with_id("gateway.devices.list", None, json!(1));
            let list: PairedDeviceList =
                serde_json::from_value(handle_devices_list(req, ctx).await.result.unwrap())
                    .unwrap();
            list.devices
                .into_iter()
                .map(|d| (d.device_id, (d.connected, d.connected_since)))
                .collect::<std::collections::HashMap<_, _>>()
        };
        let rows = since(ctx.clone()).await;
        assert_eq!(rows["panel-online"], (true, Some(1_000)));
        assert_eq!(rows["panel-offline"], (false, None));

        // The old connection closes: the witness advances strictly.
        assert!(ctx.presence.remove("conn-old").is_some());
        assert_eq!(
            since(ctx.clone()).await["panel-online"],
            (true, Some(2_000))
        );

        // Every connection closes: offline and null, never a stale start.
        assert!(ctx.presence.remove("conn-new").is_some());
        assert_eq!(since(ctx).await["panel-online"], (false, None));
    }

    #[tokio::test]
    async fn revoke_requires_device_id() {
        let req = JsonRpcRequest::with_id("gateway.devices.revoke", Some(json!({})), json!(1));
        let resp = handle_devices_revoke(req, ctx()).await;
        assert!(resp.is_error());
    }

    #[tokio::test]
    async fn revoke_invalidates_the_device_token() {
        let ctx = ctx();
        let ticket = ctx
            .device_token_mgr
            .create_bootstrap_ticket(None, None)
            .unwrap();
        let paired = ctx
            .device_token_mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();

        let req = JsonRpcRequest::with_id(
            "gateway.devices.revoke",
            Some(json!({ "device_id": "panel-1" })),
            json!(1),
        );
        let resp = handle_devices_revoke(req, ctx.clone()).await;
        assert!(resp.is_success(), "{resp:?}");
        assert_eq!(resp.result.unwrap().get("revoked").unwrap(), &json!(true));

        assert!(ctx
            .device_token_mgr
            .validate_device_token(&paired.device_token)
            .unwrap()
            .is_none());
    }

    // ── Real-effect: revoke clears live verified sessions (single + bulk) ──

    use crate::gateway::security::store::{DeviceUpsertData, UserRole, OWNER_USER_ID as OWNER};
    use crate::gateway::server::connection::auth::finalize_handshake_identity;

    /// A connection stamped by the REAL handshake verdict function.
    fn live_conn(store: &SecurityStore, device: Option<&str>, local: bool) -> ConnectionState {
        let ip = if local { "127.0.0.1" } else { "203.0.113.7" };
        let mut cs = ConnectionState::new(ip.parse().unwrap(), local);
        let (user, role, verified) =
            finalize_handshake_identity(true, local, device, device, Some(store));
        cs.caller_user = user;
        cs.caller_role = role.to_string();
        cs.verified_user = verified;
        cs.device_id = device.map(String::from);
        cs.permissions = vec!["*".to_string()];
        cs
    }

    struct Fixture {
        store: Arc<SecurityStore>,
        mgr: Arc<DeviceTokenManager>,
        conns: Arc<RwLock<HashMap<String, ConnectionState>>>,
        bus: Arc<GatewayEventBus>,
        tokens: HashMap<&'static str, String>,
    }

    fn pair(mgr: &DeviceTokenManager, user: Option<&str>, id: &str) -> String {
        let t = mgr.create_bootstrap_ticket(None, user).unwrap();
        mgr.exchange_bootstrap_ticket(&t, Some(id.to_string()), None, None)
            .unwrap()
            .device_token
    }

    /// Owner + member Panel devices, each with a loopback and a remote live
    /// session, plus a cluster node session and a credential-less compat
    /// loopback session.
    async fn fixture() -> Fixture {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr = Arc::new(DeviceTokenManager::new(store.clone()));
        let mut tokens = HashMap::new();
        tokens.insert("panel-owner", pair(&mgr, None, "panel-owner"));
        tokens.insert("panel-alice", pair(&mgr, Some("u-alice"), "panel-alice"));
        store
            .upsert_device(&DeviceUpsertData {
                device_id: "node-1",
                device_name: "node",
                device_type: Some("node"),
                public_key: &[2u8; 32],
                fingerprint: "node-1",
                role: "node",
                scopes: &[],
                user_id: None,
            })
            .unwrap();
        let mut map = HashMap::new();
        map.insert(
            "owner-loop".to_string(),
            live_conn(&store, Some("panel-owner"), true),
        );
        map.insert(
            "owner-remote".to_string(),
            live_conn(&store, Some("panel-owner"), false),
        );
        map.insert(
            "alice-loop".to_string(),
            live_conn(&store, Some("panel-alice"), true),
        );
        map.insert(
            "alice-remote".to_string(),
            live_conn(&store, Some("panel-alice"), false),
        );
        map.insert("node".to_string(), live_conn(&store, Some("node-1"), false));
        map.insert("compat".to_string(), live_conn(&store, None, true));
        Fixture {
            store,
            mgr,
            conns: Arc::new(RwLock::new(map)),
            bus: Arc::new(GatewayEventBus::new()),
            tokens,
        }
    }

    #[tokio::test]
    async fn fixture_sessions_start_verified() {
        let f = fixture().await;
        let m = f.conns.read().await;
        for c in ["owner-loop", "owner-remote"] {
            assert_eq!(m[c].verified_user.as_deref(), Some(OWNER), "{c}");
        }
        for c in ["alice-loop", "alice-remote"] {
            assert_eq!(m[c].verified_user.as_deref(), Some("u-alice"), "{c}");
        }
        assert_eq!(m["compat"].verified_user, None);
        assert_eq!(m["compat"].caller_user.as_deref(), Some(OWNER));
    }

    #[tokio::test]
    async fn bulk_revoke_clears_every_panel_session_and_rejects_tokens() {
        let f = fixture().await;
        let mut rx = f.bus.subscribe_typed();
        let node_before = {
            let m = f.conns.read().await;
            (m["node"].caller_role.clone(), m["node"].caller_user.clone())
        };

        let n = revoke_all_panel_devices_and_kick(&f.mgr, &f.conns, &f.bus)
            .await
            .unwrap();
        assert_eq!(n, 2, "both Panel devices revoked; node untouched");

        // Tokens really rejected.
        for (id, tok) in &f.tokens {
            assert!(
                f.mgr.validate_device_token(tok).unwrap().is_none(),
                "{id} token must be rejected"
            );
        }
        assert!(!f.store.is_device_approved("panel-owner").unwrap());
        assert!(!f.store.is_device_approved("panel-alice").unwrap());
        assert!(
            f.store.is_device_approved("node-1").unwrap(),
            "node untouched"
        );

        let m = f.conns.read().await;
        for c in ["owner-loop", "owner-remote", "alice-loop", "alice-remote"] {
            assert_eq!(m[c].caller_role, "guest", "{c}");
            assert_eq!(m[c].caller_user, None, "{c}");
            assert_eq!(m[c].verified_user, None, "{c}");
            assert!(m[c].permissions.is_empty(), "{c}");
        }
        // Nodes and credential-less compat sessions are not device-bound
        // Panel sessions: untouched.
        assert_eq!(m["node"].caller_role, node_before.0);
        assert_eq!(m["node"].caller_user, node_before.1);
        assert_ne!(m["node"].caller_role, "guest");
        assert_eq!(m["compat"].caller_user.as_deref(), Some(OWNER));
        assert_ne!(m["compat"].caller_role, "guest");
        assert!(!m["compat"].permissions.is_empty());

        // One DeviceRevoked per revoked Panel device, none for the node.
        let mut revoked = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let GatewayEventFrame::DeviceRevoked { device_id } = frame {
                revoked.push(device_id);
            }
        }
        revoked.sort();
        assert_eq!(
            revoked,
            vec!["panel-alice".to_string(), "panel-owner".to_string()]
        );
    }

    #[tokio::test]
    async fn single_revoke_clears_only_that_device_and_final_handshake_cannot_restore() {
        let f = fixture().await;
        assert!(
            revoke_device_and_kick(&f.mgr, &f.conns, &f.bus, "panel-alice")
                .await
                .unwrap()
        );
        {
            let m = f.conns.read().await;
            for c in ["alice-loop", "alice-remote"] {
                assert_eq!(m[c].caller_role, "guest");
                assert_eq!(m[c].verified_user, None);
                assert_eq!(m[c].caller_user, None);
            }
            assert_eq!(m["owner-remote"].verified_user.as_deref(), Some(OWNER));
        }
        // A handshake whose credential check passed BEFORE the revoke runs its
        // final verdict AFTER it: it must not restore the identity.
        let (u, role, v) = finalize_handshake_identity(
            true,
            false,
            Some("panel-alice"),
            Some("panel-alice"),
            Some(&f.store),
        );
        assert_eq!((u, role, v), (None, "guest", None));
        // And a second revoke is a no-op reporting false.
        assert!(
            !revoke_device_and_kick(&f.mgr, &f.conns, &f.bus, "panel-alice")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn stamp_before_revoke_is_still_cleared() {
        // Order B: the handshake stamp lands first, the revoke second.
        let f = fixture().await;
        let stamped = live_conn(&f.store, Some("panel-owner"), false);
        assert_eq!(stamped.verified_user.as_deref(), Some(OWNER));
        f.conns.write().await.insert("late".to_string(), stamped);
        revoke_device_and_kick(&f.mgr, &f.conns, &f.bus, "panel-owner")
            .await
            .unwrap();
        let m = f.conns.read().await;
        assert_eq!(m["late"].verified_user, None);
        assert_eq!(m["late"].caller_role, "guest");
    }

    #[tokio::test]
    async fn bulk_revoke_with_no_panel_devices_is_a_counted_no_op() {
        let c = ctx();
        let n =
            revoke_all_panel_devices_and_kick(&c.device_token_mgr, &c.connections, &c.event_bus)
                .await
                .unwrap();
        assert_eq!(n, 0);
    }

    /// The inventory is the surface an operator revokes a credential from, so
    /// what it sends is a contract. Nothing pinned the key set before this
    /// test, which is why `created_at` — sent since the list shipped, rendered
    /// by no client in any language — was invisible.
    #[tokio::test]
    async fn a_listed_row_carries_exactly_the_declared_wire_keys() {
        let ctx = ctx();
        let ticket = ctx
            .device_token_mgr
            .create_bootstrap_ticket(None, None)
            .unwrap();
        ctx.device_token_mgr
            .exchange_bootstrap_ticket(&ticket, Some("panel-1".to_string()), None, None)
            .unwrap();

        let req = JsonRpcRequest::with_id("gateway.devices.list", None, json!(1));
        let resp = handle_devices_list(req, ctx).await;
        let result = resp.result.unwrap();
        let arr = result.get("devices").and_then(|v| v.as_array()).unwrap();
        let mut keys: Vec<&str> = arr[0]
            .as_object()
            .expect("a device row is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "connected",
                "connected_since",
                "device_id",
                "device_name",
                "display_name",
                "last_seen_at",
                "user_id",
            ],
            "the emitted row must be exactly `PairedDeviceRow`; an extra key \
             here is a field with no renderer, a missing one is a blank column"
        );
    }
}
