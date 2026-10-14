//! Auth phase — connection identity stamping + login wall.
//!
//! Owns the pure functions that resolve *who* a connection is and *what*
//! authority it carries. The actual `connect` handshake still runs inside
//! the dispatch loop in `connection/mod.rs` — these are the verdict
//! builders it calls and the wall predicate every direction (request arm,
//! event-forward arm, metrics gauge) re-derives the answer from. All items
//! are `pub(crate)` so `handler.rs` can re-export them for the test
//! module (`use super::*;`).

/// The login wall's refusal text, shared by the request-arrival check in the
/// connection loop and the dispatch-time recheck of off-loop RPCs.
pub const LOGIN_WALL_MESSAGE: &str =
    "Not authorized: present a valid Gateway token via `connect` to access this core.";

/// The guest login wall: may a connection stamped with `role` send `method`?
///
/// The wall is the *guest* wall and nothing else — it separates "this
/// connection presented a credential" from "it did not." Both authorized roles
/// pass every method: `"operator"` (loopback, legacy shared token, or a device
/// bound to an `admin` user) and `"member"` (a device bound to a `member`-role
/// user). The admin/member split for server-global methods is decided further
/// in, at the `process_request` chokepoint (`method_admin.rs`) — teaching this
/// predicate about it would put the same decision in two places.
///
/// Anything else — `"guest"`, an unrecognized role string, or absent
/// connection state — may only send `connect` to authorize (fail closed).
///
/// ## The wall has TWO consumers, and it had one for too long
///
/// A connection has two directions, and until 2026-08-08 this predicate was
/// evaluated on only one of them. The request arm consults it before dispatch;
/// the *event-forward* arm's verdict was `scope_allowed && audience_allows &&
/// should_receive && event_admits` — four terms, none of them authentication.
/// Every one of them passes for a connection that has never authorized:
/// `ConnectionState` is inserted into `ctx.connections` the moment the socket is
/// accepted (`permissions: []`, `caller_user: None`, `caller_role: "guest"`),
/// `can_receive` allows any topic no rule names, `should_receive` returns `true`
/// when the connection registered no filter at all, and `event_admits` short-
/// circuits on `SessionIdentity::Global` *before* it reads `caller_user`. A bare
/// remote WebSocket that sent nothing therefore received every `Global` frame —
/// including `pty.screen`, whose RPC face has been in `ADMIN_PREFIXES` all along.
///
/// So: **an authorization predicate belongs on every direction a connection
/// carries data, not on the one where the caller asks a question.** The event
/// arm now evaluates this same function on the same `caller_role` field, which
/// also means `restamp_live_connections` closes both planes at once — a
/// deactivated user's socket stops receiving in the same instant it stops being
/// served.
///
/// Pure so the wall's own logic is host-testable. The
/// `resolve_stamped_identity` tests below cover *what role gets stamped*; the
/// class of bug this function exists to prevent lives in the *predicate* — a
/// correctly-stamped `"member"` being refused every method and then
/// flood-guard-kicked as an abuser stays green under any test that scopes
/// task-locals below the wall.
///
/// A third consumer reads it with `method: ""`:
/// `metrics_endpoint::count_authenticated`. "Does this connection hold
/// authority" must have ONE derivation in the gateway, so the gauge moves when
/// `restamp_live_connections` demotes someone, exactly as the two delivery
/// planes do.
#[must_use]
pub fn wall_admits(role: Option<&str>, method: &str) -> bool {
    matches!(role, Some("operator" | "member")) || method == "connect"
}

/// The authorization verdict echoed back in a `connect` response:
/// `(role, authorized, needs_token)`.
///
/// Derived from the **resolved** identity, never from the raw credential
/// verdict alone. "Was the credential valid" and "does this connection hold
/// any authority" are different questions, and P0 made them come apart: a
/// device token that is still valid but whose bound user was deactivated (or
/// whose `user_id` dangles) is a valid credential that grants nothing. It must
/// be reported with the shape the Panel already knows — `("guest", false,
/// true)`, i.e. the login wall — rather than a new close reason or verdict
/// word no client parses.
///
/// Pure so the exact wire triple is host-testable; the surrounding JSON
/// insertion has no seam (it edits a response inside the live WS loop).
#[must_use]
pub fn connect_verdict(credential_ok: bool, resolved_role: &str) -> (&str, bool, bool) {
    let holds_authority = credential_ok && resolved_role != "guest";
    (resolved_role, holds_authority, !holds_authority)
}

