//! Per-request authorization for the MCP face (Streamable HTTP `/mcp`).
//!
//! Applies the same credential rules the WebSocket `connect` handler uses —
//! [`resolve_connect_auth`] for the credential verdict and
//! [`resolve_connection_identity`] for the `(user, role)` pair — mapped to a
//! single synchronous function that returns a typed [`McpCaller`] or an
//! [`AuthRefusal`].
//!
//! The wall rule is deliberately borrowed from
//! [`wall_admits`](crate::gateway::server::connection::auth::wall_admits) so
//! any future change to that predicate propagates here automatically.

use crate::gateway::handlers::connect::{
    resolve_connect_auth, resolve_connection_identity, ConnectAuthOutcome,
};
use crate::gateway::openai_api::auth::extract_bearer_token;
use crate::gateway::security::{DeviceTokenManager, SecurityStore};
use crate::gateway::server::connection::auth::wall_admits;
use crate::gateway::trusted_proxy::ResolvedClient;
use crate::sync_primitives::Arc;
use axum::http::HeaderMap;

/// An opaque validator for the shared-token credential.
///
/// Wraps
/// [`SharedTokenManager::global_validate`](crate::gateway::security::SharedTokenManager::global_validate)
/// so callers can store and clone it without knowing the concrete type.
pub type SharedTokenValidator = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The resolved identity of an authorized MCP caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCaller {
    /// Wire role: `"operator"` or `"member"`.  Guest and no-role are rejected
    /// by the wall check before this struct is returned.
    pub role: &'static str,
    /// Resolved user id, or `None` for legacy/unbound credentials.
    pub user: Option<String>,
    /// `true` if the request arrived on a loopback interface.
    pub is_local: bool,
    /// Paired device id.  `None` for shared-token and loopback callers.
    pub device_id: Option<String>,
}

impl McpCaller {
    /// Whether this caller carries operator (config-tier) authority.
    ///
    /// The same predicate the gateway uses for a WS caller —
    /// [`role_is_operator`](crate::tools::turn_context::role_is_operator) — so
    /// an MCP call and a WS call with the same `(user, role)` are judged alike.
    #[must_use]
    pub fn is_operator(&self) -> bool {
        crate::tools::turn_context::role_is_operator(Some(self.role))
    }
}

/// Why the request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRefusal {
    /// No credential was supplied on a non-loopback connection.
    NoCredential,
    /// A credential was supplied but it was not accepted by any validator.
    BadCredential,
    /// The credential was accepted but the resolved role is not admitted past
    /// the multi-user wall (guest or no role).
    Walled,
}

/// Authorize a single MCP HTTP request.
///
/// # Arguments
/// - `headers` — the request's HTTP headers.
/// - `client` — the resolved client address (loopback flag, IP, secure flag).
/// - `device_tokens` — the manager that holds current device-token state.
/// - `store` — the security store for identity resolution.
/// - `validate_shared` — shared-token validator; returns `true` if the token
///   is currently valid.
///
/// # Returns
/// `Ok(McpCaller)` when the request is authorized, `Err(AuthRefusal)` when
/// it is not.
pub fn authorize(
    headers: &HeaderMap,
    client: ResolvedClient,
    device_tokens: &DeviceTokenManager,
    store: &SecurityStore,
    validate_shared: &dyn Fn(&str) -> bool,
) -> Result<McpCaller, AuthRefusal> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer_token);

    let outcome = resolve_connect_auth(
        client.local,
        bearer, // shared_token slot
        bearer, // device_token slot
        None,   // no bootstrap ticket on the stateless HTTP path
        None,   // device_id not supplied via headers
        None,   // device_name not supplied via headers
        |t| validate_shared(t),
        device_tokens,
    );

    let device_id = match outcome {
        ConnectAuthOutcome::Authorized { device_id } => device_id,
        ConnectAuthOutcome::BootstrapExchanged { .. } => {
            // Bootstrap exchange requires a stateful round-trip that the
            // stateless HTTP face does not support.  A ticket-shaped bearer
            // is therefore an unrecognised credential here.
            return Err(AuthRefusal::BadCredential);
        }
        ConnectAuthOutcome::Unauthorized => {
            return Err(if bearer.is_some() {
                AuthRefusal::BadCredential
            } else {
                AuthRefusal::NoCredential
            });
        }
    };

    let (user, role) = resolve_connection_identity(client.local, device_id.as_deref(), store);

    // Apply the same wall rule the WebSocket path uses.  `wall_admits` is the
    // single authoritative derivation; we never re-derive the predicate here.
    if !wall_admits(Some(role), "") {
        return Err(AuthRefusal::Walled);
    }

    Ok(McpCaller {
        role,
        user,
        is_local: client.local,
        device_id,
    })
}

