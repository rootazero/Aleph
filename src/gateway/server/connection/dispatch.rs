//! Dispatch phase — JSON-RPC request parsing + middleware invocation.
//!
//! Two functions live here, called by both dispatch stations inside the
//! `handle_connection` `tokio::select!` loop:
//!
//! - `dispatch_with_caller_context`: scopes the four caller-identity task-
//!   locals (role / user / is-loopback / conn-id) plus a P1 personal-scope
//!   attribution around `process_request`. Single source of truth so the two
//!   stations cannot drift apart.
//! - `process_request`: parses the JSON-RPC envelope, applies the
//!   per-method admin gate, runs the W3C `traceparent` instrumentation, then
//!   delegates to the `MiddlewareChain`.

use std::collections::HashMap;

use tokio::sync::RwLock;

use crate::gateway::middleware::MiddlewareChain;
use crate::gateway::panel_project_context::{PanelCallerContext, PanelRequestContext};
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, ADMIN_REQUIRED_MESSAGE, AUTH_REQUIRED, PARSE_ERROR,
    PERMISSION_DENIED,
};
use crate::gateway::server::ConnectionState;

/// Scope `process_request` with the caller-identity task-locals + P1 scope
/// attribution that must surround every dispatched request. Single source of
/// truth shared by both dispatch stations (`do_lane_dispatch`'s closure and
/// the idempotency `Proceed` arm) so the two call sites cannot drift apart —
/// see `src/gateway/CLAUDE.md`'s note that `CALLER_ROLE`/`CALLER_USER`/
/// `CALLER_IS_LOOPBACK`/`CALLER_CONN_ID` must be scoped around
/// `process_request` at both sites. `scope::with_scope` is the outermost
/// (4th) layer: a `caller_user` seeds a personal-scope attribution,
/// observable via `scope::current_scope` for the lifetime of this dispatch
/// (spec P1 §5).
///
/// `caller_verified_user` is the handshake-proved user (see
/// `CALLER_VERIFIED_USER`); `None` for every connection that did not prove one.
pub async fn dispatch_with_caller_context(
    text: &str,
    mc: &MiddlewareChain,
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
    caller_verified_user: Option<String>,
) -> String {
    dispatch_snapshot(
        text,
        mc,
        CallerSnapshot {
            role: caller_role,
            user: caller_user,
            verified_user: caller_verified_user,
            device_id: None,
        },
        caller_is_loopback,
        caller_conn_id,
    )
    .await
}

async fn dispatch_snapshot(
    text: &str,
    mc: &MiddlewareChain,
    snapshot: CallerSnapshot,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
) -> String {
    let context = mc.panel_context(PanelCallerContext::authenticated(
        viewer_principal(&snapshot),
        snapshot.device_id.clone(),
    ));
    crate::gateway::caller_identity::with_verified_caller_identity(
        snapshot.role.clone(),
        snapshot.user.clone(),
        caller_is_loopback,
        caller_conn_id,
        snapshot.verified_user.clone(),
        process_request_with_panel_context(text, mc, context, Some(&snapshot)),
    )
    .await
}

/// One consistent read of a connection's per-request identity (role, user and
/// handshake-verified user all come from the SAME `ConnectionState` read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerSnapshot {
    pub role: Option<String>,
    pub user: Option<String>,
    pub verified_user: Option<String>,
    pub device_id: Option<String>,
}

impl CallerSnapshot {
    /// Read the live connection state. A missing entry (pre-handshake, probe
    /// or already torn down) defaults by network position exactly as the old
    /// per-field reads did -- loopback operator/owner, remote guest -- and
    /// NEVER carries a verified user (no fallback).
    pub async fn read(
        connections: &RwLock<HashMap<String, ConnectionState>>,
        conn_id: &str,
        client_is_local: bool,
    ) -> Self {
        let conns = connections.read().await;
        match conns.get(conn_id) {
            Some(s) => Self {
                role: Some(s.caller_role.clone()),
                user: s.caller_user.clone(),
                verified_user: s.verified_user.clone(),
                device_id: s.device_id.clone(),
            },
            None => Self {
                role: Some(if client_is_local { "operator" } else { "guest" }.to_string()),
                user: client_is_local
                    .then(|| crate::gateway::security::store::OWNER_USER_ID.to_string()),
                verified_user: None,
                device_id: None,
            },
        }
    }
}

/// RPC methods that mint/revoke/decide/list per-user browser viewer
/// capabilities and resources. They require a handshake-VERIFIED user, not
/// the stamped (possibly implicit-owner) one.
pub const BROWSER_VIEWER_METHODS: [&str; 4] = [
    "browser.viewer.grant.issue",
    "browser.viewer.grant.revoke",
    "browser.viewer.resource.decide",
    "browser.viewer.resource.list",
];

#[must_use]
pub fn is_browser_viewer_method(method: &str) -> bool {
    BROWSER_VIEWER_METHODS.contains(&method)
}