/// Resolve the `(caller_role, caller_user)` pair stamped onto
/// `ConnectionState` at a `connect` handshake, given the authorization
/// verdict `resolve_connect_auth` already decided. Pure — host-testable
/// without a live WS socket, unlike the handshake it's extracted from.
///
/// `authorized == false` stays walled (guest, no user) exactly as before
/// per-user resolution existed. `authorized == true` resolves the bound
/// device's user via [`resolve_connection_identity`](crate::gateway::handlers::connect::resolve_connection_identity)
/// when a security store is available — loopback and legacy unbound-device
/// paths still resolve to the implicit owner as operator (zero-change
/// guarantee), but a device bound to a deactivated user is walled here even
/// though its token was valid.
///
/// With **no store wired** (probe/test server) the arm splits on whether the
/// connection is device-bound. No device and no store is the pre-P0 shape
/// (loopback / legacy shared token) and keeps resolving to the implicit owner
/// as operator — unchanged from before per-user resolution existed. A device
/// *is* presented but there is no store to resolve it against ⇒ `("guest",
/// None)`, fail-closed, mirroring the ruled `Err` semantics inside
/// `resolve_connection_identity`: a binding lookup that could not be
/// performed must never be read as "unbound, therefore owner" — that is the
/// one input a remote caller controls, and it would otherwise buy full
/// operator authority on any deployment whose store failed to wire.
pub fn resolve_stamped_identity(
    authorized: bool,
    is_loopback: bool,
    device_id: Option<&str>,
    store: Option<&crate::gateway::security::store::SecurityStore>,
) -> (Option<String>, &'static str) {
    if !authorized {
        return (None, "guest");
    }
    match store {
        Some(store) => crate::gateway::handlers::connect::resolve_connection_identity(
            is_loopback,
            device_id,
            store,
        ),
        // Device-bound but unresolvable: fail closed (see doc above).
        None if device_id.is_some() => (None, "guest"),
        None => (
            Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
            "operator",
        ),
    }
}

/// Derive the server-verified user for a `connect` handshake (the value
/// stamped into `ConnectionState::verified_user`).
///
/// Stricter than [`resolve_stamped_identity`], which also resolves the implicit
/// owner for credential-less loopback, the legacy shared token and unbound
/// devices. This returns `Some(uid)` ONLY when all hold:
/// - `authorized` (the credential verdict) and `device_id` is the id a
///   *validated* credential resolved to (never a client claim);
/// - a `store` is wired;
/// - `store.device_user(device_id)` is `Ok(Some(uid))` with a non-empty `uid`
///   (a real binding, not the unbound/compat path);
/// - `store.get_user(uid)` is `Ok(Some(user))` with `UserStatus::Active`;
/// - `uid` equals the non-empty `resolved_user` from `resolve_stamped_identity`.
///
/// Everything else -- no device, no store, unbound, store error, deactivated,
/// dangling, mismatch, empty ids -- is `None` (fail closed). Never inferred
/// from loopback, role, device name or request params. Pure apart from the
/// two store reads; called once per handshake, not per request.
#[must_use]
pub fn derive_verified_user(
    authorized: bool,
    device_id: Option<&str>,
    resolved_user: Option<&str>,
    store: Option<&crate::gateway::security::store::SecurityStore>,
) -> Option<String> {
    use crate::gateway::security::store::UserStatus;

    if !authorized {
        return None;
    }
    let device_id = device_id.filter(|d| !d.trim().is_empty())?;
    let resolved_user = resolved_user.filter(|u| !u.trim().is_empty())?;
    let store = store?;
    // A revoked / unknown device proves nobody, even if its row still carries a
    // binding. Error folds to "not approved" (fail closed).
    if !matches!(store.is_device_approved(device_id), Ok(true)) {
        return None;
    }
    let bound = match store.device_user(device_id) {
        Ok(Some(uid)) if !uid.trim().is_empty() => uid,
        // Unbound (compat owner), or a lookup we could not perform.
        Ok(Some(_)) | Ok(None) | Err(_) => return None,
    };
    if bound != resolved_user {
        return None;
    }
    match store.get_user(&bound) {
        Ok(Some(user)) if user.status == UserStatus::Active && user.user_id == bound => Some(bound),
        // Deactivated, dangling, or store error.
        Ok(Some(_)) | Ok(None) | Err(_) => None,
    }
}

