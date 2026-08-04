//! Connect handler — session handshake + Gateway-token authorization seam.
//!
//! Transport model: the Panel connects over plain WS (same-origin HTTP),
//! identical to a browser opening the core's LAN IP — there is no channel
//! pipeline and no shell-only shortcut. Authorization supports three
//! credentials, in priority order:
//!
//! 1. **Loopback** (the local desktop App / same machine) is always authorized
//!    as operator — zero-config, no token required.
//! 2. **Device token** (`device_token` in `connect` params): a long-lived token
//!    issued to a previously paired remote device. Valid ⇒ operator.
//! 3. **Bootstrap ticket** (`bootstrap_ticket` in `connect` params): a short-lived,
//!    single-use code exchanged for a fresh device token during onboarding.
//! 4. **Legacy shared Gateway token** (`token` in `connect` params): the original
//!    single shared token. Kept for backward compatibility.
//!
//! A missing/invalid credential leaves the connection unauthorized: the WS
//! dispatch login wall refuses everything but `connect`, and the Panel renders
//! a token box.
//!
//! `handle_connect` returns only the session baseline. The actual authorization
//! decision needs the per-connection client IP (loopback?) and access to the
//! token managers, both of which live in `server::handler`; it calls
//! [`resolve_connect_auth`] at the handshake and stamps the resolved role onto
//! the connection state.

use crate::sync_primitives::Arc;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::security::{DeviceTokenError, DeviceTokenManager};

/// Context for the connect handshake.
pub struct ConnectContext {
    /// Monotonic state-version tracker, shared with `GatewayServer`.
    /// Surfaced in the `connect` success response so clients capture a
    /// baseline snapshot at handshake time.
    pub state_versions: Arc<crate::gateway::state_version::StateVersionTracker>,
    /// Transport keep-alive policy returned to clients at handshake time.
    /// Sourced from `GatewayConfig::{ping_interval_secs, idle_timeout_secs}`
    /// so client and server agree on the live cadence.
    pub transport_policy: crate::gateway::handlers::auth::TransportPolicy,
}

/// Authorization decision for a WS handshake under the Gateway-token model.
///
/// Loopback is always authorized (zero-config operator). Remote connections
/// are authorized by (in order): a valid device token, a valid bootstrap ticket
/// exchanged for a new device token, or the legacy shared Gateway token.
#[derive(Debug, Clone)]
pub enum ConnectAuthOutcome {
    /// Authorized as operator. `device_id` carries the paired device's id when
    /// authorization came from a valid device token, so the session can be
    /// attributed in presence and the device's `last_seen_at` refreshed. It is
    /// `None` for loopback and the legacy shared-token path, which are not bound
    /// to a specific paired device.
    Authorized { device_id: Option<String> },
    /// Unauthorized — login wall.
    Unauthorized,
    /// Bootstrap ticket accepted and a new device token was issued.
    /// The plaintext token must be echoed to the client in the connect response.
    BootstrapExchanged {
        device_id: String,
        device_token: String,
    },
}

