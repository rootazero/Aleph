//! User (principal) management handlers — `users.me` / `users.list` /
//! `users.create` / `users.update`.
//!
//! Backed by [`crate::gateway::security::store::SecurityStore`]'s `users`
//! table (Task 1). Admin authorization for `users.create` / `users.update` is
//! enforced once, upstream, by `method_admin.rs` (spec §4.6) — these handlers
//! never re-check the caller's role (single-chokepoint discipline). `users.me`
//! and `users.list` are member-safe carve-outs.
//!
//! Both mutating verdicts reach **already-open sessions**, not just the next
//! connect — the wire role is latched into `ConnectionState` at the handshake,
//! so a store-only write would leave live connections stale indefinitely:
//!
//! - Deactivation (`users.update { status: "deactivated" }`) revokes every
//!   live device bound to that user through [`revoke_device_and_kick`], the
//!   exact pipeline `gateway.devices.revoke` uses — same store write, same
//!   `DeviceRevoked` event, same demote-before-kick order. See that function's
//!   doc in `gateway_devices.rs` for why it isn't duplicated here.
//! - A role change (`users.update { role }`) re-stamps those same connections
//!   in place via [`restamp_live_connections`] — promotion and demotion both,
//!   except for connections already walled at `"guest"` (see that function).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;

use super::gateway_devices::revoke_device_and_kick;
use super::parse_params;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::store::{
    SecurityStore, UserRecord, UserRole, UserStatus, OWNER_USER_ID,
};
use crate::gateway::security::DeviceTokenManager;
use crate::gateway::server::ConnectionState;
use crate::sync_primitives::Arc;

/// Serializable view of a user — role/status render as their wire strings so
/// Panel/CLI consumers never touch the enum representation.
#[derive(Debug, Clone, Serialize)]
pub struct UserView {
    pub user_id: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
}

impl From<UserRecord> for UserView {
    fn from(u: UserRecord) -> Self {
        Self {
            user_id: u.user_id,
            display_name: u.display_name,
            role: u.role.as_str().to_string(),
            status: u.status.as_str().to_string(),
        }
    }
}

/// Dependencies `users.update`'s deactivation path needs to kick a
/// deactivated user's live device sessions — the same connection map and
/// event bus `gateway.devices.revoke` is wired with. Kept separate from
/// `SecurityStore` (rather than folded into a bigger context struct) so
/// tests can inject empty/no-op instances and assert only the store effect,
/// mirroring `DevicesHandlerContext`.
#[derive(Clone)]
pub struct UserDeactivationKick {
    pub connections: Arc<RwLock<HashMap<String, ConnectionState>>>,
    pub event_bus: Arc<GatewayEventBus>,
}

// ============================================================================
// users.me
// ============================================================================

/// `users.me` → `{ "user": UserView | null }`. Reads the caller's own
/// principal record via [`current_caller_user`](crate::gateway::caller_identity::current_caller_user).
/// `null` (not an error) when no caller user is scoped (e.g. non-gateway
/// callers) or the id doesn't resolve to a row.
pub async fn handle_me(request: JsonRpcRequest, store: Arc<SecurityStore>) -> JsonRpcResponse {
    let Some(caller_id) = crate::gateway::caller_identity::current_caller_user() else {
        return JsonRpcResponse::success(request.id, json!({ "user": null }));
    };

    match store.get_user(&caller_id) {
        Ok(user) => {
            let view = user.map(UserView::from);
            JsonRpcResponse::success(request.id, json!({ "user": view }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to read caller user: {e}"),
        ),
    }
}

// ============================================================================
// users.list
// ============================================================================

/// `users.list` → `{ "users": [UserView] }`. Member-visible (project roster
/// picking needs the member list).
pub async fn handle_list(request: JsonRpcRequest, store: Arc<SecurityStore>) -> JsonRpcResponse {
    match store.list_users() {
        Ok(users) => {
            let view: Vec<UserView> = users.into_iter().map(UserView::from).collect();
            JsonRpcResponse::success(request.id, json!({ "users": view }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to list users: {e}"),
        ),
    }
}

// ============================================================================
// users.create
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub display_name: String,
    #[serde(default)]
    pub role: Option<String>,
}

/// `users.create { display_name, role? ("member") }` → `{ "user": UserView }`.
/// `user_id` is server-generated (`u-<uuid v4>`). An unrecognized `role`
/// string is rejected loudly (invalid params) rather than silently defaulted,
/// as is an empty or whitespace-only `display_name`.
pub async fn handle_create(request: JsonRpcRequest, store: Arc<SecurityStore>) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if params.display_name.trim().is_empty() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "display_name must not be empty".to_string(),
        );
    }

    let role = match params.role.as_deref() {
        Some(s) => match UserRole::from_str(s) {
            Some(r) => r,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("unknown role: {s}"),
                )
            }
        },
        None => UserRole::Member,
    };

    let user_id = format!("u-{}", uuid::Uuid::new_v4());
    match store.create_user(&user_id, &params.display_name, role) {
        Ok(()) => {
            let view = UserView {
                user_id,
                display_name: params.display_name,
                role: role.as_str().to_string(),
                status: UserStatus::Active.as_str().to_string(),
            };
            JsonRpcResponse::success(request.id, json!({ "user": view }))
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to create user: {e}"),
        ),
    }
}