/// The single final verdict of a `connect` handshake: `(caller_user, role,
/// verified_user)`. Meant to be evaluated INSIDE the connection-map write
/// guard, so it is serialized against `revoke_device_and_kick` (which writes
/// the store revoke under the same guard): a handshake either runs first (and
/// the revoke then clears what it stamped) or after (and sees the device
/// unapproved here).
///
/// A credential-proved device (`authed_device_id`) must be currently approved
/// in a wired store; unknown / revoked / store error / no store all fold to
/// `(None, "guest", None)`. Credential-less loopback and legacy shared-token
/// connects (no device) keep the pre-existing compat-owner resolution.
#[must_use]
pub fn finalize_handshake_identity(
    authorized: bool,
    is_loopback: bool,
    authed_device_id: Option<&str>,
    credential_verified_device_id: Option<&str>,
    store: Option<&crate::gateway::security::store::SecurityStore>,
) -> (Option<String>, &'static str, Option<String>) {
    if !authorized {
        return (None, "guest", None);
    }
    if let Some(did) = authed_device_id {
        let approved = store.is_some_and(|s| matches!(s.is_device_approved(did), Ok(true)));
        if !approved {
            return (None, "guest", None);
        }
    }
    let (user, role) = resolve_stamped_identity(authorized, is_loopback, authed_device_id, store);
    let verified = derive_verified_user(
        authorized,
        credential_verified_device_id,
        user.as_deref(),
        store,
    );
    (user, role, verified)
}

/// The device id a handshake credential PROVES, for [`derive_verified_user`].
///
/// - `Authorized { device_id }`: the id a validated device token resolved to.
/// - `BootstrapExchanged`: the exchange atomically consumed a one-time ticket
///   and minted a token. The response's `device_id` is not trusted by itself;
///   it is accepted only if the freshly minted `device_token` validates in the
///   store to a row whose `device_id` equals it. Any error / missing row /
///   mismatch is `None` (fail closed, no fallback).
/// - `Unauthorized`: `None`.
///
/// Never reads the request's own claimed device id, and never consults node
/// admission (which runs later and mints its own ids).
///
/// NOTE: point-in-time derivation. The production handshake no longer
/// authorizes from it; the final source is
/// [`revalidate_connect_credential_source`], run under the connection-map
/// write guard.
#[must_use]
pub fn credential_provenance_device_id(
    outcome: &crate::gateway::handlers::connect::ConnectAuthOutcome,
    manager: Option<&crate::gateway::security::DeviceTokenManager>,
) -> Option<String> {
    use crate::gateway::handlers::connect::ConnectAuthOutcome;

    match outcome {
        ConnectAuthOutcome::Authorized { device_id } => device_id.clone(),
        ConnectAuthOutcome::BootstrapExchanged {
            device_id,
            device_token,
        } => {
            let row = manager?.validate_device_token(device_token).ok()??;
            (!row.device_id.trim().is_empty() && row.device_id == *device_id)
                .then_some(row.device_id)
        }
        ConnectAuthOutcome::Unauthorized => None,
    }
}