/// Resolve authorization for a `connect` handshake.
///
/// Priority:
/// 1. loopback ⇒ authorized
/// 2. `device_token` valid ⇒ authorized
/// 3. `bootstrap_ticket` valid ⇒ authorized + issue new device token
/// 4. legacy `shared_token` valid ⇒ authorized
/// 5. otherwise ⇒ unauthorized
#[must_use]
pub fn resolve_connect_auth(
    is_loopback: bool,
    shared_token: Option<&str>,
    device_token: Option<&str>,
    bootstrap_ticket: Option<&str>,
    device_id: Option<&str>,
    device_name: Option<&str>,
    validate_shared_token: impl Fn(&str) -> bool,
    device_token_mgr: &DeviceTokenManager,
) -> ConnectAuthOutcome {
    if is_loopback {
        return ConnectAuthOutcome::Authorized { device_id: None };
    }

    // 1. Device token.
    if let Some(token) = device_token {
        if !token.is_empty() {
            match device_token_mgr.validate_device_token(token) {
                Ok(Some(row)) => {
                    return ConnectAuthOutcome::Authorized {
                        device_id: Some(row.device_id),
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!("device token validation failed: {}", e);
                }
            }
        }
    }

    // 2. Bootstrap ticket.
    if let Some(ticket) = bootstrap_ticket {
        if !ticket.is_empty() {
            match device_token_mgr.exchange_bootstrap_ticket(
                ticket,
                device_id.map(String::from),
                device_name.map(String::from),
                None,
            ) {
                Ok(result) => {
                    return ConnectAuthOutcome::BootstrapExchanged {
                        device_id: result.device_id,
                        device_token: result.device_token,
                    };
                }
                Err(DeviceTokenError::InvalidBootstrapTicket) => {
                    tracing::debug!("bootstrap ticket rejected");
                }
                Err(e) => {
                    tracing::warn!("bootstrap ticket exchange failed: {}", e);
                }
            }
        }
    }

    // 3. Legacy shared token.
    if let Some(token) = shared_token {
        if !token.is_empty() && validate_shared_token(token) {
            return ConnectAuthOutcome::Authorized { device_id: None };
        }
    }

    ConnectAuthOutcome::Unauthorized
}

/// Authorization decision for a WS handshake under the Gateway-token model.
///
/// Loopback is always authorized (zero-config operator). A remote connection
/// is authorized only when it presents a non-empty token that `validate`
/// accepts. `validate` is injected so this stays a pure, host-testable
/// predicate — production passes a closure over
/// `SharedTokenManager::global().validate`.
#[must_use]
pub fn connect_authorized(
    is_loopback: bool,
    token: Option<&str>,
    validate: impl Fn(&str) -> bool,
) -> bool {
    if is_loopback {
        return true;
    }
    matches!(token, Some(t) if !t.is_empty() && validate(t))
}

/// Resolve the (user, caller_role) pair for an authorized connection.
///
/// Rules (spec §4):
/// - loopback ⇒ implicit owner, operator (zero-config, unchanged)
/// - device token bound to a user ⇒ that user; role admin⇒"operator",
///   member⇒"member"; deactivated ⇒ walled ("guest", no user)
/// - authorized but unbound (legacy shared token, pre-v14 device — the device
///   row carries no `user_id`) ⇒ implicit owner, operator — the zero-change
///   guarantee for existing deployments
/// - fail-closed: a store error on either the device→user or the user lookup,
///   or a device's `user_id` pointing at a row no longer present in `users`
///   (dangling reference), walls the connection ("guest", no user) instead of
///   defaulting to owner. A lookup we could not actually perform, or a link
///   we know is broken, must never silently grant full authority — the
///   connection can simply reconnect once the store is healthy again.
#[must_use]
pub fn resolve_connection_identity(
    is_loopback: bool,
    device_id: Option<&str>,
    store: &crate::gateway::security::store::SecurityStore,
) -> (Option<String>, &'static str) {
    use crate::gateway::security::store::{UserRole, UserStatus, OWNER_USER_ID};

    if is_loopback {
        return (Some(OWNER_USER_ID.to_string()), "operator");
    }

    let Some(device_id) = device_id else {
        // No device at all (legacy shared token): unbound, zero-change guarantee.
        return (Some(OWNER_USER_ID.to_string()), "operator");
    };

    let linked_user_id = match store.device_user(device_id) {
        // Device row exists but carries no user_id: genuinely unbound
        // (pre-v14 device, not yet adopted) — zero-change guarantee.
        Ok(None) => return (Some(OWNER_USER_ID.to_string()), "operator"),
        Ok(Some(uid)) => uid,
        // Store error: fail-closed. We could not actually determine whether
        // this device is bound, so never default to owner.
        Err(_) => return (None, "guest"),
    };

    match store.get_user(&linked_user_id) {
        Ok(Some(u)) if u.status == UserStatus::Deactivated => (None, "guest"),
        Ok(Some(u)) => {
            let role = match u.role {
                UserRole::Admin => "operator",
                UserRole::Member => "member",
            };
            (Some(u.user_id), role)
        }
        // Dangling user_id (points at a row no longer in `users`), or a
        // store error on this lookup: fail-closed, never default to owner.
        Ok(None) | Err(_) => (None, "guest"),
    }
}

/// Whether a resolved `connect` outcome should be recorded as an
/// [`AuditEventType::AuthFailure`](crate::security::audit::AuditEventType) in
/// the security audit log.
///
/// Only a rejected **remote** connect is auditable: loopback is the zero-config
/// operator and its connects are never logged (the local user is not an
/// intruder, and logging every local attach is noise/privacy). An authorized
/// remote connect is a success, not a failure. Kept as a pure predicate so the
/// "loopback is never audited" invariant is host-testable and regression-guarded.
#[must_use]
pub const fn should_audit_connect_failure(authorized: bool, is_loopback: bool) -> bool {
    !authorized && !is_loopback
}

/// Handle "connect" — returns the session baseline. `server::handler` overlays
/// the authorization verdict (`role` / `authorized` / `needs_token`) computed
/// via [`resolve_connect_auth`]; the `role` here is just a default for any path
/// that bypasses that overlay.
pub async fn handle_connect(request: JsonRpcRequest, ctx: Arc<ConnectContext>) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "role": "operator",
            "state_version": ctx.state_versions.snapshot(),
            "keepalive": ctx.transport_policy.clone(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::TransportPolicy;
    use crate::gateway::security::store::{SecurityStore, UserRole, UserStatus};

    fn ctx() -> Arc<ConnectContext> {
        Arc::new(ConnectContext {
            state_versions: Arc::new(crate::gateway::state_version::StateVersionTracker::new()),
            transport_policy: TransportPolicy::defaults(),
        })
    }

    fn mgr() -> DeviceTokenManager {
        DeviceTokenManager::new(Arc::new(
            crate::gateway::security::SecurityStore::in_memory().unwrap(),
        ))
    }

    /// `SecurityStore::in_memory()` runs migrations, including owner
    /// bootstrap, so a fresh store already contains the owner user.
    fn seeded_store() -> SecurityStore {
        SecurityStore::in_memory().unwrap()
    }

    /// Upsert a panel device and bind it to `user_id` — `DeviceUpsertData`
    /// doesn't carry `user_id` yet (that lands in a later task), so the
    /// binding is a separate write via the test-only `set_device_user`.
    fn upsert_panel_device(store: &SecurityStore, device_id: &str, user_id: &str) {
        store
            .upsert_device(&crate::gateway::security::store::DeviceUpsertData {
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

    #[test]
    fn loopback_is_always_authorized() {
        let mgr = mgr();
        // Loopback is authorized but not device-bound: device_id stays None.
        assert!(matches!(
            resolve_connect_auth(true, None, None, None, None, None, |_| false, &mgr),
            ConnectAuthOutcome::Authorized { device_id: None }
        ));
        assert!(matches!(
            resolve_connect_auth(true, Some(""), None, None, None, None, |_| false, &mgr),
            ConnectAuthOutcome::Authorized { device_id: None }
        ));
    }

    #[test]
    fn remote_requires_a_valid_shared_token() {
        let mgr = mgr();
        let valid = |t: &str| t == "aleph-good";
        assert!(matches!(
            resolve_connect_auth(
                false,
                Some("aleph-good"),
                None,
                None,
                None,
                None,
                valid,
                &mgr
            ),
            ConnectAuthOutcome::Authorized { device_id: None }
        ));
        assert!(matches!(
            resolve_connect_auth(
                false,
                Some("aleph-bad"),
                None,
                None,
                None,
                None,
                valid,
                &mgr
            ),
            ConnectAuthOutcome::Unauthorized
        ));
        assert!(matches!(
            resolve_connect_auth(false, Some(""), None, None, None, None, |_| true, &mgr),
            ConnectAuthOutcome::Unauthorized
        ));
        assert!(matches!(
            resolve_connect_auth(false, None, None, None, None, None, |_| true, &mgr),
            ConnectAuthOutcome::Unauthorized
        ));
    }

    #[tokio::test]
    async fn bare_connect_returns_session_baseline() {
        let req = JsonRpcRequest::with_id("connect", Some(json!({})), json!(1));
        let resp = handle_connect(req, ctx()).await;
        assert!(resp.is_success(), "{resp:?}");
        let result = resp.result.unwrap();
        // Baseline role; the real verdict is overlaid by server::handler.
        assert_eq!(
            result.get("role").and_then(|v| v.as_str()),
            Some("operator")
        );
        assert!(result.get("state_version").is_some());
        assert!(result.get("keepalive").is_some());
    }

    #[test]
    fn bootstrap_ticket_exchanges_for_device_token() {
        let mgr = mgr();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();

        let outcome = resolve_connect_auth(
            false,
            None,
            None,
            Some(&ticket),
            Some("dev-1"),
            Some("Panel"),
            |_| false,
            &mgr,
        );

        assert!(
            matches!(outcome, ConnectAuthOutcome::BootstrapExchanged { .. }),
            "expected BootstrapExchanged, got {outcome:?}"
        );

        // Same ticket cannot be reused.
        let outcome2 = resolve_connect_auth(
            false,
            None,
            None,
            Some(&ticket),
            Some("dev-1"),
            Some("Panel"),
            |_| false,
            &mgr,
        );
        assert!(matches!(outcome2, ConnectAuthOutcome::Unauthorized));
    }

    #[test]
    fn only_remote_failures_are_audited() {
        // Loopback is the zero-config operator: never audited, authorized or not.
        assert!(!should_audit_connect_failure(false, true));
        assert!(!should_audit_connect_failure(true, true));
        // Authorized remote is a success, not a failure.
        assert!(!should_audit_connect_failure(true, false));
        // Rejected remote connect is the one auditable case.
        assert!(should_audit_connect_failure(false, false));
    }

    #[test]
    fn device_token_authorizes_after_exchange() {
        let mgr = mgr();
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();

        let ConnectAuthOutcome::BootstrapExchanged { device_token, .. } = resolve_connect_auth(
            false,
            None,
            None,
            Some(&ticket),
            Some("dev-1"),
            Some("Panel"),
            |_| false,
            &mgr,
        ) else {
            panic!("expected bootstrap exchange");
        };

        let outcome = resolve_connect_auth(
            false,
            None,
            Some(&device_token),
            None,
            Some("dev-1"),
            None,
            |_| false,
            &mgr,
        );
        // C5: the validated device's id is threaded through, not discarded, so
        // presence/roster can attribute the reconnect.
        assert!(
            matches!(outcome, ConnectAuthOutcome::Authorized { device_id: Some(ref d) } if d == "dev-1"),
            "expected Authorized with device_id dev-1, got {outcome:?}"
        );
    }

    #[test]
    fn loopback_resolves_to_owner_operator() {
        let store = seeded_store(); // in_memory + ensure_bootstrap_owner (ran in migrate)
        let (user, role) = resolve_connection_identity(true, None, &store);
        assert_eq!(user.as_deref(), Some(crate::gateway::security::store::OWNER_USER_ID));
        assert_eq!(role, "operator");
    }

    #[test]
    fn device_of_member_user_resolves_member_role() {
        let store = seeded_store();
        store.create_user("u-alice", "Alice", UserRole::Member).unwrap();
        upsert_panel_device(&store, "dev-a", "u-alice"); // fixture: upsert + set user_id
        let (user, role) = resolve_connection_identity(false, Some("dev-a"), &store);
        assert_eq!(user.as_deref(), Some("u-alice"));
        assert_eq!(role, "member");
    }

    #[test]
    fn device_of_admin_user_resolves_operator_role() {
        let store = seeded_store();
        store.create_user("u-root", "Root", UserRole::Admin).unwrap();
        upsert_panel_device(&store, "dev-r", "u-root");
        let (user, role) = resolve_connection_identity(false, Some("dev-r"), &store);
        assert_eq!(user.as_deref(), Some("u-root"));
        assert_eq!(role, "operator");
    }

    #[test]
    fn dangling_user_link_resolves_guest() {
        // Device row's user_id points at a user that was never created (or
        // was deleted since) — fail-closed, never a silent grant of owner
        // authority just because the link is broken.
        let store = seeded_store();
        upsert_panel_device(&store, "dev-ghost", "u-never-created");
        let (user, role) = resolve_connection_identity(false, Some("dev-ghost"), &store);
        assert_eq!(user, None);
        assert_eq!(role, "guest");
    }

    #[test]
    fn deactivated_user_resolves_guest() {
        let store = seeded_store();
        store.create_user("u-gone", "Gone", UserRole::Member).unwrap();
        store.update_user("u-gone", None, None, Some(UserStatus::Deactivated)).unwrap();
        upsert_panel_device(&store, "dev-g", "u-gone");
        let (user, role) = resolve_connection_identity(false, Some("dev-g"), &store);
        assert_eq!(user, None);
        assert_eq!(role, "guest"); // walled — deactivation takes effect at next connect
    }

    #[test]
    fn unlinked_device_and_shared_token_fall_back_to_owner() {
        // Legacy shared-token / unlinked-device connections keep today's behavior:
        // full operator as the implicit owner (zero-change guarantee).
        let store = seeded_store();
        let (user, role) = resolve_connection_identity(false, None, &store);
        assert_eq!(user.as_deref(), Some(crate::gateway::security::store::OWNER_USER_ID));
        assert_eq!(role, "operator");
    }

    // ── Task 7: P0 acceptance guards (spec §8, "单用户体验零变化") ──────────

    #[test]
    fn zero_change_loopback_is_owner_operator_on_fresh_store() {
        // The single-user guarantee: a fresh (or migrated) deployment touching
        // nothing new behaves exactly as before — loopback is a full operator.
        let store = seeded_store();
        let (user, role) = resolve_connection_identity(true, None, &store);
        assert_eq!(role, "operator");
        assert_eq!(
            user.as_deref(),
            Some(crate::gateway::security::store::OWNER_USER_ID)
        );
    }

    #[test]
    fn zero_change_admin_gate_is_inert_for_operator_and_internal() {
        // operator (single-user default) and None (cron/internal) pass every method.
        for m in ["gateway.token.rotate", "users.create", "providers.update"] {
            assert!(crate::gateway::method_admin::method_requires_admin(m));
        }
        // The gate predicate refuses ONLY Some("member") — asserted at the
        // chokepoint test in Task 4; here we pin the classifier side.
        assert!(!crate::gateway::method_admin::method_requires_admin("chat.send"));
    }
}
