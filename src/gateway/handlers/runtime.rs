//! `runtime.*` — the read-only agent panel.
//!
//! One method today: `runtime.agents.list`, a snapshot of
//! [`crate::gateway::runtime::RuntimeAgents`] (which PTY session, what agent,
//! what state). The table is process-global and populated by the PTY flush
//! loop (`gateway::pty::manager::start_flush_loop`), so this handler is
//! stateless — it reads the same singleton `gateway::handlers::pty` reaches
//! through `pty::manager()`, no boot-time wiring required.
//!
//! ## Operator-only, on BOTH faces — same gate as `pty.*`, not a new one
//!
//! An agent panel entry names a session id, its cwd, and what is running in
//! it: the same disclosure `pty.*` already gates, seen through a different
//! lens. `"runtime."` is therefore in
//! [`ADMIN_PREFIXES`](crate::gateway::method_admin) and in
//! [`EventScopeGuard::default_rules`](crate::gateway::event_scope::EventScopeGuard::default_rules)
//! — see `gateway::handlers::pty`'s own module doc for why a sentence about
//! who may reach a surface needs a copy on every face that surface has.
//!
//! The per-row ownership filter below is the SAME predicate `pty.attach` /
//! `pty.input` / `pty.resize` / `pty.close` use in their own
//! `require_owned` — [`crate::gateway::pty::PtyManager::owner_of`] +
//! [`crate::gateway::pty::SessionOwner::admits`] — copied, not re-derived,
//! so the two lists of the same underlying sessions cannot silently
//! disagree about who sees which row (判据 §9).

use aleph_protocol::runtime::RuntimeAgentsListResponse;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::pty;

/// `runtime.agents.list` — the caller's own agent-panel rows, in the order
/// [`crate::gateway::runtime::RuntimeAgents::snapshot`] returns them (by
/// session id; nothing here re-sorts — sorting for display is
/// `shared/ui_logic`'s job).
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    let actor = crate::gateway::visibility::ambient_actor();
    let agents = crate::gateway::runtime::agents()
        .snapshot()
        .into_iter()
        .filter(|entry| {
            pty::manager()
                .owner_of(&entry.session_id)
                .admits(actor.as_deref())
        })
        .collect();
    let body = RuntimeAgentsListResponse { agents };
    match serde_json::to_value(&body) {
        Ok(v) => JsonRpcResponse::success(id, v),
        // A failure to encode the server's OWN response type is never the
        // caller's fault — `INVALID_PARAMS` would tell them their request
        // was wrong when it was this handler's encode step that failed.
        // Same shape as `handlers/users.rs`'s `encoded` helper.
        Err(e) => JsonRpcResponse::error(id, INTERNAL_ERROR, format!("encode failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn req(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    /// The gate lives in `method_admin`/`event_scope` (prefix membership,
    /// checked by the router), not in this handler — pinned by
    /// `method_admin::tests` and `event_scope::tests`, not here (判据 §2:
    /// calling `handle_list` directly and asserting on a refusal it never
    /// issues would be asserting on a function that cannot go red).
    ///
    /// What THIS test proves: the response must parse as the protocol's own
    /// type, constructed from it — never a hand-rolled `json!` (判据 §10).
    #[tokio::test]
    async fn the_response_parses_as_the_protocol_type() {
        let resp = handle_list(req("runtime.agents.list", json!({}))).await;
        let _: RuntimeAgentsListResponse =
            serde_json::from_value(resp.result.expect("list always succeeds"))
                .expect("must be the protocol shape");
    }
}