/// Re-validate, at FINAL authorization time (inside the connection-map write
/// guard), that the credential this handshake authenticated with is still
/// live. Returns `(credential_ok, verified_device_id)`.
///
/// `resolve_connect_auth` validated the credential long before the map lock;
/// a `revoke` + re-pair of the same device id in between would leave
/// `is_device_approved(did)` true again (`upsert_device` can re-approve a
/// revoked row), so approval alone cannot prove the OLD credential is still
/// valid. This re-checks the credential itself:
///
/// - `Authorized { device_id: Some(expected) }`: the exact `device_token` of
///   the connect params must still validate to a row whose `device_id` equals
///   `expected` (exact, non-blank). Token gone / mismatch / manager absent /
///   store error => `(false, None)`.
/// - `BootstrapExchanged`: re-validates the ALREADY-minted `device_token`
///   (never re-consumes the ticket) the same way.
/// - `Authorized { device_id: None }` (loopback / shared token, credentialless
///   compat): `(true, None)`.
/// - `Unauthorized`: `(false, None)`.
///
/// Never reads the request's claimed `device_id` / `device_name`.
#[must_use]
pub fn revalidate_connect_credential_source(
    outcome: &crate::gateway::handlers::connect::ConnectAuthOutcome,
    params: Option<&serde_json::Value>,
    manager: Option<&crate::gateway::security::DeviceTokenManager>,
) -> (bool, Option<String>) {
    use crate::gateway::handlers::connect::ConnectAuthOutcome;

    fn still_valid(
        manager: Option<&crate::gateway::security::DeviceTokenManager>,
        token: Option<&str>,
        expected: &str,
    ) -> (bool, Option<String>) {
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return (false, None);
        };
        if expected.trim().is_empty() {
            return (false, None);
        }
        let Some(mgr) = manager else {
            return (false, None);
        };
        match mgr.validate_device_token(token) {
            Ok(Some(row)) if !row.device_id.trim().is_empty() && row.device_id == expected => {
                (true, Some(row.device_id))
            }
            _ => (false, None),
        }
    }

    match outcome {
        ConnectAuthOutcome::Authorized {
            device_id: Some(expected),
        } => {
            let token = params
                .and_then(|p| p.get("device_token"))
                .and_then(|v| v.as_str());
            still_valid(manager, token, expected)
        }
        ConnectAuthOutcome::Authorized { device_id: None } => (true, None),
        ConnectAuthOutcome::BootstrapExchanged {
            device_id,
            device_token,
        } => still_valid(manager, Some(device_token.as_str()), device_id),
        ConnectAuthOutcome::Unauthorized => (false, None),
    }
}

/// What a cluster node claims about itself in its `connect` frame.
pub struct NodeConnectClaim {
    /// The node's persisted `node_id`, or `None` on its very first boot (it has
    /// nothing to present yet — `cluster::admit_node` hands one back).
    pub presented_id: Option<String>,
    pub device_name: String,
}

