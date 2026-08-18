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
use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::gateway_devices::revoke_device_and_kick;
use super::parse_params;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::pairing_store::PairingStore;
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
    /// The same `PairingStore` Arc the inbound router and the
    /// `channel.pairing.*` handlers hold — deactivation must withdraw the
    /// channel axis from the store those two read, not from a second copy.
    pub pairing: Arc<dyn PairingStore>,
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
            // Authority-change audit (round-5 ⑦): a principal appearing is a
            // change to who can do what; it used to leave no record anywhere.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!(
                        "users.create: created {} role={}",
                        user_id,
                        role.as_str()
                    ),
                ));
            }
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

    // Read the prior state BEFORE the write: the transition detail
    // (`active→deactivated`) is what the audit entry and the receipt both
    // report, and after `update_user` commits there is nothing left to diff
    // against. A missing row here is not an error — `update_user`'s row count
    // below stays the sole not-found arbiter.
    let prior = store.get_user(&params.user_id).ok().flatten();

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

    // Authority-change audit (round-5 ⑦): role and status transitions change
    // what this principal can do; both used to commit silently.
    let audit = crate::security::audit::global();
    let actor = crate::gateway::caller_identity::current_caller_user();
    if let (Some(log), Some(new_role)) = (audit.as_ref(), role) {
        let old_role = prior.as_ref().map(|u| u.role.as_str()).unwrap_or("?");
        if old_role != new_role.as_str() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                actor.clone(),
                format!(
                    "users.update: role {} {}→{}",
                    params.user_id,
                    old_role,
                    new_role.as_str()
                ),
            ));
        }
    }

    let mut revoked_senders: Vec<Value> = Vec::new();
    let mut revoked_devices = 0usize;
    let mut freeze = FreezeReport::default();
    // The pipeline runs on EVERY deactivation write, not only on the
    // transition: each leg is best-effort, so a repeated
    // `status="deactivated"` is the operator's retry after a partial failure.
    if status == Some(UserStatus::Deactivated) {
        // …but the audit entry fires only on the transition — a retry of an
        // already-deactivated principal changes nothing and records nothing.
        if prior.as_ref().map(|u| u.status) != Some(UserStatus::Deactivated) {
            if let Some(log) = audit.as_ref() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    actor.clone(),
                    format!("users.update: status {} →deactivated", params.user_id),
                ));
            }
        }
        revoked_devices = deactivate_devices(&store, &kick, &params.user_id).await;
        revoked_senders = revoke_channel_bindings(&kick, &params.user_id).await;
        freeze = freeze_owned_background_work(&params.user_id).await;
    }
    // Reactivation is a bare store write: devices stay revoked, channel
    // senders stay withdrawn, goals/loops/crons stay paused. The receipt says
    // so — `status: "active"` alone read as if everything had come back.
    let reactivated = status == Some(UserStatus::Active)
        && prior.as_ref().map(|u| u.status) == Some(UserStatus::Deactivated);
    if reactivated {
        if let Some(log) = audit.as_ref() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                actor.clone(),
                format!("users.update: status {} deactivated→active", params.user_id),
            ));
        }
    }

    match store.get_user(&params.user_id) {
        Ok(Some(user)) => {
            // The revoked/frozen counts are named in the response because
            // they are the deactivation effects with no other surface:
            // devices show up only as closed connections, frozen background
            // work as runs that stopped happening, and a withdrawn channel
            // approval as traffic that stopped arriving.
            let mut out = json!({
                "user": UserView::from(user),
                "revoked_channel_senders": revoked_senders,
            });
            if status == Some(UserStatus::Deactivated) {
                out["revoked_devices"] = json!(revoked_devices);
                out["frozen_background_work"] = json!({
                    "goals": freeze.goals,
                    "loops": freeze.loops,
                    "crons": freeze.crons,
                });
            }
            if reactivated {
                // The write above flipped one column; everything the
                // deactivation tore down stays down. Name the recovery verbs
                // rather than implying them — "active" alone reads as if the
                // principal were whole again.
                out["reactivation_effects"] = json!({
                    "devices": "remain revoked — issue a new bootstrap ticket (`pair --user`) per device",
                    "channel_senders": "remain withdrawn — re-approve via channel.pairing.approve",
                    "background_work": "goals/loops/crons remain paused — resume per owner session (goal update status=active / loop resume / cron_manage toggle)",
                });
            }
            JsonRpcResponse::success(request.id, out)
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
///
/// Returns the number of devices actually revoked — the deactivation receipt
/// (`users.update` response) names it because a revoked device has no other
/// surface: the Panel just sees a closed connection.
async fn deactivate_devices(
    store: &Arc<SecurityStore>,
    kick: &UserDeactivationKick,
    user_id: &str,
) -> usize {
    let device_ids = match store.list_device_ids_for_user(user_id) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.update: failed to list devices for deactivation"
            );
            return 0;
        }
    };

    let total = device_ids.len();
    let mut revoked = 0usize;
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
            Ok(true) => revoked += 1,
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
    if revoked < total {
        tracing::warn!(
            user_id = %user_id,
            revoked,
            total,
            "users.update: deactivation revoked fewer devices than it listed"
        );
    }
    revoked
}

