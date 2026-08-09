//! Exec approval RPC handlers.
//!
//! Handlers for exec approval operations:
//! - exec.approval.resolve - Resolve an approval with a decision
//! - exec.approvals.pending - List pending approvals
//!
//! Approval grants are in-memory only (once / session), so there is no
//! approval config to read or write.
//!
//! # Who may answer (2026-08-08)
//!
//! The `exec.` family used to be admin-gated wholesale, which made the DEFAULT
//! execution tier a dead end for every member: `Auto` parks any non-idempotent
//! tool call on an approval, and the only principal permitted to resolve it was
//! somebody else, so the call sat for the whole approval window and then died
//! as `Timeout`. Both methods are now carved open
//! (`method_admin::MEMBER_CARVE_OUTS`) and owner-scoped HERE instead, against
//! the approval record's own `session_key`.
//!
//! That carve-out and this scoping are ONE decision — deleting the checks below
//! silently hands every member every other member's parked commands, which is
//! also the argument for why the gate could not simply be widened to `"member"`
//! at the chokepoint.
//!
//! The shape is copied from [`super::clarification`], this module's twin, down
//! to the two asymmetric rulings it makes: a list drops anything it cannot
//! resolve, and an addressed resolve answers "not yours" with the SAME response
//! as "not there" so the id space cannot be probed.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::HandlerRegistry;
use crate::exec::{ApprovalDecisionType, ExecApprovalManager, PendingApproval};
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

/// Register all exec-approval methods in the JSON-RPC handler registry.
/// All methods share a single `Arc<ExecApprovalManager>`.
///
/// `sessions` backs the per-user visibility checks both handlers apply — the
/// same `SessionStore` every other session-scoped RPC resolves ownership
/// against, and the same argument [`super::clarification::register_handlers`]
/// takes for the same reason.
pub fn register_handlers(
    registry: &mut HandlerRegistry,
    manager: Arc<ExecApprovalManager>,
    sessions: Arc<dyn SessionStore>,
) {
    {
        let m = manager.clone();
        let s = sessions.clone();
        registry.register("exec.approval.resolve", move |req| {
            let m = m.clone();
            let s = s.clone();
            async move { handle_approval_resolve(req, m, s).await }
        });
    }
    {
        let m = manager.clone();
        let s = sessions.clone();
        registry.register("exec.approvals.pending", move |req| {
            let m = m.clone();
            let s = s.clone();
            async move { handle_approvals_pending(req, m, s).await }
        });
    }
}

/// Whether the approval raised in `session_key` is addressable by this caller.
///
/// # The short-circuit is on ROLE, and that is deliberate
///
/// It would be natural to write `visibility::visible_owner_filter().is_none()`
/// here, the way `clarification.pending` does. It would also be wrong, and
/// silently: an operator's `CALLER_USER` is `OWNER_USER_ID`, not `None`
/// (`resolve_connection_identity`), so that predicate is `Some(..)` for them and
/// the filter would engage — quietly removing every MEMBER's card from the
/// operator's list. The event plane admits those same frames to an admin
/// (`event_visibility`'s `BySessionKeyOrAdmin`), and the Panel rebuilds
/// `pending_approvals` from THIS method every time such a frame lands, so the
/// two halves disagreeing does not merely hide the card — the operator watches
/// it arrive and then vanish on the refetch it triggered.
///
/// [`caller_is_member`](crate::gateway::caller_identity::caller_is_member) is
/// the same predicate the admin gate enforces and the role half of the event
/// plane's arm, so the RPC face and the delivery face answer this from one
/// derivation. An operator's view is therefore byte-identical to what it was
/// before this method was scoped at all.
///
/// A key that does not parse, or whose session row is not there, answers
/// `false`: this predicate is asked "may I be told this approval exists", and
/// "I could not work out whose it is" must never mean "everyone's".
async fn approval_session_is_visible(sessions: &dyn SessionStore, session_key: &str) -> bool {
    if !crate::gateway::caller_identity::caller_is_member() {
        return true;
    }
    let Some(key) = SessionKey::from_key_string(session_key) else {
        return false;
    };
    matches!(
        sessions.get_metadata(&key).await,
        Ok(Some(meta)) if visibility::session_visible(&meta)
    )
}

