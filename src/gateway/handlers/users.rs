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

use serde_json::Value;
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
///
/// The shape is [`aleph_protocol::users::UserView`], shared with the CLI and
/// the Panel rather than declared three times. `From<UserRecord>` cannot be
/// implemented here (both types would be foreign to this crate's orphan rule
/// on the protocol side, and the record is a `alephcore` type), so the
/// conversion is a free function used everywhere a record becomes a view.
pub use aleph_protocol::users::UserView;

/// Serialise a contract type into a success response.
///
/// Every `users.*` response goes through here, so the wire shape is whatever
/// the shared type says it is. Building responses this way (rather than with a
/// `json!` literal beside the type) is what makes over-sending a compile-time
/// impossibility: `workspace.get` shipped four fields with no reader and no
/// writer anywhere precisely because a literal can carry keys the contract
/// never mentions and still parse.
fn encoded<T: serde::Serialize>(id: Option<Value>, value: &T) -> JsonRpcResponse {
    match serde_json::to_value(value) {
        Ok(v) => JsonRpcResponse::success(id, v),
        Err(e) => JsonRpcResponse::error(id, INTERNAL_ERROR, format!("failed to encode: {e}")),
    }
}

/// The one place a stored record becomes the wire shape.
pub(crate) fn user_view(u: UserRecord) -> UserView {
    UserView {
        user_id: u.user_id,
        display_name: u.display_name,
        role: u.role.as_str().to_string(),
        status: u.status.as_str().to_string(),
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
        return encoded(
            request.id,
            &aleph_protocol::users::UserMeResult { user: None },
        );
    };

    match store.get_user(&caller_id) {
        Ok(user) => {
            let result = aleph_protocol::users::UserMeResult {
                user: user.map(user_view),
            };
            encoded(request.id, &result)
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
            let result = aleph_protocol::users::UserListResult {
                users: users.into_iter().map(user_view).collect(),
            };
            encoded(request.id, &result)
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("failed to list users: {e}"),
        ),
    }
}

// ============================================================================
// users.get
// ============================================================================

/// The stores `users.get` composes its answer from, beyond the principal
/// registry itself.
///
/// `projects` is a parameter and not `ProjectStore::shared()` read inside the
/// handler for the reason the rest of this module already follows: the
/// registration site owns which store a handler talks to, so a test can hand
/// it a fresh one instead of the process's.
#[derive(Clone)]
pub struct UserDetailContext {
    pub projects: Arc<crate::projects::ProjectStore>,
}

/// `users.get { user_id }` → [`aleph_protocol::users::UserDetail`].
///
/// # Why this method exists
///
/// The one place a principal's devices, spend and frozen background work were
/// ever joined was [`handle_update`]'s deactivation receipt — i.e. AFTER the
/// irreversible status write, which that code's own comment concedes ("the
/// deactivation effects with no other surface"). Criterion #15: the join
/// existed only as the receipt of a one-way door. This is the same join, read
/// **before** the door.
///
/// # Authorization
///
/// Nothing here re-checks the caller's role, and nothing here is carved out.
/// `method_admin.rs` already gates the whole `users.` prefix, and its
/// member carve-outs are `users.me` and `users.list` only — a dossier over
/// somebody else's holdings is not member-safe, so it must NOT join them.
/// Per OI-63 `users.*` stays CLI-only over loopback; there is no Panel face.
///
/// # Deliberately out of scope
///
/// Sessions and transcripts. An admin arm on `gateway::visibility` is a real
/// authorization change that needs its own ruling (there is no `Role::Admin`
/// / `is_admin` / `caller_role` reference in that module today), and
/// transcripts stay behind `trace.*`. This method does not widen
/// `stamped_owner_visible` / `ambient_owner_visible` either (ruling OI-2):
/// it composes reads an admin already has, it does not grant a member sight
/// of anything.
///
/// # Fail-closed reads
///
/// A ledger that cannot be read fails the whole request rather than rendering
/// as an empty wallet — the same refusal `spend.query` makes, for the same
/// reason (criterion #8). An absent ledger ROW is different and is reported
/// as `spend: None`, which the CLI prints as "no spend recorded".
pub async fn handle_get(
    request: JsonRpcRequest,
    store: Arc<SecurityStore>,
    ctx: UserDetailContext,
) -> JsonRpcResponse {
    let params: aleph_protocol::users::UserGetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let user = match store.get_user(&params.user_id) {
        Ok(Some(u)) => user_view(u),
        Ok(None) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("user not found: {}", params.user_id),
            )
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to read user: {e}"),
            )
        }
    };

    // "Live panel devices" is exactly this query's filter — `revoked_at IS
    // NULL AND device_type = 'panel'` — and the contract field's doc says so
    // rather than letting the reader guess a wider meaning. A failure to
    // enumerate must not render as "they hold none".
    let live_panel_devices = match store.list_device_ids_for_user(&params.user_id) {
        Ok(ids) => ids.len(),
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to count live devices: {e}"),
            )
        }
    };

    let room_ids = match ctx.projects.room_ids_for_member(&params.user_id) {
        Ok(ids) => ids,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to read room memberships: {e}"),
            )
        }
    };

    let spend = match spend_for_principal(&params.user_id) {
        Ok(row) => row,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("failed to read spend ledger: {e}"),
            )
        }
    };

    let detail = aleph_protocol::users::UserDetail {
        user,
        live_panel_devices,
        room_ids,
        spend,
        background_work: count_owned_background_work(&params.user_id).await,
    };
    encoded(request.id, &detail)
}