/// The single viewer-authority predicate (pure). `Some(verified)` iff the role
/// is member/operator, the stamped user is non-empty, and the verified user is
/// non-empty and equal to it. Mirrors
/// `caller_identity::current_caller_verified_user`.
#[must_use]
pub fn viewer_principal(snapshot: &CallerSnapshot) -> Option<String> {
    crate::gateway::caller_identity::verified_caller_user(
        snapshot.role.as_deref(),
        snapshot.user.as_deref(),
        snapshot.verified_user.as_deref(),
    )
}

/// Generic denial -- no credential, user or reason echoed
#[must_use]
pub fn viewer_denied_response(id: Option<serde_json::Value>) -> String {
    serde_json::to_string(&JsonRpcResponse::error(id, PERMISSION_DENIED, "denied"))
        .unwrap_or_default()
}
/// Canonical login/admin denial under the currently scoped caller identity.
fn authorization_error(method: &str, id: Option<serde_json::Value>) -> Option<String> {
    let role = crate::gateway::caller_identity::current_caller_role();
    let message = if !super::auth::wall_admits(role.as_deref(), method) {
        super::auth::LOGIN_WALL_MESSAGE
    } else if crate::gateway::method_admin::method_requires_admin(method)
        && crate::gateway::caller_identity::caller_is_member()
    {
        ADMIN_REQUIRED_MESSAGE
    } else {
        return None;
    };
    Some(
        serde_json::to_string(&JsonRpcResponse::error(id, AUTH_REQUIRED, message))
            .unwrap_or_default(),
    )
}

/// Re-validate login, admin authority and identity against LIVE connection state.
/// Used before cached/waiting results return and after a lane permit is granted.
/// Viewer methods additionally require the same handshake-verified principal.
pub async fn revalidate_for_dispatch(
    connections: &RwLock<HashMap<String, ConnectionState>>,
    conn_id: &str,
    client_is_local: bool,
    method: &str,
    start: &CallerSnapshot,
) -> Option<CallerSnapshot> {
    let live = CallerSnapshot::read(connections, conn_id, client_is_local).await;
    let admitted = crate::gateway::caller_identity::with_caller_identity(
        live.role.clone(),
        live.user.clone(),
        client_is_local,
        Some(conn_id.to_string()),
        async { authorization_error(method, None).is_none() },
    )
    .await;
    if !admitted || live.user != start.user {
        return None;
    }
    if crate::gateway::handlers::panel_capabilities::is_context_method(method) {
        return (viewer_principal(&live) == viewer_principal(start)
            && live.device_id == start.device_id)
            .then_some(live);
    }
    if !super::auth::wall_admits(live.role.as_deref(), method) {
        return None;
    }
    if !is_browser_viewer_method(method) {
        return Some(live);
    }
    let expected = viewer_principal(start)?;
    (viewer_principal(&live).as_deref() == Some(expected.as_str())).then_some(live)
}

/// UI effects may wait outside the connection loop. Never use the loopback
/// fallback for a missing/closed connection, nor execute under a changed login.
/// The record and its identity are read under one lock after queue/permit waits.
pub(super) async fn live_ui_caller(
    connections: &RwLock<HashMap<String, ConnectionState>>,
    conn_id: &str,
    start: &CallerSnapshot,
) -> Option<CallerSnapshot> {
    let connections = connections.read().await;
    let state = connections.get(conn_id)?;
    if state.first_message || !super::auth::wall_admits(Some(&state.caller_role), "capabilities.ui.invoke") {
        return None;
    }
    let live = CallerSnapshot {
        role: Some(state.caller_role.clone()),
        user: state.caller_user.clone(),
        verified_user: state.verified_user.clone(),
        device_id: state.device_id.clone(),
    };
    (live.user.as_deref().is_some_and(|user| !user.is_empty()) && &live == start).then_some(live)
}