/// Build the shared-token validator used in production.
///
/// Delegates to [`SharedTokenManager::global_validate`], the ONE derivation of
/// "is this the shared gateway token". The WebSocket `connect` arm calls that
/// function directly at both of its dispatch stations, so this closure is not a
/// third copy of the predicate — it is the same predicate behind an
/// [`Arc`]-erased handle the MCP route state can own.
pub fn production_shared_token_validator() -> SharedTokenValidator {
    Arc::new(|t: &str| crate::gateway::security::SharedTokenManager::global_validate(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::security::store::{UserRole, UserStatus, OWNER_USER_ID};
    use axum::http::header::AUTHORIZATION;

    fn store() -> Arc<SecurityStore> {
        Arc::new(SecurityStore::in_memory().unwrap())
    }

    fn headers(bearer: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(t) = bearer {
            h.insert(AUTHORIZATION, format!("Bearer {t}").parse().unwrap());
        }
        h
    }

    fn local() -> ResolvedClient {
        ResolvedClient {
            ip: "127.0.0.1".parse().unwrap(),
            secure: false,
            local: true,
        }
    }

    fn remote() -> ResolvedClient {
        ResolvedClient {
            ip: "203.0.113.7".parse().unwrap(),
            secure: true,
            local: false,
        }
    }

    #[test]
    fn loopback_needs_no_credential_and_is_the_owner_operator() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        let caller = authorize(&headers(None), local(), &mgr, &store, &|_| false).unwrap();
        assert_eq!(caller.role, "operator");
        assert_eq!(caller.user.as_deref(), Some(OWNER_USER_ID));
        assert!(caller.is_local);
        assert!(caller.device_id.is_none());
    }

    #[test]
    fn remote_without_a_bearer_is_refused_as_no_credential() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        assert_eq!(
            authorize(&headers(None), remote(), &mgr, &store, &|_| false),
            Err(AuthRefusal::NoCredential),
        );
    }

    #[test]
    fn remote_with_a_bearer_nobody_accepts_is_refused_as_bad_credential() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        assert_eq!(
            authorize(
                &headers(Some("aleph-nope")),
                remote(),
                &mgr,
                &store,
                &|_| false
            ),
            Err(AuthRefusal::BadCredential),
        );
    }

    #[test]
    fn remote_with_the_shared_token_is_the_owner_operator() {
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        let caller = authorize(&headers(Some("aleph-good")), remote(), &mgr, &store, &|t| {
            t == "aleph-good"
        })
        .unwrap();
        assert_eq!(caller.role, "operator");
        assert_eq!(caller.user.as_deref(), Some(OWNER_USER_ID));
        assert!(!caller.is_local);
        assert!(caller.device_id.is_none());
    }

    #[test]
    fn remote_with_a_member_bound_device_token_is_that_member() {
        let store = store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr = DeviceTokenManager::new(store.clone());
        let ticket = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
        let issued = mgr
            .exchange_bootstrap_ticket(
                &ticket,
                Some("dev-pi".to_string()),
                Some("pi".to_string()),
                None,
            )
            .unwrap();
        let caller = authorize(
            &headers(Some(&issued.device_token)),
            remote(),
            &mgr,
            &store,
            &|_| false,
        )
        .unwrap();
        assert_eq!(caller.role, "member");
        assert_eq!(caller.user.as_deref(), Some("u-alice"));
        assert_eq!(caller.device_id.as_deref(), Some("dev-pi"));
    }

    #[test]
    fn a_ticket_shaped_bearer_never_mints_a_device_token() {
        // A bootstrap ticket presented as an HTTP bearer must not trigger
        // exchange — the ticket exchange path requires a round-trip the
        // stateless HTTP face does not support.
        let store = store();
        let mgr = DeviceTokenManager::new(store.clone());
        let ticket = mgr.create_bootstrap_ticket(None, None).unwrap();
        assert_eq!(
            authorize(&headers(Some(&ticket)), remote(), &mgr, &store, &|_| false),
            Err(AuthRefusal::BadCredential),
        );
        // No device was registered as a side-effect of the failed attempt.
        assert!(mgr.list_panel_devices().unwrap().is_empty());
    }

    #[test]
    fn a_valid_though_deactivated_device_is_walled() {
        let store = store();
        store
            .create_user("u-alice", "Alice", UserRole::Member)
            .unwrap();
        let mgr = DeviceTokenManager::new(store.clone());
        let ticket = mgr.create_bootstrap_ticket(None, Some("u-alice")).unwrap();
        let issued = mgr
            .exchange_bootstrap_ticket(
                &ticket,
                Some("dev-pi".to_string()),
                Some("pi".to_string()),
                None,
            )
            .unwrap();
        // The credential itself stays valid; the *identity* behind it is what
        // gets walled when the user is deactivated.
        store
            .update_user("u-alice", None, None, Some(UserStatus::Deactivated))
            .unwrap();
        assert_eq!(
            authorize(
                &headers(Some(&issued.device_token)),
                remote(),
                &mgr,
                &store,
                &|_| false,
            ),
            Err(AuthRefusal::Walled),
        );
    }
}
