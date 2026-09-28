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

use crate::gateway::middleware::MiddlewareChain;
use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, ADMIN_REQUIRED_MESSAGE, AUTH_REQUIRED, PARSE_ERROR,
};

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
pub async fn dispatch_with_caller_context(
    text: &str,
    mc: &MiddlewareChain,
    caller_role: Option<String>,
    caller_user: Option<String>,
    caller_is_loopback: bool,
    caller_conn_id: Option<String>,
) -> String {
    crate::scope::with_scope(
        caller_user
            .clone()
            .map(|u| crate::scope::ScopeAttribution::personal(&u)),
        crate::gateway::caller_identity::CALLER_USER.scope(
            caller_user,
            crate::gateway::caller_identity::CALLER_ROLE.scope(
                caller_role,
                crate::gateway::caller_identity::CALLER_IS_LOOPBACK.scope(
                    caller_is_loopback,
                    crate::gateway::caller_identity::CALLER_CONN_ID
                        .scope(caller_conn_id, process_request(text, mc)),
                ),
            ),
        ),
    )
    .await
}

/// Process a JSON-RPC request string
pub async fn process_request(text: &str, middleware_chain: &MiddlewareChain) -> String {
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

    // Dispatch to middleware chain inside the trace span.
    let response = {
        use tracing::Instrument;
        middleware_chain.serve(request).instrument(span).await
    };

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