/// Dispatch one request for a connection: revalidate live authority, then
/// run `dispatch_with_caller_context` with the resulting snapshot. Single
/// helper shared by `do_lane_dispatch` and the idempotency `Proceed` arm so
/// both real dispatch paths gate and scope identically.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_for_connection(
    text: &str,
    mc: &MiddlewareChain,
    connections: &RwLock<HashMap<String, ConnectionState>>,
    conn_id: &str,
    client_is_local: bool,
    method: &str,
    req_id: Option<serde_json::Value>,
    start: &CallerSnapshot,
) -> String {
    if method == "capabilities.ui.invoke" {
        return match live_ui_caller(connections, conn_id, start).await {
            Some(live) => dispatch_snapshot(text, mc, live, client_is_local, Some(conn_id.to_owned())).await,
            None => viewer_denied_response(req_id),
        };
    }
    // Preserve the login/admin error contract before the stricter cache/identity gate.
    // The authority is the LIVE one at dispatch, not the arrival snapshot: a
    // connection that was demoted (or re-logged-in) while the request waited
    // behind a queue and a lane permit must be refused here, so the admin
    // gate in `process_request` sees the demotion too.
    //
    // Browser viewer methods keep their own contract: a denial there must not
    // echo the reason (no credential, user or wall message), so they fall
    // through to `revalidate_for_dispatch` and answer with the generic
    // `denied`. The gate below still applies -- it only decides WHICH refusal
    // the caller sees, never whether the call is admitted.
    if !is_browser_viewer_method(method) {
        let live = CallerSnapshot::read(connections, conn_id, client_is_local).await;
        let denial = crate::gateway::caller_identity::with_caller_identity(
            live.role.clone(),
            live.user.clone(),
            client_is_local,
            Some(conn_id.to_string()),
            async { authorization_error(method, req_id.clone()) },
        )
        .await;
        if let Some(denial) = denial {
            return denial;
        }
    }
    match revalidate_for_dispatch(connections, conn_id, client_is_local, method, start).await {
        Some(snap) => {
            dispatch_snapshot(text, mc, snap, client_is_local, Some(conn_id.to_string())).await
        }
        None => viewer_denied_response(req_id),
    }
}

/// Idempotency principal. Viewer methods key on the verified user (`v:` prefix,
/// disjoint from any plain user id used by other methods); everything else is
/// unchanged.
#[must_use]
pub fn idempotency_principal(method: &str, snapshot: &CallerSnapshot) -> String {
    if is_browser_viewer_method(method) {
        return match viewer_principal(snapshot) {
            Some(v) => format!("v:{v}"),
            None => "v:<none>".to_string(),
        };
    }
    snapshot.user.as_deref().unwrap_or("<anon>").to_string()
}

/// Process a JSON-RPC request string
pub async fn process_request(text: &str, middleware_chain: &MiddlewareChain) -> String {
    process_request_with_panel_context(text, middleware_chain, None, None).await
}

pub(crate) async fn process_request_with_panel_context(
    text: &str,
    middleware_chain: &MiddlewareChain,
    panel_context: Option<PanelRequestContext>,
    caller: Option<&CallerSnapshot>,
) -> String {
    // Parse the request
    let request: JsonRpcRequest = match serde_json::from_str(text) {
        Ok(req) => req,
        Err(e) => {
            return serde_json::to_string(&JsonRpcResponse::error(
                None,
                PARSE_ERROR,
                format!("Parse error: {e}"),
            ))
            .unwrap_or_default();
        }
    };

    // Multi-user role gate (spec §4.6): members cannot reach server-global
    // config/credential methods. One chokepoint covers both dispatch paths —
    // CALLER_ROLE is scoped around process_request at both call sites
    // (`do_lane_dispatch` and the idempotency `Proceed` arm). `None`
    // (internal/cron) and `"operator"` pass; `"guest"` never reaches here for
    // non-connect methods (the login wall above refuses it first).
    if crate::gateway::method_admin::method_requires_admin(&request.method)
        && crate::gateway::caller_identity::caller_is_member()
    {
        return serde_json::to_string(&JsonRpcResponse::error(
            request.id.clone(),
            AUTH_REQUIRED,
            // Shared with the Panel through `aleph_protocol` — the wording is
            // not local to this arm, because the cluster page keys its role
            // explanation off these exact words.
            ADMIN_REQUIRED_MESSAGE.to_string(),
        ))
        .unwrap_or_default();
    }

    // Distributed-trace context: honour an inbound W3C `traceparent` (carried
    // in params) or mint a fresh root trace, so every log/span emitted while
    // handling this request is correlatable. See `trace_context` for why this
    // is a lightweight propagation layer rather than a full OTel integration.
    let trace =
        crate::gateway::trace_context::TraceContext::from_request_params(request.params.as_ref());
    let span = tracing::info_span!(
        "rpc",
        trace_id = %trace.trace_id,
        span_id = %trace.span_id,
        method = %request.method,
    );

    #[cfg(feature = "native-qa")]
    let request_for_observer = request.clone();
    let response = {
        use tracing::Instrument;
        middleware_chain
            .serve_with_panel_context(request, panel_context)
            .instrument(span)
            .await
    };

    #[cfg(not(feature = "native-qa"))]
    let _ = caller;

    #[cfg(feature = "native-qa")]
    if let Some(caller) = caller {
        crate::gateway::native_qa::observe_rpc(
            &middleware_chain.native_qa_observer(),
            caller,
            &request_for_observer,
            &response,
        );
    }

    // Echo the trace context back so the caller / a downstream hop can continue
    // the trace (naming our span as the parent). `traceparent` is a non-standard
    // sibling of the JSON-RPC envelope fields; serde clients ignore unknown
    // fields, so this stays backward-compatible.
    let mut value = serde_json::to_value(&response).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "traceparent".to_string(),
            serde_json::Value::String(trace.to_header()),
        );
    }
    serde_json::to_string(&value).unwrap_or_default()
}
