//! Clarification RPC handlers (HITL P4 — `ask_user`).
//!
//! Handlers for the clarification a parked `ask_user` tool is blocked on:
//! - clarification.pending - List outstanding questions
//! - clarification.resolve - Answer one, unblocking the tool
//!
//! The clarification twin of [`super::exec_approvals`]. A channel (Telegram &
//! co.) answers by replying in the conversation — the inbound router routes it
//! to `ClarificationManager::resolve`. Panel traffic never traverses the
//! inbound router, so this is the Panel's only way to answer; both paths land
//! on the same manager, the same session key, and the same `interpret_reply`.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::HandlerRegistry;
use crate::clarification::session::PendingClarification;
use crate::clarification::ClarificationManager;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

/// Parameters for clarification.resolve
#[derive(Debug, Deserialize)]
pub struct ClarificationResolveParams {
    /// Session the question was asked in — the clarification registry key,
    /// shipped to the client on the `AskUser` frame.
    pub session_key: String,
    /// The user's answer. Interpreted exactly as a channel reply is: a bare
    /// 1-based number picks that option, an option label matches it, anything
    /// else is free text.
    pub reply: String,
}

/// Response for clarification.resolve
#[derive(Debug, Serialize)]
pub struct ClarificationResolveResponse {
    /// Whether a pending clarification was actually unblocked. `false` for a
    /// stale answer (already resolved, superseded, or timed out).
    pub resolved: bool,
}

/// Response for clarification.pending
#[derive(Debug, Serialize)]
pub struct PendingListResponse {
    pub pending: Vec<PendingClarification>,
}

/// Register the clarification methods, all sharing one `Arc<ClarificationManager>`
/// — the same instance the `ask_user` tool registers its questions with.
///
/// `sessions` backs the P1 visibility checks both handlers now apply
/// (`gateway::visibility`) — the same `SessionStore` every other
/// session-scoped RPC resolves ownership against.
pub fn register_handlers(
    registry: &mut HandlerRegistry,
    manager: Arc<ClarificationManager>,
    sessions: Arc<dyn SessionStore>,
) {
    {
        let m = manager.clone();
        let s = sessions.clone();
        registry.register("clarification.resolve", move |req| {
            let m = m.clone();
            let s = s.clone();
            async move { handle_resolve(req, m, s).await }
        });
    }
    {
        let m = manager.clone();
        let s = sessions.clone();
        registry.register("clarification.pending", move |req| {
            let m = m.clone();
            let s = s.clone();
            async move { handle_pending(req, m, s).await }
        });
    }
}

/// Handle clarification.resolve
///
/// Answers the question the session's `ask_user` is parked on.
///
/// A stale answer is NOT an error: the question may have already been answered
/// from another surface, superseded, or timed out. Reporting `resolved: false`
/// (rather than a JSON-RPC error) consumes it silently, matching the inbound
/// router's `clarify:` button branch — a stale tap there is swallowed too,
/// never surfaced and never leaked into a new agent turn.
///
/// P1 (spec §11-1c): the Task-6 addressed-key pattern — resolve
/// `session_key` and deny with `visibility::not_found_response` unless it is
/// visible to the current caller, before ever touching the manager. A
/// malformed `session_key` is a distinct, pre-existing validation error
/// (`INVALID_PARAMS`), not an existence question.
async fn handle_resolve(
    request: JsonRpcRequest,
    manager: Arc<ClarificationManager>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params: ClarificationResolveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format")
        }
    };
    match sessions.get_metadata(&key).await {
        Ok(Some(meta)) if visibility::session_visible(&meta) => {}
        _ => return visibility::not_found_response(request.id),
    }

    // `resolve` is the single truth: it reports `false` unless a waiter was
    // actually unblocked with this reply. The client MUST honour that — a
    // `false` means its Enter-hijack was stale and the text is still an
    // unsent message, not an answer.
    let resolved = manager.resolve(&params.session_key, &params.reply).await;

    JsonRpcResponse::success(request.id, json!(ClarificationResolveResponse { resolved }))
}