/// Handle exec.approval.resolve
///
/// Resolves a pending approval with a decision.
///
/// The approval is addressed by an opaque id, so ownership is resolved through
/// it: `get_pending` → the record's own `session_key` → the same visibility
/// predicate every session-addressed RPC uses. An approval belonging to someone
/// else is answered by the SAME arm as an id that was never issued — one
/// message, one code — so a member cannot use this method to learn that another
/// member has a command parked, nor which id it holds.
async fn handle_approval_resolve(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: ApprovalResolveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Ownership BEFORE the manager: `resolve_with_reason` unblocks the waiting
    // tool call, and that is not something to do first and check afterwards.
    let addressable = match manager.get_pending(&params.id) {
        Some(p) => approval_session_is_visible(&*sessions, &p.record.session_key).await,
        None => false,
    };

    let resolved = addressable
        && manager.resolve_with_reason(
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
///
/// Each item is filtered by its OWN session's visibility. The list is
/// process-wide and small, so a per-item check is the natural shape (same
/// ruling as `clarification.pending`); an unrestricted caller sees every item,
/// unchanged from before this method was open to members at all.
async fn handle_approvals_pending(
    request: JsonRpcRequest,
    manager: Arc<ExecApprovalManager>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let all = manager.list_pending();

    let pending = if !crate::gateway::caller_identity::caller_is_member() {
        all
    } else {
        let mut visible = Vec::with_capacity(all.len());
        for item in all {
            if approval_session_is_visible(&*sessions, &item.record.session_key).await {
                visible.push(item);
            }
        }
        visible
    };

    JsonRpcResponse::success(request.id, json!(PendingListResponse { pending }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::analysis::CommandAnalysis;
    use crate::exec::decision::ExecApprovalRequest;
    use crate::gateway::caller_identity::{CALLER_ROLE, CALLER_USER};
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use tempfile::TempDir;

    fn temp_manager() -> Arc<ExecApprovalManager> {
        Arc::new(ExecApprovalManager::new())
    }

    /// A real `SessionStore` — the approval record's `session_key` is a real,
    /// parseable key in production (`turn.session_key.to_string()`), so the
    /// fixtures use that shape rather than an opaque test string.
    fn sessions() -> (TempDir, Arc<dyn SessionStore>) {
        let tmp = TempDir::new().expect("tempdir");
        let store = Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: tmp.path().to_path_buf(),
                ..Default::default()
            })
            .expect("file session store"),
        );
        (tmp, store)
    }

    async fn create_session(
        sessions: &Arc<dyn SessionStore>,
        session_key: &str,
        owner: Option<&str>,
    ) {
        let key = SessionKey::from_key_string(session_key).expect("valid session_key fixture");
        let attribution = owner.map(crate::scope::ScopeAttribution::personal);
        crate::scope::with_scope(attribution, sessions.get_or_create(&key))
            .await
            .expect("get_or_create");
    }

    /// Park an approval on `session_key` and return its id. Goes through the
    /// manager's real `create` + `register_pending` pair, so the entry is the
    /// same shape a tool call parks.
    fn park_approval(manager: &Arc<ExecApprovalManager>, session_key: &str) -> String {
        let request = ExecApprovalRequest {
            // The id is the CALLER's to mint (`ExecApprovalRecord::from_request`
            // copies it verbatim, and `register_pending` keys the map on it), so
            // reusing one silently collapses two cards into one entry.
            id: uuid::Uuid::new_v4().to_string(),
            command: "rm -rf ./build".to_string(),
            cwd: None,
            analysis: CommandAnalysis::error("test fixture"),
            agent_id: "main".to_string(),
            session_key: session_key.to_string(),
            reason: Some("non-idempotent".to_string()),
            originator_user_id: None,
            grant_key: None,
        };
        let record = manager.create(&request, 120_000);
        let (id, rx, _timeout) = manager.register_pending(record);
        // The waiting tool call is what holds this receiver; leaking it keeps
        // the entry resolvable for the duration of the test.
        std::mem::forget(rx);
        id
    }

    /// Drive a handler as a MEMBER connection — both task-locals, because the
    /// scoping keys on the role and the ownership check keys on the user, and a
    /// fixture that sets only one of them would exercise a principal that
    /// cannot exist on the wire.
    async fn as_member<F, T>(user: &str, fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        CALLER_USER
            .scope(
                Some(user.to_string()),
                CALLER_ROLE.scope(Some("member".to_string()), fut),
            )
            .await
    }

    /// Drive a handler as an OPERATOR connection. Note the user id: an operator
    /// carries `OWNER_USER_ID`, not `None` — which is exactly why the filter
    /// below cannot be written against `visible_owner_filter()`.
    async fn as_operator<F, T>(fut: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        CALLER_USER
            .scope(
                Some(crate::gateway::security::store::OWNER_USER_ID.to_string()),
                CALLER_ROLE.scope(Some("operator".to_string()), fut),
            )
            .await
    }

    fn resolve_request(id: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(
            "exec.approval.resolve",
            Some(json!({ "id": id, "decision": "allow-once" })),
            Some(json!(1)),
        )
    }

    fn pending_ids(response: &JsonRpcResponse) -> Vec<String> {
        response
            .result
            .as_ref()
            .and_then(|r| r.get("pending"))
            .and_then(|p| p.as_array())
            .expect("pending is an array")
            .iter()
            .map(|item| {
                item["record"]["id"]
                    .as_str()
                    .expect("record carries an id")
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn test_handle_approvals_pending() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();

        let request = JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1));
        let response = handle_approvals_pending(request, manager, sess).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result.get("pending").is_some());
    }

    #[tokio::test]
    async fn test_handle_approval_resolve_not_found() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();

        let response =
            handle_approval_resolve(resolve_request("non-existent-id"), manager, sess).await;

        assert!(response.is_error());
    }

    // -- Member scoping: the reason `exec.` is carved out of the admin gate --

    /// The whole point of the carve-out. Before it, a member's own parked call
    /// had no principal allowed to release it and died at the approval timeout.
    #[tokio::test]
    async fn a_member_resolves_the_approval_parked_on_their_own_session() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", Some("u-alice")).await;
        let id = park_approval(&manager, "agent:main:main");

        let response = as_member("u-alice",
                handle_approval_resolve(resolve_request(&id), manager, sess),
            )
            .await;

        assert!(
            response.is_success(),
            "a member must be able to release their own parked tool call: {:?}",
            response.error
        );
    }

    /// The other half. Answered by the SAME arm as an id that was never issued,
    /// so the id space cannot be probed for other people's parked commands.
    #[tokio::test]
    async fn a_foreign_approval_is_refused_exactly_as_an_unknown_id_is() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main:s1", Some("u-bob")).await;
        let bobs = park_approval(&manager, "agent:main:main:s1");

        let foreign = as_member("u-alice",
                handle_approval_resolve(resolve_request(&bobs), manager.clone(), sess.clone()),
            )
            .await;
        let unknown = as_member("u-alice",
                handle_approval_resolve(
                    resolve_request("never-issued"),
                    manager.clone(),
                    sess.clone(),
                ),
            )
            .await;

        assert!(foreign.is_error(), "alice must not resolve bob's approval");
        let (fe, ue) = (
            foreign.error.expect("refusal carries an error"),
            unknown.error.expect("refusal carries an error"),
        );
        assert_eq!(
            fe.code, ue.code,
            "\"not yours\" and \"not there\" must share one code"
        );
        assert!(
            fe.message.contains(&bobs) && ue.message.contains("never-issued"),
            "both echo only the id the caller supplied"
        );

        // And it really was left parked, not silently consumed.
        assert!(
            manager.get_pending(&bobs).is_some(),
            "a refused resolve must not have touched the entry"
        );
    }

    /// A member's list names only their own sessions' cards. Deliberately
    /// asymmetric with the resolve path: this one drops anything it cannot
    /// resolve, because "I could not work out whose this is" must never mean
    /// "everyone's".
    #[tokio::test]
    async fn the_pending_list_names_only_the_callers_own_approvals() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", Some("u-alice")).await;
        create_session(&sess, "agent:main:main:s1", Some("u-bob")).await;
        let alices = park_approval(&manager, "agent:main:main");
        let bobs = park_approval(&manager, "agent:main:main:s1");
        // A card whose session row was never written — unresolvable, so hidden.
        let orphan = park_approval(&manager, "agent:main:main:ghost");

        let response = as_member("u-alice",
                handle_approvals_pending(
                    JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1)),
                    manager.clone(),
                    sess.clone(),
                ),
            )
            .await;

        let ids = pending_ids(&response);
        assert!(
            ids.contains(&alices),
            "alice must see her own card: {ids:?}"
        );
        assert!(!ids.contains(&bobs), "bob's card must not appear: {ids:?}");
        assert!(
            !ids.contains(&orphan),
            "an unresolvable card must be dropped, not shown: {ids:?}"
        );
    }

    /// The operator half, and the reason the short-circuit reads the ROLE.
    ///
    /// An operator connection carries `CALLER_USER = OWNER_USER_ID`, so an
    /// owner-keyed filter (`visible_owner_filter().is_none()` — the shape
    /// `clarification.pending` uses) engages for them and silently drops every
    /// member's card. That is not merely a narrower list: the event plane still
    /// delivers those frames to an admin, and the Panel rebuilds this list on
    /// each one, so the operator would see the card appear and then vanish on
    /// the refetch it triggered. This test fails on that mistake.
    #[tokio::test]
    async fn an_operator_still_sees_a_members_parked_approval() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", Some("u-alice")).await;
        let alices = park_approval(&manager, "agent:main:main");

        let response = as_operator(handle_approvals_pending(
            JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1)),
            manager.clone(),
            sess.clone(),
        ))
        .await;

        assert!(
            pending_ids(&response).contains(&alices),
            "an operator answering on a member's behalf is the one workflow \
             this family exists for; their view must be what it was before \
             this method was scoped at all"
        );

        // ...and they can actually release it, not just look at it.
        let resolved = as_operator(handle_approval_resolve(
            resolve_request(&alices),
            manager,
            sess,
        ))
        .await;
        assert!(resolved.is_success(), "{:?}", resolved.error);
    }

    /// The zero-change guarantee: an unrestricted caller (internal / cron) —
    /// no role scoped at all — short-circuits before the store and sees the
    /// whole list, which is what this method did before it was scoped.
    #[tokio::test]
    async fn an_unrestricted_caller_still_sees_every_pending_approval() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();
        let alices = park_approval(&manager, "agent:main:main");
        let bobs = park_approval(&manager, "agent:main:main:s1");

        let response = handle_approvals_pending(
            JsonRpcRequest::with_id("exec.approvals.pending", None, json!(1)),
            manager,
            sess,
        )
        .await;

        let ids = pending_ids(&response);
        assert_eq!(
            ids.len(),
            2,
            "two distinct cards were parked; a shorter list means they \
             collapsed rather than passed the filter: {ids:?}"
        );
        assert!(
            ids.contains(&alices) && ids.contains(&bobs),
            "no CALLER_USER scope means unrestricted — and note neither \
             session row exists here, so this also pins that the short-circuit \
             happens BEFORE the store lookup: {ids:?}"
        );
    }

    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let manager = temp_manager();
        let (_tmp, sess) = sessions();
        let mut registry = HandlerRegistry::empty();
        register_handlers(&mut registry, manager, sess);
        for m in ["exec.approval.resolve", "exec.approvals.pending"] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
}