/// This principal's row in the spend period that is open right now.
///
/// The existing point lookup (`SpendLedger::spent_for`), not a new store
/// method: the ledger already answers "what has this principal spent in this
/// window", and adding a second way to ask would give one fact two authors.
///
/// `Ok(None)` means **no row** — `spent_for` folds an absent row into a
/// zeroed `Spent`, so an all-zero answer is the only signal the ledger can
/// give for "nothing recorded", and the caller must render it as that rather
/// than as `0.00`. The one thing it cannot distinguish is a genuinely
/// recorded `$0.00` complete call, which reads here as "no spend recorded";
/// that is the ledger's resolution, not a choice this function makes.
///
/// `Err` is never folded into `None`: "I could not read the ledger" and
/// "they spent nothing" are the two answers criterion #8 exists to keep
/// apart, so the error travels to the caller and fails the request.
fn spend_for_principal(user_id: &str) -> anyhow::Result<Option<aleph_protocol::spend::SpendRow>> {
    let policy = crate::spend::current_policy();
    let ledger = crate::spend::global_ledger();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let period_start_ms = crate::spend::period::period_start_ms(now_ms, policy.period);
    let principal = crate::spend::Principal::user(user_id);
    let spent = ledger.spent_for(&principal, period_start_ms)?;
    if spent.usd == 0.0 && spent.unpriced_calls == 0 && spent.partial_calls == 0 {
        return Ok(None);
    }
    Ok(Some(aleph_protocol::spend::SpendRow {
        principal: principal.as_key().to_string(),
        usd: spent.usd,
        unpriced_calls: spent.unpriced_calls,
        partial_calls: spent.partial_calls,
    }))
}

// ============================================================================
// users.create
// ============================================================================

/// [`aleph_protocol::users::UserCreateParams`] — shared with `aleph users create`
/// so a renamed field is a compile error rather than a runtime
/// `INVALID_PARAMS` nobody's tests can see.
pub use aleph_protocol::users::UserCreateParams as CreateParams;

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
                    format!("users.create: created {} role={}", user_id, role.as_str()),
                ));
            }
            let result = aleph_protocol::users::UserCreateResult {
                user: UserView {
                    user_id,
                    display_name: params.display_name,
                    role: role.as_str().to_string(),
                    status: UserStatus::Active.as_str().to_string(),
                },
            };
            encoded(request.id, &result)
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

/// [`aleph_protocol::users::UserUpdateParams`] — shared with `aleph users update`.
pub use aleph_protocol::users::UserUpdateParams as UpdateParams;

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
/// the status write already succeeded) **and** burns the principal's
/// outstanding bootstrap tickets via [`burn_outstanding_bootstrap_tickets`],
/// so a ticket minted before the sweep cannot pair a brand-new device after
/// it.
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

    let mut revoked_senders: Vec<aleph_protocol::users::RevokedChannelSender> = Vec::new();
    let mut revoked_devices = 0usize;
    let mut revoked_tickets = 0usize;
    let mut freeze = aleph_protocol::users::FrozenBackgroundWork::default();
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
        revoked_tickets =
            burn_outstanding_bootstrap_tickets(&store, &params.user_id, actor.clone());
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
            let result = aleph_protocol::users::UserUpdateResult {
                user: user_view(user),
                revoked_channel_senders: revoked_senders,
                revoked_devices: (status == Some(UserStatus::Deactivated))
                    .then_some(revoked_devices),
                revoked_bootstrap_tickets: (status == Some(UserStatus::Deactivated))
                    .then_some(revoked_tickets),
                // Forwarded whole: the freeze already builds the contract
                // type, so there is no field-by-field copy here to forget a
                // leg in. `heartbeats` stays an `Option` all the way to the
                // wire — a measured zero and an unmeasured leg are different
                // answers to the operator.
                frozen_background_work: (status == Some(UserStatus::Deactivated))
                    .then_some(freeze),
                // The write above flipped one column; everything the
                // deactivation tore down stays down. Name the recovery verbs
                // rather than implying them — "active" alone reads as if the
                // principal were whole again.
                reactivation_effects: reactivated.then(|| {
                    aleph_protocol::users::ReactivationEffects {
                        devices: "remain revoked — issue a new bootstrap ticket (`pair --user`) per device".to_string(),
                        channel_senders: "remain withdrawn — re-approve via channel.pairing.approve".to_string(),
                        background_work: "goals/loops/crons/heartbeat tasks remain paused — resume per owner session (goal update status=active / loop resume / cron_manage toggle / heartbeat_toggle)".to_string(),
                    }
                }),
            };
            encoded(request.id, &result)
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