/// Withdraw every approved channel sender bound to `user_id`, returning one
/// `{channel, sender_id}` object per binding actually removed.
///
/// # Why deactivation has to reach this axis at all
///
/// A principal is bound to Aleph through two independent credentials, and
/// SECURITY.md says so: a **device** (Panel/CLI, via a bootstrap ticket) and a
/// **channel sender** (Telegram/webhook/…, via `channel.pairing.approve`).
/// Deactivation revoked the first and left the second, so an offboarded member
/// kept messaging the bot from Telegram: `inbound_router::executor` stamps
/// `ScopeAttribution::personal` from `sender_user` on every inbound turn, so
/// they kept their sessions, kept reading and writing `main__u-<them>`, kept
/// having their curated memory injected — and could call
/// `goal(action='update', status='active')` / `loop(action='resume')` to undo
/// the freeze [`freeze_owned_background_work`] had just applied. No error, no
/// failing test.
///
/// # Why removal, and not a status check inside the resolver
///
/// Teaching `sender_user` to answer `None` for a deactivated principal looks
/// like the smaller fix and is the more dangerous one: `None` on that resolver
/// does not mean "refused", it means *unlinked*, and the consumer reads
/// unlinked as legacy owner semantics — it stamps nothing and the run is
/// adopted by the operator. A walled member would have been upgraded to the
/// **owner's** scope, memory and sessions. Removal instead makes the channel
/// axis fail the way the device axis already does: the credential is gone, the
/// sender is a stranger again, and re-admission runs back through
/// `channel.pairing.approve`, which now refuses to bind onto a deactivated
/// principal.
///
/// Best-effort in the same shape as its siblings: the store write above is
/// already committed, so a failure here is logged and does not abort the rest.
async fn revoke_channel_bindings(kick: &UserDeactivationKick, user_id: &str) -> Vec<Value> {
    match kick.pairing.revoke_for_user(user_id).await {
        Ok(pairs) => {
            if !pairs.is_empty() {
                tracing::info!(
                    user_id = %user_id,
                    count = pairs.len(),
                    "users.update: deactivation revoked channel sender approvals"
                );
            }
            pairs
                .into_iter()
                .map(|(channel, sender_id)| json!({ "channel": channel, "sender_id": sender_id }))
                .collect()
        }
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.update: failed to revoke channel sender approvals during deactivation"
            );
            Vec::new()
        }
    }
}

/// What the deactivation freeze actually froze — the counts the `users.update`
/// receipt reports (round-4 ledger item ⑧: the receipt used to say only
/// `status: "deactivated"` while three subsystems changed state underneath).
#[derive(Debug, Default, Clone, Copy)]
struct FreezeReport {
    goals: usize,
    loops: usize,
    crons: usize,
}