// ============================================================================
// users.update
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub user_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

/// `users.update { user_id, display_name?, role?, status? }` → `{ "user": UserView }`.
///
/// Unrecognized `role`/`status` strings are rejected (invalid params) rather
/// than silently ignored. The owner guard (spec §10) runs before any store
/// write: `user_id == OWNER_USER_ID` refuses `status="deactivated"` and
/// `role="member"` — the loopback arm always resolves to the owner regardless
/// of stored status, so deactivating it would produce a half-effective state
/// (remote devices kicked, local access unaffected), and this also guarantees
/// the system always keeps at least one admin.
///
/// `status="deactivated"` additionally revokes every live device bound to
/// this user via [`revoke_device_and_kick`] (best-effort per device — a
/// revoke failure is logged, not surfaced as a whole-request failure, since
/// the status write already succeeded).
pub async fn handle_update(
    request: JsonRpcRequest,
    store: Arc<SecurityStore>,
    kick: UserDeactivationKick,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let role = match params.role.as_deref() {
        Some(s) => match UserRole::from_str(s) {
            Some(r) => Some(r),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("unknown role: {s}"),
                )
            }
        },
        None => None,
    };
    let status = match params.status.as_deref() {
        Some(s) => match UserStatus::from_str(s) {
            Some(v) => Some(v),
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("unknown status: {s}"),
                )
            }
        },
        None => None,
    };

    // Owner guard — before any store write (spec §10).
    if params.user_id == OWNER_USER_ID {
        if status == Some(UserStatus::Deactivated) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "the owner account cannot be deactivated".to_string(),
            );
        }
        if role == Some(UserRole::Member) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "the owner account cannot be demoted".to_string(),
            );
        }
    }

    let rows = match store.update_user(
        &params.user_id,
        params.display_name.as_deref(),
        role,
        status,
    ) {
        Ok(n) => n,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to update user: {e}"),
            )
        }
    };
    if rows == 0 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("user not found: {}", params.user_id),
        );
    }

    // Order matters: re-stamp first, deactivate second. When one call does
    // both (`role` + `status: "deactivated"`), the deactivation must have the
    // last word — it demotes those same connections to guest and closes them.
    if let Some(new_role) = role {
        restamp_live_connections(&store, &kick, &params.user_id, new_role).await;
    }

    if status == Some(UserStatus::Deactivated) {
        deactivate_devices(&store, &kick, &params.user_id).await;
    }

    match store.get_user(&params.user_id) {
        Ok(Some(user)) => {
            JsonRpcResponse::success(request.id, json!({ "user": UserView::from(user) }))
        }
        Ok(None) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "user vanished immediately after update".to_string(),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to read updated user: {e}"),
        ),
    }
}