/// Burn every outstanding bootstrap ticket minted for `user_id` — the fourth
/// leg of the deactivation sweep. Returns how many live tickets were cut.
///
/// # Why the other three legs are not enough
///
/// The three legs above cut credentials that already **exist**. A bootstrap
/// ticket is a credential that has not been redeemed yet, and
/// `DeviceTokenManager::exchange_bootstrap_ticket` performs no user-status
/// check — both status guards sit at MINT time (`gateway_ticket.rs`,
/// `pair.rs`). So mint → deactivate → redeem is two legal steps that produce a
/// fresh, non-revoked device row **after** the sweep has run, with a ten-year
/// token; `connect` then walls every frame it sends to `(None, "guest")`.
/// That is precisely the "pairs successfully and then refuses everything"
/// state the mint-time guards exist to prevent, reached by walking around
/// them, and it made the reactivation receipt's "devices remain revoked —
/// issue a new bootstrap ticket" a false sentence about a device whose token
/// worked.
///
/// Best-effort, the same shape as the legs beside it: a store failure is
/// logged and reported as zero rather than failing the whole write, because
/// the operator's next move after a partial deactivation is to retry the same
/// call.
///
/// One `AuthorityChange` entry, **only when something was actually cut** —
/// `gateway_devices.rs`'s stated rule for the same reason: a retry against an
/// already-deactivated principal changes nothing, so there is no decision to
/// record. The entry names the count and the principal and never the ticket
/// codes: a bootstrap ticket is a bearer credential and this module exists to
/// keep bearer credentials out of logs.
fn burn_outstanding_bootstrap_tickets(
    store: &Arc<SecurityStore>,
    user_id: &str,
    actor: Option<String>,
) -> usize {
    let burned = match store.revoke_bootstrap_tickets_for_user(user_id) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.update: failed to burn outstanding bootstrap tickets during deactivation — \
                 an unredeemed ticket for this principal may still pair a new device"
            );
            return 0;
        }
    };

    if burned > 0 {
        tracing::warn!(
            user_id = %user_id,
            burned,
            "users.update: deactivation burned outstanding bootstrap tickets"
        );
        if let Some(log) = crate::security::audit::global() {
            log.log(crate::security::audit::AuditEntry::authority_change(
                actor,
                format!("users.update: burned {burned} bootstrap ticket(s) for {user_id}"),
            ));
        }
    }
    burned
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
async fn revoke_channel_bindings(
    kick: &UserDeactivationKick,
    user_id: &str,
) -> Vec<aleph_protocol::users::RevokedChannelSender> {
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
                .map(
                    |(channel, sender_id)| aleph_protocol::users::RevokedChannelSender {
                        channel,
                        sender_id,
                    },
                )
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

/// The four background subsystems both the deactivation freeze and the
/// `users.get` preview reach into.
///
/// Taken as a parameter rather than read from the four process-globals at the
/// point of use, for the reason [`freeze_owned_heartbeats`] already documents
/// for its own leg: a process-global is install-once, so a test that installs
/// one can never afterwards observe the absent path in the same binary — and
/// the interesting arms here are exactly the absent ones. It is also what
/// lets one test assert the freeze's numbers and the preview's numbers over
/// the SAME four stores.
#[derive(Clone, Default)]
pub(crate) struct BackgroundWorkHandles {
    pub goals: Option<Arc<crate::goal::GoalStore>>,
    pub loops: Option<Arc<crate::looping::LoopRegistry>>,
    pub crons: Option<crate::tasks::cron::SharedCronService>,
    pub heartbeats: Option<crate::tasks::heartbeat::SharedHeartbeatService>,
}

impl BackgroundWorkHandles {
    /// The production wiring: whatever this process actually installed.
    fn from_globals() -> Self {
        Self {
            goals: crate::goal::global(),
            loops: crate::looping::global(),
            crons: crate::tasks::cron::global(),
            heartbeats: crate::tasks::heartbeat::global(),
        }
    }
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
/// Heartbeat was the FOURTH leg and it arrived a round late, by exactly the
/// same argument the cron paragraph above makes and for a subsystem gated on
/// exactly the same terms: `heartbeat.*` is admin-gated, `users.create`
/// accepts `role: "admin"`, so a second admin owns heartbeat tasks and those
/// tasks kept firing — and kept delivering through `delivery_config` — after
/// this function returned. It was not even excluded on purpose; there simply
/// was no `heartbeat::global()` for a free function to call, so nothing in
/// the receipt or in this doc ever mentioned the subsystem. The old wording
/// here ("the three background legs") and the old `FrozenBackgroundWork` doc
/// said the same thing in two places, which is why the miss survived: an
/// inventory that names three of four reads as coverage.
///
/// One-way freeze for all four: reactivating the user does NOT auto-resume
/// its goals, loops, crons or heartbeat tasks (spec is silent on auto-resume)
/// — each owner session resumes its own via
/// `goal(action='update', status='active')` / `loop(action='resume')` /
/// `cron_manage(action='toggle')` / `heartbeat_toggle`.
///
/// **Deliberately deferred, recorded rather than hidden:** cron's fire-time
/// backstop (`CronService::disable_walled_owner_job`, driven by the
/// executor's `walled_owner_reason` check) has NO heartbeat counterpart after
/// this change. The twin is not closed — a heartbeat task re-enabled by a
/// second admin after its owner was walled will still fire, exactly as a cron
/// job would have before round-5 ④.
///
/// # Why this reports the SAME struct `users.get` reports
///
/// It used to build a private `FreezeReport` with the identical four fields
/// beside `aleph_protocol::users::FrozenBackgroundWork` — one fact, two
/// declarations, and the heartbeat leg had to be added to both. There is one
/// leg enumeration now, and the read-only preview
/// ([`count_owned_background_work`]) fills the same struct, so a fifth leg
/// cannot land on one surface and not the other.
///
/// The two surfaces still report **different numbers**, on purpose: this one
/// counts what the sweep CHANGED (`enabled && owned`), the preview counts what
/// the principal OWNS. See [`aleph_protocol::users::FrozenBackgroundWork`].
async fn freeze_owned_background_work(user_id: &str) -> aleph_protocol::users::FrozenBackgroundWork {
    freeze_owned_background_work_with(&BackgroundWorkHandles::from_globals(), user_id).await
}

/// The injectable core [`freeze_owned_background_work`] delegates to — see
/// [`BackgroundWorkHandles`] for why the four stores are parameters.
async fn freeze_owned_background_work_with(
    handles: &BackgroundWorkHandles,
    user_id: &str,
) -> aleph_protocol::users::FrozenBackgroundWork {
    let mut report = aleph_protocol::users::FrozenBackgroundWork::default();
    if let Some(store) = handles.goals.as_ref() {
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
    if let Some(registry) = handles.loops.as_ref() {
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
    if let Some(cron) = handles.crons.as_ref() {
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
    report.heartbeats = freeze_owned_heartbeats(handles.heartbeats.clone(), user_id).await;
    report
}

/// What one principal OWNS across the same four background subsystems — the
/// read-only counterpart of [`freeze_owned_background_work`], and the
/// `background_work` leg of `users.get`'s dossier.
///
/// # Why this exists at all
///
/// `pause_all_owned_by` existed as a MUTATOR on all four subsystems and had
/// no read-only counterpart anywhere. The only way to learn what a principal
/// owned was to deactivate them and read the receipt — criterion #15: the
/// join existed only as the receipt of a one-way door.
///
/// # The deliberate difference from the freeze
///
/// Same owner predicate ([`aleph_protocol::users::owned_by`], including its
/// legacy-`None` handling, shared with all four sweeps), a deliberately
/// different activity filter. The freeze counts what it CHANGED
/// (`enabled && owned`); this counts what is OWNED. A read that reused the
/// freeze's `enabled` filter would silently under-report the preview — the
/// paused goal and the disabled cron job the operator is about to strand
/// would not appear — and a freeze that reused this filter would over-report
/// what it actually stopped. **Neither surface asserts they are equal.**
///
/// The `heartbeats` leg keeps the freeze's fail-closed shape: `None` when the
/// service is not running in this process, never `Some(0)`, because "the
/// subsystem was not reachable" and "they own none" are different answers
/// (criterion #8). Goals is the one leg that can fail on its own (a SQLite
/// read); a failure there is logged and reported as `0`, matching the freeze
/// leg-by-leg — noted, not hidden, in the CLI's rendering, which says the
/// count is of what was reachable.
pub(crate) async fn count_owned_background_work(
    user_id: &str,
) -> aleph_protocol::users::FrozenBackgroundWork {
    count_owned_background_work_with(&BackgroundWorkHandles::from_globals(), user_id).await
}

/// The injectable core [`count_owned_background_work`] delegates to.
async fn count_owned_background_work_with(
    handles: &BackgroundWorkHandles,
    user_id: &str,
) -> aleph_protocol::users::FrozenBackgroundWork {
    let mut report = aleph_protocol::users::FrozenBackgroundWork::default();
    if let Some(store) = handles.goals.as_ref() {
        match store.count_owned_by(user_id) {
            Ok(count) => report.goals = count,
            Err(e) => tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.get: failed to count owned goals"
            ),
        }
    }
    if let Some(registry) = handles.loops.as_ref() {
        report.loops = registry.count_owned_by(user_id);
    }
    if let Some(cron) = handles.crons.as_ref() {
        report.crons = cron.lock().await.count_owned_by(user_id).await;
    }
    // Fail-closed, same as the freeze leg: an unreachable heartbeat service
    // says nothing rather than `0`, so the dossier cannot claim the principal
    // owns no heartbeat task when nobody looked.
    report.heartbeats = match handles.heartbeats.as_ref() {
        Some(service) => Some(service.lock().await.count_owned_by(user_id).await),
        None => {
            tracing::warn!(
                user_id = %user_id,
                "users.get: the heartbeat leg was NOT measured — no heartbeat \
                 service in this process"
            );
            None
        }
    };
    report
}

/// The heartbeat leg of [`freeze_owned_background_work`].
///
/// The service is taken as a PARAMETER rather than read from the global here,
/// because the interesting arm is the one where it is absent: a process-global
/// is install-once, so a test that installs a heartbeat service can never
/// afterwards observe the declined path in the same binary. Passing it in is
/// what makes "the service was not running" a testable outcome instead of a
/// branch nothing can reach twice.
///
/// Every non-answer returns `None`, never `Some(0)` (criterion #8: a
/// fail-closed answer is only allowed to say "I do not know"):
///
/// - no service — `[heartbeat] enabled = false`, a data directory that would
///   not resolve, or a store that would not open. Boot records WHICH of those
///   it was on the capability slot, and `aleph doctor`'s
///   `core/capability-wiring` check quotes that sentence verbatim; this
///   function only knows that the leg did not run.
/// - the sweep itself failed — the tasks may or may not have been disabled,
///   and reporting the count it did not get would be worse than reporting
///   nothing.
///
/// `Some(0)` therefore means one thing only: the sweep ran and this principal
/// owned no enabled heartbeat task.
async fn freeze_owned_heartbeats(
    service: Option<crate::tasks::heartbeat::SharedHeartbeatService>,
    user_id: &str,
) -> Option<usize> {
    let Some(service) = service else {
        tracing::warn!(
            user_id = %user_id,
            "users.update: deactivation did NOT reach the heartbeat leg — no \
             heartbeat service in this process, so any heartbeat task this \
             principal owns is still armed"
        );
        return None;
    };
    let swept = service.lock().await.pause_all_owned_by(user_id).await;
    match swept {
        Ok(count) => {
            if count > 0 {
                tracing::warn!(
                    user_id = %user_id,
                    count,
                    "users.update: deactivation disabled owned heartbeat tasks"
                );
            }
            Some(count)
        }
        Err(e) => {
            tracing::warn!(
                user_id = %user_id,
                error = %e,
                "users.update: failed to disable owned heartbeat tasks during deactivation"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::DeviceUpsertData;
    use serde_json::json;

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
            let mut state = ConnectionState::new(std::net::IpAddr::from([203, 0, 113, 7]), false);
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

    /// Build a heartbeat service holding one enabled task per `(id, owner)`
    /// pair. `None` as the owner seeds a legacy, pre-P1 row.
    async fn heartbeat_service_with(
        tasks: &[(&str, Option<&str>)],
    ) -> crate::tasks::heartbeat::SharedHeartbeatService {
        use crate::tasks::heartbeat::config::{
            HeartbeatConfig, HeartbeatTask, ProbeConfig, TriggerCondition,
        };
        let store = crate::tasks::heartbeat::store::HeartbeatStore::open_in_memory().unwrap();
        let service =
            crate::tasks::heartbeat::HeartbeatService::new(store, HeartbeatConfig::default());
        let clock = crate::tasks::shared::clock::testing::FakeClock::new(1_000_000);
        for (id, owner) in tasks {
            let mut task = HeartbeatTask::new(
                (*id).to_string(),
                "main".to_string(),
                60_000,
                ProbeConfig {
                    tool_name: "test.probe".to_string(),
                    tool_params: None,
                    trigger_condition: TriggerCondition::Always,
                },
            );
            task.id = (*id).to_string();
            task.owner_user_id = owner.map(std::string::ToString::to_string);
            service.add_task(task, &clock).await.unwrap();
        }
        Arc::new(tokio::sync::Mutex::new(service))
    }

    /// The fourth leg, end to end: deactivating a SECOND ADMIN who owns an
    /// enabled heartbeat task must disable that task — asserted by reading the
    /// store back, not by trusting the count — and the receipt must report it.
    ///
    /// A second admin is the population that makes this reachable at all:
    /// `heartbeat.*` is admin-gated, `users.create` accepts `role: "admin"`,
    /// and the owner guard in this file pins only `OWNER_USER_ID`.
    #[tokio::test]
    async fn deactivating_a_second_admin_disables_their_heartbeat_tasks() {
        // Install-once, so the whole binary shares whichever service gets here
        // first; the ids below are unique to this test so the assertions stay
        // correct either way (mirrors `deactivate_pauses_owned_goals_and_loops`).
        if crate::tasks::heartbeat::global().is_none() {
            crate::tasks::heartbeat::init_global(heartbeat_service_with(&[]).await);
        }
        let heartbeat = crate::tasks::heartbeat::global().expect("heartbeat service installed");
        {
            use crate::tasks::heartbeat::config::{HeartbeatTask, ProbeConfig, TriggerCondition};
            let clock = crate::tasks::shared::clock::testing::FakeClock::new(1_000_000);
            let svc = heartbeat.lock().await;
            for (id, owner) in [
                ("hb-admin2-p0t9", Some("u-admin2-p0t9")),
                ("hb-other-p0t9", Some("u-other-p0t9")),
                ("hb-legacy-p0t9", None),
            ] {
                let mut task = HeartbeatTask::new(
                    id.to_string(),
                    "main".to_string(),
                    60_000,
                    ProbeConfig {
                        tool_name: "test.probe".to_string(),
                        tool_params: None,
                        trigger_condition: TriggerCondition::Always,
                    },
                );
                task.id = id.to_string();
                task.owner_user_id = owner.map(str::to_string);
                svc.add_task(task, &clock).await.unwrap();
            }
        }

        let store = seeded_store();
        store
            .create_user("u-admin2-p0t9", "Second Admin", UserRole::Admin)
            .unwrap();
        let resp = handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-admin2-p0t9", "status": "deactivated"}),
            ),
            store,
            test_kick_sink(),
        )
        .await;

        let svc = heartbeat.lock().await;
        assert_eq!(
            svc.get_task("hb-admin2-p0t9").await.map(|t| t.enabled),
            Some(false),
            "the deactivated admin's heartbeat task must be DISABLED in the store"
        );
        assert_eq!(
            svc.get_task("hb-other-p0t9").await.map(|t| t.enabled),
            Some(true),
            "another principal's heartbeat task must be untouched"
        );
        assert_eq!(
            svc.get_task("hb-legacy-p0t9").await.map(|t| t.enabled),
            Some(true),
            "a legacy task with no owner belongs to nobody and must survive"
        );
        drop(svc);

        assert_eq!(
            response_json(&resp)["frozen_background_work"]["heartbeats"],
            json!(1),
            "the receipt must count the heartbeat task it froze"
        );
    }

    /// The declined path (criterion #8): with no heartbeat service in the
    /// process, the leg reports "I do not know", never a zero. A zero here
    /// would read to an operator as "they owned no heartbeat tasks" while
    /// every one of them stayed armed.
    #[tokio::test]
    async fn a_missing_heartbeat_service_reports_unmeasured_not_zero() {
        assert_eq!(
            freeze_owned_heartbeats(None, "u-alice").await,
            None,
            "an absent heartbeat service must not answer with a count"
        );
    }

    /// The measured-zero path, so the test above cannot pass by the leg never
    /// producing a number at all: a live service with nothing owned by this
    /// principal answers `Some(0)`, which is a different fact from `None`.
    #[tokio::test]
    async fn a_live_heartbeat_service_that_froze_nothing_reports_a_measured_zero() {
        let svc = heartbeat_service_with(&[("hb-x", Some("u-bob"))]).await;
        assert_eq!(
            freeze_owned_heartbeats(Some(svc.clone()), "u-alice").await,
            Some(0),
            "a service that ran and found nothing owned must say so with a zero"
        );
        assert_eq!(
            freeze_owned_heartbeats(Some(svc), "u-bob").await,
            Some(1),
            "and the same call must count what it did freeze"
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
            let mut state = ConnectionState::new(std::net::IpAddr::from([203, 0, 113, 7]), false);
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
                !guard.can_receive("pty.screen", &s.permissions),
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
        // `heartbeats` is deliberately NOT in this list. The other three legs
        // are always measured because their globals are `Option`s the freeze
        // simply skips; the heartbeat leg reports whether it RAN, so whether
        // the key is present here depends on whether some other test in this
        // binary has already installed the install-once global. Asserting on
        // it would be asserting on test order. Its two states are covered by
        // name in `a_missing_heartbeat_service_reports_unmeasured_not_zero`
        // and `deactivating_a_second_admin_disables_their_heartbeat_tasks`.
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

    /// The two-step, end to end: mint a ticket for a live principal,
    /// deactivate them, then attempt the redemption the ticket holder would
    /// attempt. The assertion is on the **devices table**, not on the RPC's
    /// status code — a rejected exchange that still leaves a device row behind
    /// is the exact failure this leg exists to prevent, and only the store can
    /// say whether the row appeared.
    #[tokio::test]
    async fn a_ticket_minted_before_deactivation_cannot_pair_a_device_after_it() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr =
            crate::gateway::security::device_token_manager::DeviceTokenManager::new(store.clone());
        let ticket = mgr
            .create_bootstrap_ticket(Some(600_000), Some("u-alice"))
            .unwrap();

        let resp = handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "status": "deactivated"}),
            ),
            store.clone(),
            test_kick_sink(),
        )
        .await;
        assert_eq!(
            response_json(&resp)["revoked_bootstrap_tickets"],
            1,
            "the deactivation receipt must report the burned ticket"
        );

        let exchanged = mgr.exchange_bootstrap_ticket(
            &ticket,
            Some("dev-late".to_string()),
            Some("Late Panel".to_string()),
            None,
        );
        assert!(exchanged.is_err(), "a burned ticket must not redeem");
        assert!(
            store.get_device("dev-late").unwrap().is_none(),
            "the exchange must not have created a device row — a row here is a live \
             credential minted AFTER the deactivation sweep ran"
        );
    }

    /// The reactivation receipt's "devices remain revoked — issue a new
    /// bootstrap ticket" is now true for every device, including the one an
    /// unredeemed ticket would have produced. Before the fourth leg that
    /// sentence was a lie about a device whose ten-year token worked.
    #[tokio::test]
    async fn reactivation_guidance_holds_because_the_old_ticket_is_dead() {
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr =
            crate::gateway::security::device_token_manager::DeviceTokenManager::new(store.clone());
        let ticket = mgr
            .create_bootstrap_ticket(Some(600_000), Some("u-alice"))
            .unwrap();

        for status in ["deactivated", "active"] {
            handle_update(
                rpc_request(
                    "users.update",
                    json!({"user_id": "u-alice", "status": status}),
                ),
                store.clone(),
                test_kick_sink(),
            )
            .await;
        }

        assert!(
            mgr.exchange_bootstrap_ticket(&ticket, Some("dev-old".to_string()), None, None)
                .is_err(),
            "reactivation must not resurrect the burned ticket"
        );
        assert!(
            store.get_device("dev-old").unwrap().is_none(),
            "'issue a NEW bootstrap ticket' must be the only path back"
        );
    }

    /// A retry of an already-deactivated principal burns nothing and records
    /// nothing — the audit line rides on the transition, exactly like
    /// `gateway_devices.rs`'s per-credential rule.
    #[tokio::test]
    async fn a_second_deactivation_burns_zero_tickets_and_writes_no_authority_change() {
        let _serial = crate::security::audit::AUDIT_TEST_LOCK.lock().unwrap();
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr =
            crate::gateway::security::device_token_manager::DeviceTokenManager::new(store.clone());
        mgr.create_bootstrap_ticket(Some(600_000), Some("u-alice"))
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

        // Install the audit handle only for the SECOND write, so anything it
        // receives came from the retry.
        let (log, mut rx) = crate::security::audit::SecurityAuditLog::new(16);
        crate::security::audit::replace_global_for_test(&log);
        let resp = handle_update(
            rpc_request(
                "users.update",
                json!({"user_id": "u-alice", "status": "deactivated"}),
            ),
            store.clone(),
            test_kick_sink(),
        )
        .await;
        let mut details = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            details.push(entry.detail);
        }
        crate::security::audit::clear_global_for_test();

        assert_eq!(
            response_json(&resp)["revoked_bootstrap_tickets"],
            0,
            "the retry had nothing left to burn"
        );
        assert!(
            !details.iter().any(|d| d.contains("bootstrap ticket")),
            "a retry that cut nothing must not write an authority-change row: {details:?}"
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

    // ========================================================================
    // users.get — the dossier read
    // ========================================================================

    /// Four real stores, none of them process-global, so the freeze and the
    /// preview can be asked the same question over the same data inside one
    /// test binary.
    fn background_fixture() -> (BackgroundWorkHandles, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let goals = Arc::new(crate::goal::GoalStore::open(&dir.path().join("goals.db")).unwrap());
        let loops = Arc::new(crate::looping::LoopRegistry::default());
        let crons = Arc::new(tokio::sync::Mutex::new(
            crate::tasks::cron::CronService::new(crate::tasks::cron::CronConfig {
                db_path: dir.path().join("cron.db").to_string_lossy().to_string(),
                ..crate::tasks::cron::CronConfig::default()
            })
            .unwrap(),
        ));
        let heartbeats = Arc::new(tokio::sync::Mutex::new(
            crate::tasks::heartbeat::HeartbeatService::new(
                crate::tasks::heartbeat::store::HeartbeatStore::open_in_memory().unwrap(),
                crate::tasks::heartbeat::config::HeartbeatConfig::default(),
            ),
        ));
        (
            BackgroundWorkHandles {
                goals: Some(goals),
                loops: Some(loops),
                crons: Some(crons),
                heartbeats: Some(heartbeats),
            },
            dir,
        )
    }

    fn seed_goal(handles: &BackgroundWorkHandles, session: &str, owner: Option<&str>) {
        let goal = crate::goal::Goal::new(session, "obj", 0, 0).with_pursuit(
            crate::goal::PursuitMode::Active { max_iterations: 5 },
        );
        let goal = match owner {
            Some(u) => {
                goal.with_owner_scope(Some(&crate::scope::ScopeAttribution::personal(u)))
            }
            None => goal,
        };
        handles.goals.as_ref().unwrap().put(&goal).unwrap();
    }

    fn seed_loop(handles: &BackgroundWorkHandles, session: &str, owner: Option<&str>) {
        let state = crate::looping::LoopState::new(
            session,
            "p",
            crate::looping::Cadence::Fixed { interval_ms: 1000 },
            0,
        );
        let state = match owner {
            Some(u) => {
                state.with_owner_scope(Some(&crate::scope::ScopeAttribution::personal(u)))
            }
            None => state,
        };
        handles.loops.as_ref().unwrap().put(state);
    }

    async fn seed_cron(handles: &BackgroundWorkHandles, name: &str, owner: Option<&str>) {
        let mut job = crate::tasks::cron::CronJob::new(
            name,
            "agent-1",
            "do something",
            crate::tasks::cron::ScheduleKind::Every {
                every_ms: 60_000,
                anchor_ms: None,
            },
        );
        job.owner_user_id = owner.map(str::to_string);
        handles
            .crons
            .as_ref()
            .unwrap()
            .lock()
            .await
            .add_job(job)
            .await
            .unwrap();
    }

    /// The preview and the freeze answer over the SAME four stores, and when
    /// every owned row is active the two structs are equal — asserted as
    /// whole structs, so a fifth leg cannot land on one surface and not the
    /// other (criterion #1: one leg enumeration, two readers).
    #[tokio::test]
    async fn the_preview_and_the_freeze_report_the_same_struct_for_all_active_work() {
        let (handles, _dir) = background_fixture();
        seed_goal(&handles, "s-alice", Some("u-alice"));
        seed_loop(&handles, "l-alice", Some("u-alice"));
        seed_cron(&handles, "c-alice", Some("u-alice")).await;

        let preview = count_owned_background_work_with(&handles, "u-alice").await;
        assert_eq!(
            preview,
            aleph_protocol::users::FrozenBackgroundWork {
                goals: 1,
                loops: 1,
                crons: 1,
                heartbeats: Some(0),
            }
        );

        let frozen = freeze_owned_background_work_with(&handles, "u-alice").await;
        assert_eq!(
            frozen, preview,
            "with nothing already paused, what she owns and what the freeze \
             changed are the same numbers — and the same TYPE, so a fifth leg \
             cannot be added to one of them alone"
        );
    }

    /// The deliberate asymmetry, pinned as two explicit numbers rather than
    /// as an equality: already-paused work is 0-changed to the freeze and
    /// still hers in the preview. Swap the preview's filter for the freeze's
    /// `enabled`/`is_active` one and the first assertion drops to 0.
    #[tokio::test]
    async fn already_paused_work_is_counted_by_the_read_and_reported_zero_by_the_freeze() {
        let (handles, _dir) = background_fixture();
        seed_goal(&handles, "s-alice", Some("u-alice"));
        seed_loop(&handles, "l-alice", Some("u-alice"));
        seed_cron(&handles, "c-alice", Some("u-alice")).await;
        // Freeze once: now every one of her rows is paused/disabled.
        let first = freeze_owned_background_work_with(&handles, "u-alice").await;
        assert_eq!((first.goals, first.loops, first.crons), (1, 1, 1));

        let preview = count_owned_background_work_with(&handles, "u-alice").await;
        assert_eq!(
            (preview.goals, preview.loops, preview.crons),
            (1, 1, 1),
            "she still OWNS all three — the operator must see what they are \
             about to strand"
        );

        let second = freeze_owned_background_work_with(&handles, "u-alice").await;
        assert_eq!(
            (second.goals, second.loops, second.crons),
            (0, 0, 0),
            "the freeze reports only what it CHANGED, and it changed nothing"
        );
    }

    /// A row stamped before P1 belongs to nobody: it appears in no
    /// principal's dossier and is never counted for one.
    #[tokio::test]
    async fn legacy_unowned_work_appears_in_no_principals_dossier() {
        let (handles, _dir) = background_fixture();
        seed_goal(&handles, "s-legacy", None);
        seed_loop(&handles, "l-legacy", None);
        seed_cron(&handles, "c-legacy", None).await;

        for principal in ["u-alice", "u-bob", "u-owner"] {
            let preview = count_owned_background_work_with(&handles, principal).await;
            assert_eq!(
                (preview.goals, preview.loops, preview.crons),
                (0, 0, 0),
                "{principal} must not be credited with an unstamped row"
            );
        }
    }

    /// Fail-closed (criterion #8): with no heartbeat service in this process
    /// the leg says nothing, never `Some(0)` — "nobody looked" and "they own
    /// none" are different answers, and the CLI gives each its own sentence.
    #[tokio::test]
    async fn an_unreachable_heartbeat_service_leaves_the_leg_unmeasured() {
        let (mut handles, _dir) = background_fixture();
        handles.heartbeats = None;
        let preview = count_owned_background_work_with(&handles, "u-alice").await;
        assert_eq!(preview.heartbeats, None);
    }

    fn detail_ctx() -> UserDetailContext {
        UserDetailContext {
            projects: Arc::new({
                let store =
                    crate::projects::ProjectStore::new(rusqlite::Connection::open_in_memory().unwrap());
                store.create_schema().unwrap();
                store
            }),
        }
    }

    /// The composition an operator reads before the one-way door: the
    /// principal's own record, their live panel devices, and the rooms they
    /// sit in. A device belonging to somebody else and a room she is not
    /// seated in must not appear.
    #[tokio::test]
    async fn the_dossier_names_her_devices_and_her_rooms_and_nobody_elses() {
        let _g = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        store.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        upsert_panel_device(&store, "dev-alice-1", "u-alice");
        upsert_panel_device(&store, "dev-alice-2", "u-alice");
        upsert_panel_device(&store, "dev-bob", "u-bob");
        // A node-namespace row bound to alice: `list_device_ids_for_user`
        // filters `device_type = 'panel'`, which is exactly why the field is
        // named "live panel devices".
        upsert_node_device(&store, "node-alice", "u-alice");

        let ctx = detail_ctx();
        let hers = ctx.projects.create("hers", Some("u-alice"), None).unwrap();
        let his = ctx.projects.create("his", Some("u-bob"), None).unwrap();

        let resp = handle_get(
            rpc_request("users.get", json!({"user_id": "u-alice"})),
            store,
            ctx,
        )
        .await;
        let detail: aleph_protocol::users::UserDetail =
            serde_json::from_value(response_json(&resp)).unwrap();

        assert_eq!(detail.user.user_id, "u-alice");
        assert_eq!(detail.user.display_name, "Alice");
        assert_eq!(
            detail.live_panel_devices, 2,
            "bob's device and the node-namespace row are not hers"
        );
        assert_eq!(detail.room_ids, vec![hers.id]);
        assert!(!detail.room_ids.contains(&his.id));
    }

    /// With no ledger row the field is absent, so the renderer can say "no
    /// spend recorded" rather than printing a measured-looking `0.00`.
    #[tokio::test]
    async fn a_principal_with_no_ledger_row_reports_no_spend_rather_than_zero() {
        let _g = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let store = seeded_store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let resp = handle_get(
            rpc_request("users.get", json!({"user_id": "u-alice"})),
            store,
            detail_ctx(),
        )
        .await;
        let raw = response_json(&resp);
        assert!(
            raw.get("spend").is_none(),
            "an unrecorded spend must not reach the wire as a number: {raw}"
        );
    }

    /// An unknown principal is a refusal, not an empty dossier — an
    /// all-zero composition would read as "they hold nothing", which is the
    /// most dangerous possible answer immediately before a deactivation.
    #[tokio::test]
    async fn an_unknown_principal_is_refused_not_answered_with_an_empty_dossier() {
        let _g = crate::projects::roster::TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let resp = handle_get(
            rpc_request("users.get", json!({"user_id": "u-nobody"})),
            seeded_store(),
            detail_ctx(),
        )
        .await;
        assert!(response_is_error(&resp));
    }

    // ========================================================================
    // The two registration faces, and the gate that covers them
    // ========================================================================

    /// Face one: the default in-memory registry every test harness that boots
    /// via `HandlerRegistry::new()` gets.
    #[test]
    fn the_default_registry_resolves_users_get() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        assert!(registry.has_method("users.get"));
        // Its siblings, so a registry that resolved nothing at all could not
        // pass this by accident.
        assert!(registry.has_method("users.me"));
        assert!(registry.has_method("users.update"));
    }

    /// Face two, mirrored: every `users.` method BOOT registers must also be
    /// in the default registry. The opposite direction is asserted from the
    /// boot file itself (`start/mod.rs`'s
    /// `boot_registers_every_users_method_the_default_registry_has`), so
    /// neither face can grow a method the other lacks — the shape that let a
    /// `users.*` verb resolve in tests and be `METHOD_NOT_FOUND` on a real
    /// server.
    #[test]
    fn the_default_registry_and_boot_register_the_same_users_methods() {
        let boot = include_str!("../../bin/aleph-server/commands/start/mod.rs").replace('\r', "");
        let boot = crate::utils::source_scan::strip_comment_lines(
            &crate::utils::source_scan::production_prefix(&boot),
        );
        let names: Vec<String> = boot
            .split("register(\"users.")
            .skip(1)
            .filter_map(|seg| seg.split('"').next())
            .map(|suffix| format!("users.{suffix}"))
            .collect();
        assert!(
            names.contains(&"users.get".to_string()),
            "the scrape found no `users.get` at boot — this guard is reading \
             the wrong file or shape and would pass over an empty set"
        );

        let registry = crate::gateway::handlers::HandlerRegistry::new();
        for method in names {
            assert!(
                registry.has_method(&method),
                "{method} is registered at boot but not in the default \
                 registry — one face of a two-faced registration"
            );
        }
    }

    /// No carve-out was added. `method_admin.rs` already gates the whole
    /// `users.` prefix; the member-safe exceptions are `users.me` and
    /// `users.list` and there must still be exactly those two, so a dossier
    /// over somebody else's holdings stays admin-only.
    #[test]
    fn users_get_is_admin_gated_with_no_new_carve_out() {
        assert!(crate::gateway::method_admin::method_requires_admin(
            "users.get"
        ));
        // Read out of the table that OWNS the fact, not a copy retyped here.
        let (_, entries) = crate::gateway::method_admin::GHOST_CHECK_TABLES
            .iter()
            .find(|(name, _)| *name == "MEMBER_CARVE_OUTS")
            .expect("the carve-out table is named in GHOST_CHECK_TABLES");
        let carved: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|m| m.starts_with("users."))
            .collect();
        assert_eq!(
            carved,
            ["users.me", "users.list"],
            "the `users.` carve-out list must still be exactly two entries — a \
             third would open somebody else's dossier to every member"
        );
    }
}
