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
        let ticket = mgr.create_bootstrap_ticket(None).unwrap();

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
        let ticket = mgr.create_bootstrap_ticket(None).unwrap();

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
}