/// Re-stamp `caller_role` **and the event scope** on every live connection
/// belonging to `user_id`'s devices, so a promotion/demotion takes effect on
/// sessions that are already open.
///
/// Without this, a role change is latched-at-`connect` only: the wire role
/// lives in `ConnectionState.caller_role`, written once at the handshake and
/// read from there by the login wall and the admin gate on every later frame.
/// A demoted admin would keep full admin authority on its open Panel tab
/// indefinitely (until it happened to reconnect) — the exact indefinite window
/// deactivation already closes via `revoke_device_and_kick`.
///
/// Same lock discipline as
/// [`invalidate_device_sessions`](crate::gateway::server::invalidate_device_sessions):
/// one write lock over the shared connection map, mutate in place, drop before
/// logging. Connections are matched by `device_id` — the same key the revoke
/// path uses — so a connection that merely carries a `caller_user` value it
/// was never bound to cannot be re-stamped by someone else's role change.
///
/// **Never promotes a walled connection.** A connection already sitting at
/// `"guest"` was put there deliberately (its device was revoked, or its user
/// deactivated); only a fresh `connect` may lift that. Re-stamping it would
/// resurrect a revoked device's authority through the back door.
///
/// The role and the event scope move together, both derived from
/// [`scope_for_role`](crate::gateway::event_scope::scope_for_role) — the same
/// authority the `connect` handshake stamps from. Restamping only the role
/// would leave a demoted admin holding the `"*"` wildcard on his open tab, i.e.
/// still receiving exec approval cards and their command text, until he
/// happened to reconnect — the same indefinite window this function exists to
/// close on the role axis.
async fn restamp_live_connections(
    store: &Arc<SecurityStore>,
    kick: &UserDeactivationKick,
    user_id: &str,
    role: UserRole,
) {
    let device_ids = match store.list_device_ids_for_user(user_id) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.update: failed to list devices for the live role re-stamp"
            );
            return;
        }
    };
    if device_ids.is_empty() {
        return;
    }

    // Wire word, not the enum's storage word: `admin` ⇒ `"operator"` is the
    // same mapping `resolve_connection_identity` applies at connect time.
    let wanted = match role {
        UserRole::Admin => "operator",
        UserRole::Member => "member",
    };
    let bound: std::collections::HashSet<&str> = device_ids.iter().map(String::as_str).collect();

    let mut restamped = 0usize;
    {
        let mut conns = kick.connections.write().await;
        for state in conns.values_mut() {
            let Some(did) = state.device_id.as_deref() else {
                continue;
            };
            if !bound.contains(did) || state.caller_role == "guest" {
                continue;
            }
            if state.caller_role != wanted {
                state.caller_role = wanted.to_string();
                state.permissions = crate::gateway::event_scope::scope_for_role(wanted);
                restamped += 1;
            }
        }
    }

    if restamped > 0 {
        tracing::info!(
            user_id = %user_id,
            role = %wanted,
            sessions = restamped,
            "users.update: role change applied to live connections"
        );
    }
}