/// LAN-trust cluster-node detection at `connect` time.
///
/// Token roles are gone, so a cluster node announces itself by request shape:
/// the node client (`aleph-server node`) always sends `commands` + `tags` in its
/// connect params, which no other client does. Returns `None` for every non-node
/// connect. Unlike the old `node_connect_identity`, this does NOT invent an id
/// from `device_name`/`conn_id` — identity is resolved against the device store
/// by [`crate::cluster::admit_node`], so a node's id is stable across reconnects
/// and a revoked node can be told apart from a brand-new one.
pub fn node_connect_claim(params: Option<&serde_json::Value>) -> Option<NodeConnectClaim> {
    let p = params?;
    if p.get("commands").is_none() && p.get("tags").is_none() {
        return None;
    }
    Some(NodeConnectClaim {
        presented_id: p
            .get("device_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        device_name: p
            .get("device_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}

/// Did the `connect` params explicitly present a device credential
/// (`device_token` or `bootstrap_ticket`, non-empty)? Mirrors the
/// `presents_credential` derivation in `handlers::connect::resolve_connect_auth`.
#[must_use]
pub fn has_presented_device_credential(
    device_token: Option<&str>,
    bootstrap_ticket: Option<&str>,
) -> bool {
    [device_token, bootstrap_ticket]
        .into_iter()
        .flatten()
        .any(|c| !c.is_empty())
}

/// Connect verdict when the device-token manager is NOT wired.
///
/// Fail closed on an explicit device credential: nothing here can validate it,
/// so it must never be silently downgraded to the legacy shared-token /
/// loopback-operator path. Only credential-less connects reach `legacy_authorized`.
#[must_use]
pub fn connect_auth_without_device_manager(
    device_token: Option<&str>,
    bootstrap_ticket: Option<&str>,
    legacy_authorized: impl FnOnce() -> bool,
) -> crate::gateway::handlers::connect::ConnectAuthOutcome {
    use crate::gateway::handlers::connect::ConnectAuthOutcome;
    if has_presented_device_credential(device_token, bootstrap_ticket) || !legacy_authorized() {
        ConnectAuthOutcome::Unauthorized
    } else {
        ConnectAuthOutcome::Authorized { device_id: None }
    }
}

#[cfg(test)]
mod verified_user_tests {
    use super::*;
    use crate::gateway::security::store::{
        DeviceUpsertData, SecurityStore, UserRole, UserStatus, OWNER_USER_ID,
    };

    fn add_device(store: &SecurityStore, device_id: &str) {
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
    }

    fn bound_store(device_id: &str, user_id: &str, role: UserRole) -> SecurityStore {
        let store = SecurityStore::in_memory().unwrap();
        store.create_user(user_id, "Test User", role).unwrap();
        add_device(&store, device_id);
        store.set_device_user(device_id, user_id).unwrap();
        store
    }

    /// Run the real handshake chain: resolve identity, then derive verified.
    fn derive(
        authorized: bool,
        device_id: Option<&str>,
        store: Option<&SecurityStore>,
    ) -> Option<String> {
        let (resolved, _role) = resolve_stamped_identity(authorized, false, device_id, store);
        derive_verified_user(authorized, device_id, resolved.as_deref(), store)
    }

    #[test]
    fn active_bound_member_and_admin_are_verified() {
        let member = bound_store("dev-a", "u-alice", UserRole::Member);
        assert_eq!(
            derive(true, Some("dev-a"), Some(&member)).as_deref(),
            Some("u-alice")
        );
        let admin = bound_store("dev-r", "u-root", UserRole::Admin);
        assert_eq!(
            derive(true, Some("dev-r"), Some(&admin)).as_deref(),
            Some("u-root")
        );
    }

    #[test]
    fn device_bound_to_the_owner_user_is_verified() {
        // An explicit binding to the owner row is a real binding, unlike the
        // unbound compat fallback that merely resolves to the same id.
        let store = SecurityStore::in_memory().unwrap();
        add_device(&store, "dev-o");
        store.set_device_user("dev-o", OWNER_USER_ID).unwrap();
        assert_eq!(
            derive(true, Some("dev-o"), Some(&store)).as_deref(),
            Some(OWNER_USER_ID)
        );
    }

    #[test]
    fn compat_owner_shapes_are_never_verified() {
        let store = SecurityStore::in_memory().unwrap();
        // Credential-less loopback / legacy shared token: authorized, no device.
        assert_eq!(derive(true, None, Some(&store)), None);
        assert_eq!(derive(true, None, None), None);
        // Unbound device resolves to the implicit owner but proves nothing.
        add_device(&store, "dev-unbound");
        let (resolved, role) =
            resolve_stamped_identity(true, false, Some("dev-unbound"), Some(&store));
        assert_eq!(resolved.as_deref(), Some(OWNER_USER_ID));
        assert_eq!(role, "operator");
        assert_eq!(derive(true, Some("dev-unbound"), Some(&store)), None);
        // Unknown device row.
        assert_eq!(derive(true, Some("dev-missing"), Some(&store)), None);
    }

    #[test]
    fn unauthorized_no_store_or_empty_ids_are_not_verified() {
        let store = bound_store("dev-a", "u-alice", UserRole::Member);
        assert_eq!(
            derive_verified_user(false, Some("dev-a"), Some("u-alice"), Some(&store)),
            None
        );
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), Some("u-alice"), None),
            None
        );
        assert_eq!(
            derive_verified_user(true, None, Some("u-alice"), Some(&store)),
            None
        );
        assert_eq!(
            derive_verified_user(true, Some(""), Some("u-alice"), Some(&store)),
            None
        );
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), None, Some(&store)),
            None
        );
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), Some(""), Some(&store)),
            None
        );
    }

    #[test]
    fn resolved_user_mismatch_is_not_verified() {
        let store = bound_store("dev-a", "u-alice", UserRole::Member);
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), Some(OWNER_USER_ID), Some(&store)),
            None
        );
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), Some("u-bob"), Some(&store)),
            None
        );
    }

    #[test]
    fn deactivated_user_is_not_verified() {
        let store = bound_store("dev-a", "u-alice", UserRole::Member);
        store
            .update_user("u-alice", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        // Even if a caller still passed the stale resolved user, the Active
        // check inside the helper refuses.
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), Some("u-alice"), Some(&store)),
            None
        );
        assert_eq!(derive(true, Some("dev-a"), Some(&store)), None);
    }

    #[test]
    fn dangling_binding_is_not_verified() {
        let store = SecurityStore::in_memory().unwrap();
        add_device(&store, "dev-ghost");
        store
            .set_device_user("dev-ghost", "u-never-created")
            .unwrap();
        assert_eq!(
            derive_verified_user(
                true,
                Some("dev-ghost"),
                Some("u-never-created"),
                Some(&store)
            ),
            None
        );
        assert_eq!(derive(true, Some("dev-ghost"), Some(&store)), None);
    }

    #[test]
    fn revoked_device_binding_is_not_verified() {
        let store = bound_store("dev-a", "u-alice", UserRole::Member);
        assert_eq!(
            derive(true, Some("dev-a"), Some(&store)).as_deref(),
            Some("u-alice")
        );
        assert!(store.revoke_device("dev-a").unwrap());
        // Binding row still exists, but the device is no longer approved.
        assert_eq!(
            derive_verified_user(true, Some("dev-a"), Some("u-alice"), Some(&store)),
            None
        );
        assert_eq!(derive(true, Some("dev-a"), Some(&store)), None);
    }

    #[test]
    fn finalize_approved_member_is_verified_member() {
        let store = bound_store("dev-a", "u-alice", UserRole::Member);
        let (user, role, verified) =
            finalize_handshake_identity(true, false, Some("dev-a"), Some("dev-a"), Some(&store));
        assert_eq!(user.as_deref(), Some("u-alice"));
        assert_ne!(role, "guest");
        assert_eq!(verified.as_deref(), Some("u-alice"));
    }

    #[test]
    fn finalize_after_revoke_cannot_restore_verified() {
        // Ordering: credential check passed (device was approved), then the
        // revoke landed, then the final verdict runs. It must be the guest
        // verdict -- the stale credential cannot resurrect the identity.
        let store = bound_store("dev-a", "u-alice", UserRole::Member);
        store.revoke_device("dev-a").unwrap();
        let (user, role, verified) =
            finalize_handshake_identity(true, false, Some("dev-a"), Some("dev-a"), Some(&store));
        assert_eq!((user, role, verified), (None, "guest", None));
        // Loopback does not rescue a revoked credential-proved device.
        let (user, role, verified) =
            finalize_handshake_identity(true, true, Some("dev-a"), Some("dev-a"), Some(&store));
        assert_eq!((user, role, verified), (None, "guest", None));
    }

    #[test]
    fn finalize_unknown_device_or_missing_store_is_guest() {
        let store = SecurityStore::in_memory().unwrap();
        assert_eq!(
            finalize_handshake_identity(
                true,
                false,
                Some("dev-new"),
                Some("dev-new"),
                Some(&store)
            ),
            (None, "guest", None)
        );
        assert_eq!(
            finalize_handshake_identity(true, false, Some("dev-new"), Some("dev-new"), None),
            (None, "guest", None)
        );
        assert_eq!(
            finalize_handshake_identity(false, true, None, None, Some(&store)),
            (None, "guest", None)
        );
    }

    #[test]
    fn finalize_credentialless_loopback_keeps_compat_owner_unverified() {
        let store = SecurityStore::in_memory().unwrap();
        let (user, role, verified) =
            finalize_handshake_identity(true, true, None, None, Some(&store));
        assert_eq!(user.as_deref(), Some(OWNER_USER_ID));
        assert_ne!(role, "guest");
        assert_eq!(verified, None);
    }
}

