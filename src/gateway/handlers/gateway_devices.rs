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
    let revoked = device_token_mgr.revoke_panel_device(device_id)?;
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
            ));
        }
        let downgraded =
            crate::gateway::server::invalidate_device_sessions(connections, device_id).await;
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

/// `gateway.devices.list` — list paired remote Panel devices.
///
/// Response: `{ "devices": [{ device_id, device_name, created_at,
/// last_seen_at, connected }] }`. `connected` is a live join against the
/// presence roster (mirrors openclaw's `device.pair.list` `connected` flag) —
/// without it "Last seen 3 minutes ago" is the only signal an operator has, and
/// revoking is now a session-killing action that deserves to say so up front.
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

    let online: std::collections::HashSet<String> = ctx
        .presence
        .list()
        .into_iter()
        .filter_map(|e| e.device_id)
        .collect();

    let list: Vec<_> = devices
        .into_iter()
        .map(|d| {
            json!({
                "connected": online.contains(&d.device_id),
                "device_id": d.device_id,
                "device_name": d.device_name,
                "created_at": d.created_at,
                "last_seen_at": d.last_seen_at,
                // Which principal this device speaks as. SECURITY.md calls
                // this list "the inventory", and until 2026-08-13 it was an
                // inventory with no owners: five members' phones showed as
                // five rows named "iPhone", so revoking the right one was
                // guesswork and verifying that a deactivation actually cut
                // someone's devices was impossible from any surface.
                // `display_name` resolves through the same directory
                // projection the channel-pairing list and the room bubbles
                // use, so an operator reads a name rather than a `u-` id.
                "user_id": d.user_id,
                "display_name": d.user_id.as_deref().and_then(crate::scope::directory::display_name),
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "devices": list }))
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
}