/// Revoke every live device bound to `user_id`, one call per device through
/// the shared [`revoke_device_and_kick`] pipeline. Best-effort: a single
/// device's revoke failing, or coming back a no-op for a device the store
/// just listed as live, is logged and does not abort the rest.
async fn deactivate_devices(
    store: &Arc<SecurityStore>,
    kick: &UserDeactivationKick,
    user_id: &str,
) {
    let device_ids = match store.list_device_ids_for_user(user_id) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.update: failed to list devices for deactivation"
            );
            return;
        }
    };

    let device_token_mgr = DeviceTokenManager::new(store.clone());
    for device_id in device_ids {
        match revoke_device_and_kick(
            &device_token_mgr,
            &kick.connections,
            &kick.event_bus,
            &device_id,
        )
        .await
        {
            Ok(true) => {}
            // `list_device_ids_for_user` just reported this device as live
            // (`revoked_at IS NULL`), so a no-op here means
            // `revoke_panel_device` didn't recognize it as a panel device —
            // e.g. a `devices` row whose `device_id` collides with the
            // cluster-node namespace (`devices` is shared between panel and
            // node rows, see gateway/CLAUDE.md mine 3). That's a silent skip
            // worth a name in the logs, not a hard failure.
            Ok(false) => {
                tracing::warn!(
                    device_id = %device_id,
                    user_id = %user_id,
                    "users.update: device listed as live for user but revoke was a no-op"
                );
            }
            Err(e) => {
                tracing::warn!(
                    device_id = %device_id,
                    user_id = %user_id,
                    error = %e,
                    "users.update: failed to revoke device during deactivation"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::DeviceUpsertData;

    /// `SecurityStore::in_memory()` runs migrations, including owner
    /// bootstrap, so a fresh store already contains the owner user (mirrors
    /// `handlers/connect.rs`'s test fixture of the same name).
    fn seeded_store() -> Arc<SecurityStore> {
        Arc::new(SecurityStore::in_memory().unwrap())
    }

    /// Upsert a panel device and bind it to `user_id` — mirrors
    /// `handlers/connect.rs`'s test fixture of the same name.
    fn upsert_panel_device(store: &SecurityStore, device_id: &str, user_id: &str) {
        store
            .upsert_device(&DeviceUpsertData {
                device_id,
                device_name: "Test Device",
                device_type: Some("panel"),
                public_key: &[1u8; 32],
                fingerprint: device_id,
                role: "operator",
                scopes: &[],
                user_id: None,
            })
            .unwrap();
        store.set_device_user(device_id, user_id).unwrap();
    }

    fn rpc_request(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    fn response_json(resp: &JsonRpcResponse) -> serde_json::Value {
        resp.result.clone().expect("expected a success response")
    }

    fn response_is_error(resp: &JsonRpcResponse) -> bool {
        resp.is_error()
    }

    /// Test double for `users.update`'s deactivation kick dependencies — an
    /// empty connection map and a fresh event bus. `revoke_device_and_kick`
    /// (called by `deactivate_devices` below) runs its full demote-then-kick
    /// body against whatever is seeded here: most tests leave the map empty
    /// and only assert the store effect (devices revoked), but
    /// `deactivate_demotes_live_connections_for_kicked_devices` below seeds a
    /// connection into `.connections` to pin the demote half from this call
    /// site too. The physical socket-*close* reaction to the `DeviceRevoked`
    /// event (severing the WS) is separate — it lives in the WS dispatch
    /// loop (`server/handler.rs::device_revoked_should_close`), already
    /// covered by that module's own tests, and isn't exercised here.
    fn test_kick_sink() -> UserDeactivationKick {
        UserDeactivationKick {
            connections: Arc::new(RwLock::new(HashMap::new())),
            event_bus: Arc::new(GatewayEventBus::new()),
        }
    }

    #[tokio::test]
    async fn me_reflects_caller_user() {
        let store = seeded_store();
        let req = rpc_request("users.me", json!({}));
        let resp = crate::gateway::caller_identity::CALLER_USER
            .scope(Some("u-owner".to_string()), handle_me(req, store.clone()))
            .await;
        let v = response_json(&resp);
        assert_eq!(v["user"]["user_id"], "u-owner");
        assert_eq!(v["user"]["role"], "admin");
    }

    #[tokio::test]
    async fn me_is_null_when_no_caller_user_is_scoped() {
        let store = seeded_store();
        let req = rpc_request("users.me", json!({}));
        // No CALLER_USER scope — must not error.
        let resp = handle_me(req, store).await;
        assert!(resp.is_success(), "{resp:?}");
        let v = response_json(&resp);
        assert!(v["user"].is_null());
    }

    #[tokio::test]
    async fn create_then_list_shows_member() {
        let store = seeded_store();
        let req = rpc_request("users.create", json!({"display_name": "Alice"}));
        let resp = handle_create(req, store.clone()).await;
        let created = response_json(&resp);
        assert_eq!(created["user"]["role"], "member");

        let listed = response_json(&handle_list(rpc_request("users.list", json!({})), store).await);
        assert_eq!(listed["users"].as_array().unwrap().len(), 2); // owner + alice
    }

    #[tokio::test]
    async fn create_rejects_empty_display_name() {
        let store = seeded_store();
        for name in ["", "   "] {
            let resp = handle_create(
                rpc_request("users.create", json!({"display_name": name})),
                store.clone(),
            )
            .await;
            assert!(response_is_error(&resp), "{name:?} must be rejected");
        }
        // Owner only — no half-created rows from the rejected calls.
        assert_eq!(store.list_users().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deactivate_revokes_all_user_devices() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        upsert_panel_device(&store, "dev-a1", "u-alice");
        upsert_panel_device(&store, "dev-a2", "u-alice");
        assert_eq!(
            store.list_device_ids_for_user("u-alice").unwrap().len(),
            2,
            "fixture must seed two live devices, or the post-update empty check below passes vacuously"
        );

        let req = rpc_request(
            "users.update",
            json!({"user_id": "u-alice", "status": "deactivated"}),
        );
        handle_update(req, store.clone(), test_kick_sink()).await;

        assert!(
            store
                .list_device_ids_for_user("u-alice")
                .unwrap()
                .is_empty(),
            "live (un-revoked) device list must be empty after deactivation"
        );
    }

    /// Pins the demote half of demote-before-kick (gateway/CLAUDE.md mine 2)
    /// from the `users.update` call site: `revoke_device_and_kick` must
    /// downgrade any connection bound to a revoked device to the login wall
    /// (`caller_role = "guest"`, `caller_user = None`), not just flip the
    /// store row. `gateway_devices.rs`'s own tests only exercise the RPC
    /// response and store effect for `gateway.devices.revoke`; this is the
    /// connection-state assertion for the shared function, reachable from
    /// this (the `users.update`) call site.
    #[tokio::test]
    async fn deactivate_demotes_live_connections_for_kicked_devices() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        upsert_panel_device(&store, "dev-a1", "u-alice");

        let kick = test_kick_sink();
        let connections = kick.connections.clone();
        {
            let mut conns = connections.write().await;
            let mut state = ConnectionState::new(std::net::IpAddr::from([203, 0, 113, 7]));
            state.caller_role = "operator".to_string();
            state.caller_user = Some("u-alice".to_string());
            state.device_id = Some("dev-a1".to_string());
            conns.insert("conn-alice".to_string(), state);
        }

        let req = rpc_request(
            "users.update",
            json!({"user_id": "u-alice", "status": "deactivated"}),
        );
        handle_update(req, store.clone(), kick).await;

        let conns = connections.read().await;
        let demoted = conns
            .get("conn-alice")
            .expect("connection must still be present, only demoted");
        assert_eq!(demoted.caller_role, "guest", "must be downgraded to guest");
        assert_eq!(demoted.caller_user, None, "caller_user must be cleared");
    }

    /// Seed one live connection bound to `device_id` at `role`, and return
    /// the kick sink whose map holds it (so the caller can read it back).
    async fn kick_with_live_connection(
        conn_id: &str,
        device_id: &str,
        user_id: &str,
        role: &str,
    ) -> UserDeactivationKick {
        let kick = test_kick_sink();
        {
            let mut conns = kick.connections.write().await;
            let mut state = ConnectionState::new(std::net::IpAddr::from([203, 0, 113, 7]));
            state.caller_role = role.to_string();
            state.caller_user = Some(user_id.to_string());
            state.device_id = Some(device_id.to_string());
            // Seed the scope the `connect` handshake would have stamped for
            // this role, so the re-stamp assertions below measure a real
            // transition rather than an already-empty vec.
            state.permissions = crate::gateway::event_scope::scope_for_role(role);
            conns.insert(conn_id.to_string(), state);
        }
        kick
    }

    /// Mirror of `deactivate_demotes_live_connections_for_kicked_devices` for
    /// the role axis: a role change must reach sessions that are ALREADY open,
    /// not merely the user's next connect. The stamped role is latched at the
    /// handshake and read on every later frame, so a store-only write leaves a
    /// demoted admin holding admin authority on its open tab indefinitely.
    #[tokio::test]
    async fn role_change_restamps_live_connections_both_directions() {
        // Demote: admin → member.
        {
            let store = seeded_store();
            store
                .create_user("u-boss", "Boss", UserRole::Admin)
                .unwrap();
            upsert_panel_device(&store, "dev-b1", "u-boss");
            let kick = kick_with_live_connection("conn-boss", "dev-b1", "u-boss", "operator").await;
            let conns = kick.connections.clone();

            let resp = handle_update(
                rpc_request(
                    "users.update",
                    json!({"user_id": "u-boss", "role": "member"}),
                ),
                store,
                kick,
            )
            .await;
            assert!(resp.is_success(), "{resp:?}");

            let c = conns.read().await;
            let s = c.get("conn-boss").unwrap();
            assert_eq!(
                s.caller_role, "member",
                "a demoted admin's live session must lose operator authority immediately"
            );
            // The event scope must narrow in the same breath, or the demoted
            // admin keeps receiving exec approval cards (and the command text
            // inside them) on the tab he already has open.
            assert!(
                s.permissions.is_empty(),
                "a demoted admin's live session must lose the `*` event scope"
            );
            let guard = crate::gateway::event_scope::EventScopeGuard::default_rules();
            assert!(
                !guard.can_receive("approval.requested", &s.permissions),
                "a demoted admin must no longer be delivered approval cards"
            );
            assert!(
                !guard.can_receive("config.changed", &s.permissions),
                "a demoted admin must no longer be delivered config.changed"
            );
            assert!(
                guard.can_receive("agent.run.started", &s.permissions),
                "narrowing the scope must not black out ordinary run events"
            );
        }

        // Promote: member → admin.
        {
            let store = seeded_store();
            store
                .create_user("u-alice", "Alice", UserRole::Member)
                .unwrap();
            upsert_panel_device(&store, "dev-a1", "u-alice");
            let kick = kick_with_live_connection("conn-alice", "dev-a1", "u-alice", "member").await;
            let conns = kick.connections.clone();

            handle_update(
                rpc_request(
                    "users.update",
                    json!({"user_id": "u-alice", "role": "admin"}),
                ),
                store,
                kick,
            )
            .await;

            let c = conns.read().await;
            let s = c.get("conn-alice").unwrap();
            assert_eq!(s.caller_role, "operator");
            assert_eq!(
                s.permissions,
                vec!["*".to_string()],
                "a promoted member's live session must widen to the operator scope"
            );
            let guard = crate::gateway::event_scope::EventScopeGuard::default_rules();
            assert!(
                guard.can_receive("approval.requested", &s.permissions),
                "a promoted member must now be delivered approval cards"
            );
        }
    }

    /// The re-stamp must not become a back door around revocation: a
    /// connection already demoted to the login wall stays there until it
    /// re-`connect`s, even if its user is promoted in the meantime.
    #[tokio::test]
    async fn role_change_never_promotes_an_already_walled_connection() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        upsert_panel_device(&store, "dev-a1", "u-alice");
        // Already walled (its device was revoked a moment ago).
        let kick = kick_with_live_connection("conn-alice", "dev-a1", "u-alice", "guest").await;
        let conns = kick.connections.clone();

        handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "role": "admin"}),
            ),
            store,
            kick,
        )
        .await;

        let c = conns.read().await;
        assert_eq!(
            c.get("conn-alice").unwrap().caller_role,
            "guest",
            "only a fresh connect may lift the login wall"
        );
    }

    /// Another user's live connection must be untouched — the re-stamp is
    /// keyed by the updated user's own device ids, not by `caller_user`.
    #[tokio::test]
    async fn role_change_leaves_other_users_connections_alone() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        store.create_user("u-bob", "Bob", UserRole::Admin).unwrap();
        upsert_panel_device(&store, "dev-a1", "u-alice");
        upsert_panel_device(&store, "dev-b1", "u-bob");

        let kick = kick_with_live_connection("conn-bob", "dev-b1", "u-bob", "operator").await;
        let conns = kick.connections.clone();

        handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "role": "admin"}),
            ),
            store,
            kick,
        )
        .await;

        let c = conns.read().await;
        assert_eq!(c.get("conn-bob").unwrap().caller_role, "operator");
    }

    #[tokio::test]
    async fn owner_cannot_be_deactivated_or_demoted() {
        let store = seeded_store();
        for body in [
            json!({"user_id": OWNER_USER_ID, "status": "deactivated"}),
            json!({"user_id": OWNER_USER_ID, "role": "member"}),
        ] {
            let resp = handle_update(
                rpc_request("users.update", body),
                store.clone(),
                test_kick_sink(),
            )
            .await;
            assert!(response_is_error(&resp), "owner must stay an active admin");
        }
        let owner = store
            .get_user(OWNER_USER_ID)
            .unwrap()
            .expect("owner exists");
        assert_eq!(owner.status, UserStatus::Active);
        assert_eq!(owner.role, UserRole::Admin);
    }
}