#[cfg(test)]
mod credential_provenance_tests {
    use super::*;
    use crate::gateway::handlers::connect::ConnectAuthOutcome;
    use crate::gateway::security::store::{SecurityStore, UserRole, OWNER_USER_ID};
    use crate::gateway::security::DeviceTokenManager;
    use std::sync::Arc;

    fn fixture() -> (Arc<SecurityStore>, DeviceTokenManager) {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = DeviceTokenManager::new(store.clone());
        (store, mgr)
    }

    /// The real handshake chain after `resolve_connect_auth`: provenance ->
    /// identity resolution -> verified derivation (what stamp and echo use).
    fn handshake_verified(
        outcome: &ConnectAuthOutcome,
        mgr: &DeviceTokenManager,
        store: &SecurityStore,
    ) -> Option<String> {
        let authorized = !matches!(outcome, ConnectAuthOutcome::Unauthorized);
        let did = credential_provenance_device_id(outcome, Some(mgr));
        let (resolved, _role) =
            resolve_stamped_identity(authorized, false, did.as_deref(), Some(store));
        derive_verified_user(authorized, did.as_deref(), resolved.as_deref(), Some(store))
    }

    fn exchange(mgr: &DeviceTokenManager, user: Option<&str>, did: &str) -> ConnectAuthOutcome {
        let ticket = mgr.create_bootstrap_ticket(None, user).unwrap();
        let r = mgr
            .exchange_bootstrap_ticket(&ticket, Some(did.to_string()), None, None)
            .unwrap();
        ConnectAuthOutcome::BootstrapExchanged {
            device_id: r.device_id,
            device_token: r.device_token,
        }
    }