/// Handle clarification.pending
///
/// Lists the live questions. The `AskUser` frame is a one-shot push, so a
/// client that connects or reloads mid-question needs this to learn a tool is
/// parked on its answer.
///
/// P1 (spec §11-1c): each item is filtered by its OWN session's visibility —
/// the list is process-wide and small, so a per-item check (rather than a
/// single `SessionFilter`) is the natural shape. An unrestricted caller
/// (`visibility::visible_owner_filter() == None`) sees every pending item,
/// unchanged from pre-P1 behaviour; a scoped caller sees only the ones whose
/// session they own (an item whose session_key doesn't parse, or whose
/// session row doesn't exist yet, is hidden rather than guessed at).
async fn handle_pending(
    request: JsonRpcRequest,
    manager: Arc<ClarificationManager>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let all = manager.list_pending().await;

    let pending = if visibility::visible_owner_filter().is_none() {
        all
    } else {
        let mut visible = Vec::with_capacity(all.len());
        for item in all {
            let show = match SessionKey::from_key_string(&item.session_key) {
                Some(key) => matches!(
                    sessions.get_metadata(&key).await,
                    Ok(Some(meta)) if visibility::session_visible(&meta)
                ),
                None => false,
            };
            if show {
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
    use crate::clarification::{
        ClarificationOption, ClarificationRequest, DEFAULT_CLARIFY_TIMEOUT,
    };
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::protocol::RESOURCE_NOT_FOUND;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use tempfile::TempDir;

    fn manager() -> Arc<ClarificationManager> {
        Arc::new(ClarificationManager::new())
    }

    /// A real `SessionStore` backing the P1 addressed-key check both handlers
    /// now apply. Production clarification session keys are always real,
    /// parseable `SessionKey` strings (`turn.session_key.to_string()` in
    /// `ask_user.rs`) — these fixtures use the same shape rather than the
    /// opaque test-only strings `ClarificationManager`'s own unit tests use
    /// (that module treats the key as opaque; this RPC layer does not).
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

    fn resolve_request(session_key: &str, reply: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(
            "clarification.resolve",
            Some(json!({ "session_key": session_key, "reply": reply })),
            Some(json!(1)),
        )
    }

    /// The whole point of the RPC: it is the Panel's only path to the oneshot
    /// the `ask_user` tool is blocked on.
    #[tokio::test]
    async fn resolve_unblocks_the_parked_ask_user() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;
        let rx = mgr
            .register(
                "agent:main:main",
                ClarificationRequest::select(
                    "Deploy where?",
                    vec![
                        ClarificationOption::new("staging", "staging"),
                        ClarificationOption::new("production", "production"),
                    ],
                ),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        // A button tap sends the 1-based index — the same string the Telegram
        // `clarify:<idx>` callback resolves with.
        let response = handle_resolve(resolve_request("agent:main:main", "2"), mgr, sess).await;
        assert!(response.is_success(), "{:?}", response.error);
        assert_eq!(response.result.unwrap()["resolved"], true);

        let result = rx.await.expect("the parked tool must be unblocked");
        assert_eq!(result.selected_index, Some(1));
        assert_eq!(result.get_value(), Some("production"));
    }

    #[tokio::test]
    async fn resolve_free_text_is_taken_verbatim() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;
        let rx = mgr
            .register(
                "agent:main:main",
                ClarificationRequest::text("Which file?"),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        let response =
            handle_resolve(resolve_request("agent:main:main", "src/main.rs"), mgr, sess).await;
        assert_eq!(response.result.unwrap()["resolved"], true);
        assert_eq!(rx.await.unwrap().get_value(), Some("src/main.rs"));
    }

    /// A stale answer (the question was already resolved elsewhere, superseded,
    /// or timed out) must be a silent no-op, not a JSON-RPC error — same as a
    /// stale `clarify:` tap in the inbound router. Distinct from a malformed
    /// or invisible `session_key`, which is covered separately below.
    #[tokio::test]
    async fn stale_resolve_is_a_silent_no_op() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;

        // No question was ever registered for this (real, visible) session.
        let response = handle_resolve(
            resolve_request("agent:main:main", "yes"),
            mgr.clone(),
            sess.clone(),
        )
        .await;
        assert!(
            response.is_success(),
            "a stale answer must not surface as an error"
        );
        assert_eq!(response.result.unwrap()["resolved"], false);

        // Same for a session whose question was already answered.
        let rx = mgr
            .register(
                "agent:main:main",
                ClarificationRequest::text("Which file?"),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;
        let first = handle_resolve(
            resolve_request("agent:main:main", "a.rs"),
            mgr.clone(),
            sess.clone(),
        )
        .await;
        assert_eq!(first.result.unwrap()["resolved"], true);
        let _ = rx.await;

        let second = handle_resolve(resolve_request("agent:main:main", "b.rs"), mgr, sess).await;
        assert!(second.is_success());
        assert_eq!(second.result.unwrap()["resolved"], false);
    }

    /// A malformed `session_key` is a validation error, not an existence
    /// question — same convention `artifacts.rs`/`sessions.*` use.
    #[tokio::test]
    async fn resolve_rejects_a_malformed_session_key() {
        let mgr = manager();
        let (_tmp, sess) = sessions();

        let response = handle_resolve(resolve_request("not-a-session", "yes"), mgr, sess).await;
        assert_eq!(response.error.expect("expected error").code, INVALID_PARAMS);
    }

    /// P1: bob cannot resolve a question parked on alice's session, even by
    /// naming its real key — NOT_FOUND, and the parked waiter is untouched
    /// (it can still be resolved by alice afterward).
    #[tokio::test]
    async fn resolve_denies_a_foreign_owner_waiter_intact() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", Some("u-alice")).await;
        let rx = mgr
            .register(
                "agent:main:main",
                ClarificationRequest::text("Which file?"),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        let bob_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_resolve(
                    resolve_request("agent:main:main", "src/evil.rs"),
                    mgr.clone(),
                    sess.clone(),
                )
                .await
            })
            .await;
        assert_eq!(
            bob_resp.error.expect("expected error").code,
            RESOURCE_NOT_FOUND
        );

        // The waiter is untouched — alice can still answer it.
        let alice_resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_resolve(resolve_request("agent:main:main", "src/main.rs"), mgr, sess).await
            })
            .await;
        assert_eq!(alice_resp.result.unwrap()["resolved"], true);
        assert_eq!(rx.await.unwrap().get_value(), Some("src/main.rs"));
    }

    #[tokio::test]
    async fn pending_lists_the_live_question() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;
        let _rx = mgr
            .register(
                "agent:main:main",
                ClarificationRequest::select(
                    "Deploy where?",
                    vec![ClarificationOption::new("staging", "staging")],
                ),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        let request = JsonRpcRequest::with_id("clarification.pending", None, json!(1));
        let response = handle_pending(request, mgr, sess).await;

        assert!(response.is_success());
        let pending = response.result.unwrap();
        let pending = pending["pending"].as_array().expect("pending array");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["session_key"], "agent:main:main");
        assert_eq!(pending[0]["question"], "Deploy where?");
        assert_eq!(pending[0]["options"][0], "staging");
    }

    /// P1's own acceptance case: bob omits alice's pending question from
    /// `clarification.pending`, but still sees his own.
    #[tokio::test]
    async fn pending_as_bob_omits_alices_question() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", Some("u-alice")).await;
        create_session(&sess, "agent:main:main:s1", Some("u-bob")).await;
        let _alice_rx = mgr
            .register(
                "agent:main:main",
                ClarificationRequest::text("Alice's question?"),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;
        let _bob_rx = mgr
            .register(
                "agent:main:main:s1",
                ClarificationRequest::text("Bob's question?"),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        let request = JsonRpcRequest::with_id("clarification.pending", None, json!(1));
        let response = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_pending(request, mgr, sess).await
            })
            .await;

        let pending = response.result.unwrap();
        let pending = pending["pending"].as_array().expect("pending array");
        assert_eq!(pending.len(), 1, "bob sees only his own question");
        assert_eq!(pending[0]["question"], "Bob's question?");
    }

    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let mut registry = HandlerRegistry::empty();
        let (_tmp, sess) = sessions();
        register_handlers(&mut registry, manager(), sess);
        for m in ["clarification.resolve", "clarification.pending"] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
}
