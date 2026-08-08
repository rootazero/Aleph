//! Exec approval RPC handlers.
//!
//! Handlers for exec approval operations:
//! - exec.approval.resolve - Resolve an approval with a decision
//! - exec.approvals.pending - List pending approvals
//!
//! Approval grants are in-memory only (once / session), so there is no
//! approval config to read or write.
//!
//! ## Who may see and answer a gate (2026-08-08)
//!
//! Both methods used to be plain `exec.` family members, i.e. operator-only
//! with "No carve-outs" written next to the prefix. Together with the delivery
//! half — `approval.` was gated whole in `EventScopeGuard` — that made every
//! face of the approval gate invisible to a member, so a member's own run
//! blocked on a confirmation nobody could give it, sat for the full 120-second
//! timeout, and the recorded workaround was to send `exec_tier: "full"`. The
//! least safe tier was the only one that worked, which is the exact inversion
//! of what a permission tier is for.
//!
//! Both methods are now member-reachable and **filtered by the same predicate
//! the rest of the perimeter uses**: a caller sees, and may answer, exactly the
//! approvals whose `session_key` they can see through
//! [`visibility::session_visible_to`]. Two properties are load-bearing:
//!
//! 1. **A record with an empty `session_key` is a FLEET approval** (a cluster
//!    node's command, raised over reverse RPC and belonging to no local run —
//!    `approval/node_requester.rs` publishes it with `String::new()`). It has no
//!    owner to compare against, so it is operator-only on both faces. The
//!    delivery-side twin of this rule is `SessionIdentity::OperatorOnly`.
//! 2. **`resolve` refuses with the message an unknown id already produces.**
//!    This is the one verb in the round-2 arc that GRANTS rather than restricts,
//!    so a foreign id must be indistinguishable from a stale one — otherwise the
//!    refusal itself enumerates which approvals exist.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::HandlerRegistry;
use crate::exec::{ApprovalDecisionType, ExecApprovalManager, PendingApproval};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

/// Parameters for exec.approval.resolve
#[derive(Debug, Deserialize)]
pub struct ApprovalResolveParams {
    /// Approval request ID
    pub id: String,
    /// Decision
    pub decision: ApprovalDecisionType,
    /// Display name of resolver
    pub resolved_by: Option<String>,
    /// Free-text reason for a `deny` decision, relayed verbatim to the model
    /// so it re-plans on the human's actual objection. Ignored on approvals.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Response for list pending
#[derive(Debug, Serialize)]
pub struct PendingListResponse {
    pub pending: Vec<PendingApproval>,
}

/// May this caller see (and therefore answer) an approval raised for
/// `session_key`?
///
/// The empty string is not a session — it is how `node_requester` marks a
/// fleet approval — so it resolves on the role instead. Everything else goes
/// through the one visibility chokepoint, which means room membership is
/// answered for free and there is no second predicate here to drift from
/// `visibility.rs`.
///
/// Fails closed: an unparseable key, a missing row, or a store error is a
/// refusal. That direction is deliberate and is the opposite of
/// `existing_session_is_visible`'s — that predicate answers "may I address a
/// key", where a not-yet-created session is a normal first turn; this one
/// answers "may I be told this approval exists and act on it", where being
/// unable to establish ownership is exactly when the answer must be no.
async fn approval_visible_to_caller(session_key: &str, store: &Arc<dyn SessionStore>) -> bool {
    if session_key.is_empty() {
        return crate::tools::turn_context::role_is_operator(
            crate::gateway::caller_identity::current_caller_role().as_deref(),
        );
    }
    let Some(parsed) = SessionKey::from_key_string(session_key) else {
        return false;
    };
    match store.get_metadata(&parsed).await {
        Ok(Some(meta)) => visibility::session_visible(&meta),
        Ok(None) | Err(_) => false,
    }
}

/// Register all exec-approval methods in the JSON-RPC handler registry.
/// All methods share a single `Arc<ExecApprovalManager>`; `session_store` backs
/// the per-caller filter both of them apply.
pub fn register_handlers(
    registry: &mut HandlerRegistry,
    manager: Arc<ExecApprovalManager>,
    session_store: Arc<dyn SessionStore>,
) {
    {
        let m = manager.clone();
        let s = session_store.clone();
        registry.register("exec.approval.resolve", move |req| {
            let m = m.clone();
            let s = s.clone();
            async move { handle_approval_resolve(req, m, s).await }
        });
    }
    {
        let m = manager.clone();
        let s = session_store.clone();
        registry.register("exec.approvals.pending", move |req| {
            let m = m.clone();
            let s = s.clone();
            async move { handle_approvals_pending(req, m, s).await }
        });
    }
}

/// Handle exec.approval.resolve
///
/// Resolves a pending approval with a decision.
async fn handle_approval_resolve(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
    session_store: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: ApprovalResolveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Resolve the id to its session BEFORE acting on it. An id the caller may
    // not see is refused with the message a stale id already produces, so the
    // refusal cannot be used to learn which approvals exist. An id the manager
    // has no record of falls through to `resolve_with_reason`, which produces
    // that same message on its own.
    if let Some(pending) = manager
        .list_pending()
        .into_iter()
        .find(|p| p.record.id == params.id)
    {
        if !approval_visible_to_caller(&pending.record.session_key, &session_store).await {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Approval not found or already resolved: {}", params.id),
            );
        }
    }

    let resolved = manager.resolve_with_reason(
        &params.id,
        params.decision,
        params.resolved_by,
        params.reason,
    );