/// Deactivation freeze (spec §10): pause every goal and loop OWNED BY
/// `user_id`, mirroring [`deactivate_devices`]'s best-effort shape (a scan
/// failure for one subsystem must not abort the other, and neither aborts
/// the already-committed store write above).
///
/// Cron used to be DELIBERATELY skipped here, on the reasoning that "`cron.*`
/// is admin-gated, so a deactivated MEMBER owns none by construction — there
/// is nothing to freeze", with the escape hatch named as "if cron creation is
/// ever opened to non-admin members".
///
/// Both halves were falsifiable from this same file. The owner guard above
/// pins only `OWNER_USER_ID`, and `users.create` accepts `role: "admin"` — so
/// a SECOND admin is fully deactivatable and certainly owns cron jobs. Their
/// jobs kept firing afterwards, correctly attributed to a walled principal:
/// rehydrating that attribution into every run
/// (`cron::executor::build_cron_metadata`), writing their memory partition,
/// delivering to their channel, indefinitely, with no surface saying so.
/// (The hatch that fired was not the one the comment watched for, which is
/// the usual way: an invariant asserted about one door while a second door
/// stood open beside it.)
///
/// One-way freeze for all three: reactivating the user does NOT auto-resume
/// its goals, loops or crons (spec is silent on auto-resume) — each owner
/// session resumes its own via `goal(action='update', status='active')` /
/// `loop(action='resume')` / `cron_manage(action='toggle')`.
async fn freeze_owned_background_work(user_id: &str) -> FreezeReport {
    let mut report = FreezeReport::default();
    if let Some(store) = crate::goal::global() {
        match store.pause_all_owned_by(user_id) {
            Ok(count) => {
                report.goals = count;
                if count > 0 {
                    tracing::warn!(
                        user_id = %user_id,
                        count,
                        "users.update: deactivation paused owned goals"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "users.update: failed to pause owned goals during deactivation"
                );
            }
        }
    }
    if let Some(registry) = crate::looping::global() {
        let count = registry.pause_all_owned_by(user_id);
        report.loops = count;
        if count > 0 {
            tracing::warn!(
                user_id = %user_id,
                count,
                "users.update: deactivation paused owned loops"
            );
        }
    }
    if let Some(cron) = crate::tasks::cron::global() {
        match cron.lock().await.pause_all_owned_by(user_id).await {
            Ok(count) => {
                report.crons = count;
                if count > 0 {
                    tracing::warn!(
                        user_id = %user_id,
                        count,
                        "users.update: deactivation disabled owned cron jobs"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    user_id = %user_id,
                    error = %e,
                    "users.update: failed to disable owned cron jobs during deactivation"
                );
            }
        }
    }
    report
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

    /// Upsert a node-namespace device (device_type absent, mirroring
    /// `admit_node`'s own backfill — `src/gateway/CLAUDE.md` mine 3) and bind
    /// it to `user_id`. This row shape should never exist in production
    /// (cluster nodes don't carry a `user_id`), but the store lookup's
    /// `device_type = 'panel'` predicate must exclude it regardless of that
    /// invariant holding.
    fn upsert_node_device(store: &SecurityStore, device_id: &str, user_id: &str) {
        store
            .upsert_device(&DeviceUpsertData {
                device_id,
                device_name: "Test Node",
                device_type: None,
                public_key: &[2u8; 32],
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
        kick_with_pairing(Arc::new(
            crate::gateway::pairing_store::SqlitePairingStore::in_memory().unwrap(),
        ))
    }

    /// [`test_kick_sink`] with a caller-supplied pairing store, so a test can
    /// seed approved senders and assert the deactivation revoke.
    fn kick_with_pairing(pairing: Arc<dyn PairingStore>) -> UserDeactivationKick {
        UserDeactivationKick {
            connections: Arc::new(RwLock::new(HashMap::new())),
            event_bus: Arc::new(GatewayEventBus::new()),
            pairing,
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

    /// A principal is bound to Aleph through two independent credentials — a
    /// device and a channel sender. Deactivation revoked the first and left the
    /// second, so an offboarded member kept talking to the bot from Telegram
    /// under their own identity: their sessions, their memory partition, their
    /// curated memory in the prompt, and `goal(action='update')` to thaw the
    /// freeze this same handler had just applied.
    ///
    /// The response is asserted too, because a withdrawn channel approval has
    /// no other surface: a closed connection is visible, a paused goal is
    /// listable, and a revoked sender is only "traffic that stopped arriving".
    #[tokio::test]
    async fn deactivate_revokes_the_users_channel_sender_approvals() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        store.create_user("u-bob", "Bob", UserRole::Member).unwrap();

        let pairing =
            Arc::new(crate::gateway::pairing_store::SqlitePairingStore::in_memory().unwrap());
        for (channel, sender, user) in [
            ("telegram", "tg-alice", "u-alice"),
            ("webhook", "wh-alice", "u-alice"),
            ("telegram", "tg-bob", "u-bob"),
        ] {
            let (code, _) = pairing
                .upsert(channel, sender, HashMap::new())
                .await
                .unwrap();
            pairing.approve(channel, &code, Some(user)).await.unwrap();
        }
        assert_eq!(
            pairing.list_approved("telegram").await.unwrap().len(),
            2,
            "fixture must seed both principals on one channel, or the negative control below is vacuous"
        );

        let resp = handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "status": "deactivated"}),
            ),
            store.clone(),
            kick_with_pairing(pairing.clone()),
        )
        .await;

        let telegram = pairing.list_approved("telegram").await.unwrap();
        assert_eq!(
            telegram.len(),
            1,
            "only Alice's binding may be withdrawn from this channel"
        );
        assert_eq!(telegram[0].sender_id, "tg-bob");
        assert!(
            pairing.list_approved("webhook").await.unwrap().is_empty(),
            "the sweep is per-principal, not per-channel — every channel Alice \
             was approved on must lose her"
        );
        assert!(
            pairing.sender_user("telegram", "tg-alice").await.is_none(),
            "the resolver the inbound router reads must no longer know her"
        );

        let v = response_json(&resp);
        let revoked = v["revoked_channel_senders"]
            .as_array()
            .expect("the response names what it withdrew");
        assert_eq!(revoked.len(), 2);
        let mut pairs: Vec<(String, String)> = revoked
            .iter()
            .map(|r| {
                (
                    r["channel"].as_str().unwrap().to_string(),
                    r["sender_id"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("telegram".to_string(), "tg-alice".to_string()),
                ("webhook".to_string(), "wh-alice".to_string()),
            ]
        );
    }

    /// The counterpart nobody would notice was missing: a role change or a
    /// display-name edit must not touch the channel axis. The sweep is bound to
    /// `status == Deactivated`, not to "this handler ran".
    #[tokio::test]
    async fn a_non_deactivating_update_leaves_channel_bindings_alone() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let pairing =
            Arc::new(crate::gateway::pairing_store::SqlitePairingStore::in_memory().unwrap());
        let (code, _) = pairing
            .upsert("telegram", "tg-alice", HashMap::new())
            .await
            .unwrap();
        pairing
            .approve("telegram", &code, Some("u-alice"))
            .await
            .unwrap();

        handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "role": "admin"}),
            ),
            store.clone(),
            kick_with_pairing(pairing.clone()),
        )
        .await;

        assert_eq!(
            pairing.list_approved("telegram").await.unwrap().len(),
            1,
            "a promotion must not withdraw a channel credential"
        );
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

    /// Spec §10 end-to-end: deactivating a user must freeze its OWNED goals
    /// and loops, and leave another user's untouched. Session ids are unique
    /// to this test to stay correct even if another test in the same binary
    /// already installed the process-global registries (mirrors
    /// `db_handlers::modify::delete_stops_the_sessions_timer_loop`'s
    /// `unwrap_or_else` idiom for exactly that reason).
    #[tokio::test]
    async fn deactivate_pauses_owned_goals_and_loops() {
        let goal_store = crate::goal::global().unwrap_or_else(|| {
            let temp = tempfile::tempdir().unwrap();
            crate::goal::set_global_for_test(Arc::new(
                crate::goal::GoalStore::open(&temp.path().join("g.db")).unwrap(),
            ));
            std::mem::forget(temp); // keep the sqlite file alive for the process
            crate::goal::global().expect("goal store installed")
        });
        let loop_registry = crate::looping::global().unwrap_or_else(|| {
            crate::looping::init_global(Arc::new(crate::looping::LoopRegistry::default()));
            crate::looping::global().expect("loop registry installed")
        });

        let alice_attr = crate::scope::ScopeAttribution::personal("u-alice-p0t5");
        let bob_attr = crate::scope::ScopeAttribution::personal("u-bob-p0t5");
        goal_store
            .put(
                &crate::goal::Goal::new("agent:g-alice-p0t5:main", "obj", 0, 0)
                    .with_pursuit(crate::goal::PursuitMode::Active { max_iterations: 5 })
                    .with_owner_scope(Some(&alice_attr)),
            )
            .unwrap();
        goal_store
            .put(
                &crate::goal::Goal::new("agent:g-bob-p0t5:main", "obj", 0, 0)
                    .with_pursuit(crate::goal::PursuitMode::Active { max_iterations: 5 })
                    .with_owner_scope(Some(&bob_attr)),
            )
            .unwrap();
        loop_registry.put(
            crate::looping::LoopState::new(
                "agent:l-alice-p0t5:main",
                "watch",
                crate::looping::Cadence::Fixed {
                    interval_ms: 300_000,
                },
                0,
            )
            .with_owner_scope(Some(&alice_attr)),
        );
        loop_registry.put(
            crate::looping::LoopState::new(
                "agent:l-bob-p0t5:main",
                "watch",
                crate::looping::Cadence::Fixed {
                    interval_ms: 300_000,
                },
                0,
            )
            .with_owner_scope(Some(&bob_attr)),
        );

        let store = seeded_store();
        store
            .create_user("u-alice-p0t5", "Alice", UserRole::Member)
            .unwrap();
        let req = rpc_request(
            "users.update",
            json!({"user_id": "u-alice-p0t5", "status": "deactivated"}),
        );
        handle_update(req, store, test_kick_sink()).await;

        assert_eq!(
            goal_store
                .get("agent:g-alice-p0t5:main")
                .unwrap()
                .unwrap()
                .status,
            crate::goal::GoalStatus::Paused,
            "alice's goal must be paused"
        );
        assert_eq!(
            goal_store
                .get("agent:g-bob-p0t5:main")
                .unwrap()
                .unwrap()
                .status,
            crate::goal::GoalStatus::Active,
            "bob's goal must be untouched"
        );
        assert_eq!(
            loop_registry.get("agent:l-alice-p0t5:main").unwrap().status,
            crate::looping::LoopStatus::Paused,
            "alice's loop must be paused"
        );
        assert_eq!(
            loop_registry.get("agent:l-bob-p0t5:main").unwrap().status,
            crate::looping::LoopStatus::Active,
            "bob's loop must be untouched"
        );
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
            // admin keeps receiving admin-guarded traffic on the tab he
            // already has open.
            assert!(
                s.permissions.is_empty(),
                "a demoted admin's live session must lose the `*` event scope"
            );
            let guard = crate::gateway::event_scope::EventScopeGuard::default_rules();
            assert!(
                !crate::gateway::event_scope::is_superuser_scope(&s.permissions),
                "a demoted admin must no longer satisfy the admin arm that \
                 delivers OTHER users' approval cards — the raw `approval.*` \
                 topics are owner-scoped now (`BySessionKeyOrAdmin`), so this \
                 is the predicate that used to be `can_receive(approval.…)`. \
                 He keeps his own cards, which is correct: he is still someone \
                 whose tool calls park."
            );
            // The R5 BANNER joined them on 2026-08-09 — it carries a
            // `session_key` now, so the demotion that matters for it is the
            // `is_superuser_scope` assertion above, not a prefix lookup. Pinned
            // as a positive here so that "this guard is silent about the
            // banner" is a stated fact rather than a deletion nobody notices.
            assert!(
                guard.can_receive("surface.approval", &s.permissions),
                "the prefix table must be silent about the banner — the owner \
                 check is what demotes him, and it is asserted above"
            );
            assert!(
                !guard.can_receive("pty.output", &s.permissions),
                "a demoted admin must no longer be delivered the operator's shell"
            );
            // The three `approval.*` FRAMES moved off this table on 2026-08-08
            // (see `event_scope`'s own pin): they are gated per payload, so the
            // demotion that matters for them is the one below, on the
            // `caller_user` this connection now carries.
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

    /// P1 hardening (Task 9): the `devices` table is the shared panel/node
    /// namespace (`src/gateway/CLAUDE.md` mine 3). The restamp loop's store
    /// lookup must consider only `device_type = 'panel'` rows, so a
    /// node-namespace row that happens to carry a matching `user_id`/
    /// `device_id` (never true in production today — nodes are enrolled via
    /// `admit_node`, unrelated to the `users` table — but the predicate must
    /// hold regardless of that invariant) can never restamp a live
    /// connection.
    #[tokio::test]
    async fn restamp_skips_a_node_namespace_device_row_with_a_matching_id() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        upsert_node_device(&store, "dev-node-1", "u-alice");

        let kick = kick_with_live_connection("conn-alice", "dev-node-1", "u-alice", "member").await;
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
            "member",
            "a node-namespace device row must never restamp a live connection"
        );
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

    /// The deactivation receipt (round-5 ⑧): `users.update` must NAME what it
    /// tore down — devices revoked, background work frozen — because none of
    /// those effects has any other surface.
    #[tokio::test]
    async fn deactivation_receipt_reports_devices_and_frozen_work() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        upsert_panel_device(&store, "dev-a1", "u-alice");
        upsert_panel_device(&store, "dev-a2", "u-alice");

        let resp = handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "status": "deactivated"}),
            ),
            store.clone(),
            test_kick_sink(),
        )
        .await;

        let v = response_json(&resp);
        assert_eq!(v["revoked_devices"], json!(2));
        let frozen = &v["frozen_background_work"];
        for key in ["goals", "loops", "crons"] {
            assert!(
                frozen.get(key).is_some(),
                "the receipt must report the {key} count even when it is zero"
            );
        }
        assert!(
            v.get("reactivation_effects").is_none(),
            "a deactivation receipt must not carry reactivation guidance"
        );
    }

    /// The reactivation receipt (round-5 ⑧): flipping `status` back is a bare
    /// store write — devices stay revoked, channel senders stay withdrawn,
    /// background work stays paused. The response must say so, because
    /// `status: "active"` alone reads as if the principal were whole again.
    #[tokio::test]
    async fn reactivation_receipt_names_what_stays_down() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();

        handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "status": "deactivated"}),
            ),
            store.clone(),
            test_kick_sink(),
        )
        .await;
        let resp = handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "status": "active"}),
            ),
            store.clone(),
            test_kick_sink(),
        )
        .await;

        let v = response_json(&resp);
        let effects = v
            .get("reactivation_effects")
            .expect("reactivation must name what did NOT come back");
        for key in ["devices", "channel_senders", "background_work"] {
            assert!(
                effects[key].as_str().is_some_and(|s| !s.is_empty()),
                "reactivation_effects.{key} must name the recovery path"
            );
        }
        assert!(
            v.get("revoked_devices").is_none(),
            "a reactivation did not revoke anything — the deactivation fields must not appear"
        );
    }

    /// Authority-change audit (round-5 ⑦): create / role transition /
    /// deactivation each append exactly one `authority_change` entry naming
    /// actor, verb and target. Serialised under `AUDIT_TEST_LOCK` because the
    /// handle is process-global.
    #[tokio::test]
    async fn authority_changes_are_audited() {
        let _serial = crate::security::audit::AUDIT_TEST_LOCK.lock().unwrap();
        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(16);
        crate::security::audit::replace_global_for_test(&log);

        let store = seeded_store();
        let created = crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-owner".to_string()),
                handle_create(
                    rpc_request("users.create", json!({"display_name": "Alice"})),
                    store.clone(),
                ),
            )
            .await;
        let new_id = response_json(&created)["user"]["user_id"]
            .as_str()
            .unwrap()
            .to_string();

        crate::gateway::caller_identity::CALLER_USER
            .scope(
                Some("u-owner".to_string()),
                handle_update(
                    rpc_request(
                        "users.update",
                        json!({"user_id": new_id, "role": "admin", "status": "deactivated"}),
                    ),
                    store.clone(),
                    test_kick_sink(),
                ),
            )
            .await;

        let mut details = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            assert_eq!(
                entry.event_type,
                crate::security::audit::AuditEventType::AuthorityChange
            );
            details.push((entry.actor_user, entry.detail));
        }
        crate::security::audit::clear_global_for_test();

        // Concurrent tests in this process may legitimately emit their own
        // entries into the installed handle while it is up (the lock
        // serialises audit-ASSERTING tests, not every producer) — keep only
        // this test's principal, whose id is a fresh uuid, and only those
        // entries carry this test's scoped caller.
        let mine: Vec<String> = details
            .into_iter()
            .filter(|(_, d)| d.contains(&new_id))
            .map(|(actor, d)| {
                assert_eq!(actor.as_deref(), Some("u-owner"));
                d
            })
            .collect();
        assert_eq!(mine.len(), 3, "create + role + status: {mine:?}");
        assert!(mine[0].starts_with("users.create: created u-"));
        assert!(mine[0].contains("role=member"));
        assert_eq!(mine[1], format!("users.update: role {new_id} member→admin"));
        assert_eq!(
            mine[2],
            format!("users.update: status {new_id} →deactivated")
        );
    }
}