    #[test]
    fn bootstrap_ticket_bound_to_member_yields_verified_member() {
        let (store, mgr) = fixture();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let outcome = exchange(&mgr, Some("u-alice"), "dev-new");
        assert_eq!(
            handshake_verified(&outcome, &mgr, &store).as_deref(),
            Some("u-alice")
        );
    }

    #[test]
    fn bootstrap_ticket_unbound_binds_owner_and_is_verified_owner() {
        let (store, mgr) = fixture();
        let outcome = exchange(&mgr, None, "dev-owner");
        assert_eq!(
            handshake_verified(&outcome, &mgr, &store).as_deref(),
            Some(OWNER_USER_ID)
        );
    }

    #[test]
    fn bootstrap_response_device_id_mismatch_is_denied() {
        let (store, mgr) = fixture();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        store.create_user("u-bob", "Bob", UserRole::Member).unwrap();
        let ConnectAuthOutcome::BootstrapExchanged { device_token, .. } =
            exchange(&mgr, Some("u-alice"), "dev-alice")
        else {
            unreachable!()
        };
        let other = exchange(&mgr, Some("u-bob"), "dev-bob");
        let ConnectAuthOutcome::BootstrapExchanged {
            device_id: bob_dev, ..
        } = other
        else {
            unreachable!()
        };
        // Malicious/buggy response: alice's token paired with bob's device id.
        let forged = ConnectAuthOutcome::BootstrapExchanged {
            device_id: bob_dev,
            device_token,
        };
        assert_eq!(credential_provenance_device_id(&forged, Some(&mgr)), None);
        assert_eq!(handshake_verified(&forged, &mgr, &store), None);
        // Unknown token, no manager, blank id: all fail closed.
        let unknown = ConnectAuthOutcome::BootstrapExchanged {
            device_id: "dev-alice".into(),
            device_token: "not-a-token".into(),
        };
        assert_eq!(credential_provenance_device_id(&unknown, Some(&mgr)), None);
        assert_eq!(credential_provenance_device_id(&unknown, None), None);
    }

    #[test]
    fn authorized_passes_through_and_credentialless_or_unauthorized_is_denied() {
        let (store, mgr) = fixture();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let ConnectAuthOutcome::BootstrapExchanged { device_id, .. } =
            exchange(&mgr, Some("u-alice"), "dev-a")
        else {
            unreachable!()
        };
        let authed = ConnectAuthOutcome::Authorized {
            device_id: Some(device_id.clone()),
        };
        assert_eq!(
            credential_provenance_device_id(&authed, Some(&mgr)).as_deref(),
            Some(device_id.as_str())
        );
        assert_eq!(
            handshake_verified(&authed, &mgr, &store).as_deref(),
            Some("u-alice")
        );
        // Credential-less implicit owner (loopback / shared token).
        let implicit = ConnectAuthOutcome::Authorized { device_id: None };
        assert_eq!(handshake_verified(&implicit, &mgr, &store), None);
        assert_eq!(
            handshake_verified(&ConnectAuthOutcome::Unauthorized, &mgr, &store),
            None
        );
    }
}

#[cfg(test)]
mod revalidate_credential_tests {
    use super::*;
    use crate::gateway::handlers::connect::ConnectAuthOutcome;
    use crate::gateway::security::store::{SecurityStore, UserRole};
    use crate::gateway::security::DeviceTokenManager;
    use serde_json::json;
    use std::sync::Arc;

    fn fixture() -> (Arc<SecurityStore>, DeviceTokenManager) {
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr = DeviceTokenManager::new(store.clone());
        (store, mgr)
    }

    fn pair(mgr: &DeviceTokenManager, did: &str) -> (String, String) {
        let ticket = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
        let r = mgr
            .exchange_bootstrap_ticket(&ticket, Some(did.to_string()), None, None)
            .unwrap();
        (r.device_id, r.device_token)
    }

    /// The full final chain, as run under the map write guard.
    fn final_verdict(
        outcome: &ConnectAuthOutcome,
        params: &serde_json::Value,
        mgr: &DeviceTokenManager,
        store: &SecurityStore,
        authed: Option<&str>,
    ) -> (Option<String>, &'static str, Option<String>) {
        let pre = !matches!(outcome, ConnectAuthOutcome::Unauthorized);
        let (ok, src) = revalidate_connect_credential_source(outcome, Some(params), Some(mgr));
        finalize_handshake_identity(pre && ok, false, authed, src.as_deref(), Some(store))
    }