    if resolved {
        JsonRpcResponse::success(request.id, json!({ "ok": true }))
    } else {
        JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Approval not found or already resolved: {}", params.id),
        )
    }
}

/// Handle exec.approvals.pending
///
/// Returns list of pending approvals.
async fn handle_approvals_pending(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
    session_store: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    // Filtered, not refused. This response is the ONLY seed for the Panel's
    // approval bell and for the inline card under a blocked tool row, so an
    // empty list and a denial are the same thing to the client — which is why
    // the answer has to be "the ones that are yours", never "none of them".
    let mut visible = Vec::new();
    for p in manager.list_pending() {
        if approval_visible_to_caller(&p.record.session_key, &session_store).await {
            visible.push(p);
        }
    }
    JsonRpcResponse::success(request.id, json!(PendingListResponse { pending: visible }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::gateway::caller_identity::{CALLER_IS_LOOPBACK, CALLER_ROLE, CALLER_USER};
    use crate::gateway::router::SessionKey;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::scope::{with_scope, ScopeAttribution};
    use tempfile::TempDir;

    fn temp_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    fn temp_store() -> (Arc<dyn SessionStore>, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = FileSessionStore::new(FileSessionStoreConfig {
            base_dir: temp.path().to_path_buf(),
            ..Default::default()
        })
        .unwrap();
        (Arc::new(store) as Arc<dyn SessionStore>, temp)
    }

    /// The production shape: the WS dispatch loop scopes all three task-locals
    /// around `process_request`, so a predicate that reads one of them is only
    /// exercised honestly when all three are in place.
    async fn as_caller<F, T>(user: &str, role: &str, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        CALLER_USER
            .scope(
                Some(user.to_string()),
                CALLER_ROLE.scope(Some(role.to_string()), CALLER_IS_LOOPBACK.scope(false, fut)),
            )
            .await
    }

    #[tokio::test]
    async fn test_handle_approvals_pending() {
        let manager = temp_manager();
        let (store, _dir) = temp_store();

        let request = JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1));
        let response = handle_approvals_pending(request, manager, store).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("pending").is_some());
    }

    #[tokio::test]
    async fn test_handle_approval_resolve_not_found() {
        let manager = temp_manager();
        let (store, _dir) = temp_store();

        let request = JsonRpcRequest::new(
            "exec.approval.resolve",
            Some(json!({
                "id": "non-existent-id",
                "decision": "allow-once"
            })),
            Some(json!(1)),
        );
        let response = handle_approval_resolve(request, manager, store).await;

        assert!(response.is_error());
    }

    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let manager = temp_manager();
        let (store, _dir) = temp_store();
        let mut registry = HandlerRegistry::empty();
        register_handlers(&mut registry, manager, store);
        for m in ["exec.approval.resolve", "exec.approvals.pending"] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }

    /// Both handlers are thin wrappers around this predicate, so this is where
    /// the decision is pinned. Alice may reach an approval raised for her own
    /// session and not one raised for bob's — the whole point of carving these
    /// two methods open to members.
    #[tokio::test]
    async fn a_session_approval_reaches_its_own_session_and_no_other() {
        let (store, _dir) = temp_store();

        let alice_key = SessionKey::main("alice-conv");
        let bob_key = SessionKey::main("bob-conv");
        with_scope(
            Some(ScopeAttribution::personal("u-alice")),
            store.get_or_create(&alice_key),
        )
        .await
        .unwrap();
        with_scope(
            Some(ScopeAttribution::personal("u-bob")),
            store.get_or_create(&bob_key),
        )
        .await
        .unwrap();

        assert!(
            as_caller(
                "u-alice",
                "member",
                approval_visible_to_caller(&alice_key.to_key_string(), &store)
            )
            .await,
            "a member must reach the gate blocking their OWN run — otherwise \
             the run waits out the 120s timeout with nobody able to answer it"
        );
        assert!(
            !as_caller(
                "u-alice",
                "member",
                approval_visible_to_caller(&bob_key.to_key_string(), &store)
            )
            .await,
            "and must not reach anyone else's"
        );
    }

    /// A fleet approval names no session (`node_requester` publishes
    /// `session_key: String::new()`), so there is no owner to compare against.
    /// It resolves on the role instead — the RPC twin of
    /// `SessionIdentity::OperatorOnly` on the delivery face.
    #[tokio::test]
    async fn a_fleet_approval_is_operator_only() {
        let (store, _dir) = temp_store();

        assert!(
            as_caller(
                "u-owner",
                "operator",
                approval_visible_to_caller("", &store)
            )
            .await,
            "the operator answers for the fleet"
        );
        assert!(
            !as_caller("u-alice", "member", approval_visible_to_caller("", &store)).await,
            "a member must not see a cluster node's command"
        );
    }

    /// Fail-closed, in the direction opposite to `existing_session_is_visible`.
    /// That predicate answers "may I address this key", where a not-yet-created
    /// session is a normal first turn; this one answers "may I be told this
    /// approval exists and act on it", where being unable to establish
    /// ownership is exactly when the answer must be no.
    #[tokio::test]
    async fn an_unresolvable_session_key_is_refused() {
        let (store, _dir) = temp_store();
        for key in ["not a session key", "agent:main:s999-never-created"] {
            assert!(
                !as_caller("u-alice", "member", approval_visible_to_caller(key, &store)).await,
                "{key} names no readable session and must be refused"
            );
        }
    }
}