    /// Same device id revoked then re-approved (`upsert_device` allows it).
    fn revoke_and_repair(mgr: &DeviceTokenManager, store: &SecurityStore, did: &str) -> String {
        mgr.revoke_device(did).unwrap();
        let (d2, tok2) = pair(mgr, did);
        assert_eq!(d2, did);
        assert!(store.is_device_approved(did).unwrap());
        tok2
    }

    #[test]
    fn old_device_token_after_revoke_and_repair_is_guest() {
        let (store, mgr) = fixture();
        let (did, old_tok) = pair(&mgr, "dev-a");
        let outcome = ConnectAuthOutcome::Authorized {
            device_id: Some(did.clone()),
        };
        let old_params = json!({ "device_token": old_tok });
        assert_eq!(
            final_verdict(&outcome, &old_params, &mgr, &store, Some(&did))
                .2
                .as_deref(),
            Some("u-alice")
        );
        let new_tok = revoke_and_repair(&mgr, &store, &did);
        // The stale handshake (old token) must not restore the identity.
        assert_eq!(
            final_verdict(&outcome, &old_params, &mgr, &store, Some(&did)),
            (None, "guest", None)
        );
        // A fresh handshake with the new token binds the active user.
        let new_params = json!({ "device_token": new_tok });
        assert_eq!(
            final_verdict(&outcome, &new_params, &mgr, &store, Some(&did))
                .2
                .as_deref(),
            Some("u-alice")
        );
    }

    #[test]
    fn bootstrap_issued_token_after_revoke_and_repair_is_guest() {
        let (store, mgr) = fixture();
        let (did, tok) = pair(&mgr, "dev-b");
        let outcome = ConnectAuthOutcome::BootstrapExchanged {
            device_id: did.clone(),
            device_token: tok,
        };
        let params = json!({ "bootstrap_ticket": "spent" });
        assert_eq!(
            final_verdict(&outcome, &params, &mgr, &store, Some(&did))
                .2
                .as_deref(),
            Some("u-alice")
        );
        let new_tok = revoke_and_repair(&mgr, &store, &did);
        assert_eq!(
            final_verdict(&outcome, &params, &mgr, &store, Some(&did)),
            (None, "guest", None)
        );
        let fresh = ConnectAuthOutcome::BootstrapExchanged {
            device_id: did.clone(),
            device_token: new_tok,
        };
        assert_eq!(
            final_verdict(&fresh, &params, &mgr, &store, Some(&did))
                .2
                .as_deref(),
            Some("u-alice")
        );
    }

    #[test]
    fn revalidate_fail_closed_shapes_and_compat() {
        let (_store, mgr) = fixture();
        let (did, tok) = pair(&mgr, "dev-c");
        let authed = ConnectAuthOutcome::Authorized {
            device_id: Some(did.clone()),
        };
        let good = json!({ "device_token": tok });
        assert_eq!(
            revalidate_connect_credential_source(&authed, Some(&good), Some(&mgr)),
            (true, Some(did.clone()))
        );
        // No params / blank / wrong token / no manager => closed.
        for p in [
            None,
            Some(json!({})),
            Some(json!({ "device_token": "" })),
            Some(json!({ "device_token": "nope" })),
        ] {
            assert_eq!(
                revalidate_connect_credential_source(&authed, p.as_ref(), Some(&mgr)),
                (false, None)
            );
        }
        assert_eq!(
            revalidate_connect_credential_source(&authed, Some(&good), None),
            (false, None)
        );
        // Token validates, but to a different device than the outcome claims.
        let (_d2, tok2) = pair(&mgr, "dev-d");
        let mismatch = json!({ "device_token": tok2 });
        assert_eq!(
            revalidate_connect_credential_source(&authed, Some(&mismatch), Some(&mgr)),
            (false, None)
        );
        // Credential-less compat and unauthorized.
        let implicit = ConnectAuthOutcome::Authorized { device_id: None };
        assert_eq!(
            revalidate_connect_credential_source(&implicit, None, None),
            (true, None)
        );
        assert_eq!(
            revalidate_connect_credential_source(
                &ConnectAuthOutcome::Unauthorized,
                Some(&good),
                Some(&mgr)
            ),
            (false, None)
        );
    }
}
