//! Clarification RPC handlers (HITL P4 — `ask_user`).
//!
//! Handlers for the clarification a parked `ask_user` tool is blocked on:
//! - clarification.pending - List outstanding questions
//! - clarification.resolve - Apply an addressed answer, submission, or cancellation
//!
//! RPC clients and channel replies share the same manager and parked waiter.
//! Only the legacy string path interprets a reply as a label or number.

use crate::sync_primitives::Arc;

use aleph_protocol::{
    ClarificationActionParams, ClarificationActionResponse, ClarificationActionStatus,
    ClarificationResolveParams, ClarificationResolveResponse, PendingListResponse,
};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::super::router::SessionKey;
use super::HandlerRegistry;
use crate::clarification::{ClarificationManager, ResolveOutcome};
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

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

/// Apply an addressed action only after authorizing access to its session.
/// Stale identities are a successful no-op, distinct from invalid input.
async fn handle_resolve(
    request: JsonRpcRequest,
    manager: Arc<ClarificationManager>,
    sessions: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    // Never reflect serde's error text: it can contain private answer input.
    let invalid = |message| JsonRpcResponse::error(request.id.clone(), INVALID_PARAMS, message);
    let Some(params) = request.params else {
        return invalid("Missing clarification params");
    };
    let Some(session_key) = params
        .get("session_key")
        .and_then(serde_json::Value::as_str)
    else {
        return invalid("Expected a session_key");
    };
    let key = match SessionKey::from_key_string(session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format")
        }
    };
    match sessions.get_metadata(&key).await {
        Ok(Some(meta)) if visibility::session_visible(&meta) => {}
        _ => return visibility::not_found_response(request.id),
    }

    let typed = params.get("action").is_some();
    let outcome = if typed {
        let Ok(params) = serde_json::from_value::<ClarificationActionParams>(params) else {
            return invalid("Invalid clarification action");
        };
        if params.expected_request_id.trim().is_empty()
            || params.expected_question_id.trim().is_empty()
        {
            return invalid("Both clarification identities are required");
        }
        manager.resolve_action(params).await
    } else {
        let has_reply = params.get("reply").is_some();
        let has_answers = params.get("answers").is_some();
        if has_reply == has_answers {
            return invalid("Supply exactly one of reply or answers");
        }
        let Ok(params) = serde_json::from_value::<ClarificationResolveParams>(params) else {
            return invalid("Invalid legacy clarification params");
        };
        let (Some(request_id), Some(question_id)) = (
            params
                .expected_request_id
                .as_deref()
                .filter(|id| !id.trim().is_empty()),
            params
                .expected_question_id
                .as_deref()
                .filter(|id| !id.trim().is_empty()),
        ) else {
            return invalid("Both clarification identities are required");
        };
        let replies: Vec<&str> = if has_reply {
            vec![params.reply.as_str()]
        } else {
            params.answers.iter().map(String::as_str).collect()
        };
        if replies.is_empty() || replies.iter().any(|reply| reply.trim().is_empty()) {
            return invalid("Expected nonblank clarification replies");
        }
        manager
            .resolve_many_expected(
                &params.session_key,
                &replies,
                Some(request_id),
                Some(question_id),
            )
            .await
    };

    let (status, pending_questions) = match outcome {
        ResolveOutcome::Invalid(message) => return invalid(message),
        ResolveOutcome::More { remaining, .. } => (ClarificationActionStatus::Advanced, remaining),
        ResolveOutcome::Done => (ClarificationActionStatus::Answered, 0),
        ResolveOutcome::Cancelled => (ClarificationActionStatus::Cancelled, 0),
        ResolveOutcome::Stale => (ClarificationActionStatus::Stale, 0),
    };
    let resolved = outcome.consumed();
    let stale = status == ClarificationActionStatus::Stale;
    let result = if typed {
        json!(ClarificationActionResponse {
            status,
            resolved,
            pending_questions,
            stale
        })
    } else {
        json!(ClarificationResolveResponse {
            resolved,
            pending_questions,
            stale
        })
    };
    JsonRpcResponse::success(request.id, result)
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
        ClarificationOption, ClarificationQuestion, ClarificationRequest, ClarificationResultType,
        DEFAULT_CLARIFY_TIMEOUT,
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
                max_messages: 100,
                compaction_keep: 50,
                session_expiry_secs: 30 * 24 * 60 * 60,
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

    fn resolve_request(session_key: &str, request_id: &str, reply: &str) -> JsonRpcRequest {
        JsonRpcRequest::new(
            "clarification.resolve",
            Some(json!({ "session_key": session_key, "reply": reply,
                "expected_request_id": request_id, "expected_question_id": "answer" })),
            Some(json!(1)),
        )
    }

    #[tokio::test]
    async fn typed_submit_rejects_invalid_suffix_and_preserves_input_intent() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        let key = "agent:main:main";
        create_session(&sess, key, None).await;
        let menu = || {
            vec![
                ClarificationOption::new("red", "Red"),
                ClarificationOption::new("green", "Green"),
                ClarificationOption::new("blue", "Blue"),
            ]
        };
        let (request_id, mut rx) = mgr
            .register_with_identity(
                key,
                ClarificationRequest::new(vec![
                    ClarificationQuestion::select("q1", "Colour?", menu()),
                    ClarificationQuestion::select("q2", "Colours?", menu()).with_multi_select(true),
                ])
                .unwrap(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;
        let first = json!({"question_id":"q1", "answer":{"kind":"text","text":"1"}});
        let second = json!({"question_id":"q2", "answer":{"kind":"choices","indices":[0,2]}});
        let submit = |answers| {
            JsonRpcRequest::with_id(
                "clarification.resolve",
                Some(json!({
                    "session_key": key, "expected_request_id": request_id, "expected_question_id": "q1",
                    "action": {"kind":"submit", "answers": answers}
                })),
                json!(1),
            )
        };
        for invalid in [
            json!([first]),
            json!([first, second, second]),
            json!([second, first]),
            json!([first, first]),
            json!([first, {"answer":{"kind":"choices","indices":[1]}}]),
            json!([first, {"question_id":"q2", "answer":{"kind":"choices","indices":[1,1]}}]),
            json!([first, {"question_id":"q2", "answer":{"kind":"choices","indices":[3]}}]),
            json!([first, {"question_id":"q2", "answer":{"kind":"choices","indices":[]}}]),
            json!([{"question_id":"q1", "answer":{"kind":"choices","indices":[0,1]}}, second]),
            json!([first, {"question_id":"q2", "answer":{"kind":"text","text":" \t "}}]),
            json!([first, {"question_id":"q2", "answer":{"kind":"text","text":"x".repeat(10_001)}}]),
        ] {
            let response =
                handle_resolve(submit(invalid), Arc::clone(&mgr), Arc::clone(&sess)).await;
            assert_eq!(
                response
                    .error
                    .expect("invalid answer must not acknowledge")
                    .code,
                INVALID_PARAMS
            );
            assert_eq!(mgr.list_pending().await[0].answered, 0);
            assert!(matches!(
                rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
        }

        let response = handle_resolve(submit(json!([first, second])), mgr, sess).await;
        assert_eq!(
            response.result.unwrap(),
            json!({
                "status":"answered", "resolved":true, "pending_questions":0, "stale":false,
            })
        );
        let answers = rx.await.unwrap().answers;
        assert_eq!(answers[0].value, "1");
        assert!(answers[0].selected_indices.is_empty());
        assert_eq!(answers[1].selected_indices, vec![0, 2]);
        assert_eq!(answers[1].value, "red, blue");
    }

    #[tokio::test]
    async fn typed_advance_and_cancel_apply_only_to_the_displayed_question() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        let key = "agent:main:main";
        create_session(&sess, key, None).await;
        let (request_id, rx) = mgr
            .register_with_identity(
                key,
                ClarificationRequest::new(vec![
                    ClarificationQuestion::text("q1", "First?"),
                    ClarificationQuestion::text("q2", "Second?"),
                ])
                .unwrap(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;
        let call = |question_id, action| {
            handle_resolve(
                JsonRpcRequest::with_id(
                    "clarification.resolve",
                    Some(json!({
                        "session_key": key, "expected_request_id": request_id,
                        "expected_question_id": question_id, "action": action,
                    })),
                    json!(1),
                ),
                Arc::clone(&mgr),
                Arc::clone(&sess),
            )
        };
        let answer = json!({"kind":"answer", "answer":{
            "question_id":"q1", "answer":{"kind":"text","text":"first"},
        }});
        let (first, second) = tokio::join!(call("q1", answer.clone()), call("q1", answer));
        let mut statuses = [first.result.unwrap(), second.result.unwrap()];
        statuses.sort_by(|a, b| a["status"].as_str().cmp(&b["status"].as_str()));
        assert_eq!(
            statuses[0],
            json!({
                "status":"advanced", "resolved":true, "pending_questions":1, "stale":false,
            })
        );
        assert_eq!(statuses[1]["status"], "stale");
        let stale = call("q1", json!({"kind":"cancel"})).await.result.unwrap();
        assert_eq!(stale["status"], "stale");
        assert_eq!(mgr.list_pending().await[0].answered, 1);
        let cancelled = call("q2", json!({"kind":"cancel"})).await.result.unwrap();
        assert_eq!(
            cancelled,
            json!({
                "status":"cancelled", "resolved":false, "pending_questions":0, "stale":false,
            })
        );
        let result = rx.await.unwrap();
        assert_eq!(result.result_type, ClarificationResultType::Cancelled);
        assert!(result.answers.is_empty());
    }

    #[tokio::test]
    async fn resolve_rejects_unguarded_or_ambiguous_input_without_advancing() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        let key = "agent:main:main";
        create_session(&sess, key, None).await;
        let (request_id, rx) = mgr
            .register_with_identity(
                key,
                ClarificationRequest::new(vec![
                    ClarificationQuestion::text("q1", "First?"),
                    ClarificationQuestion::text("q2", "Second?"),
                ])
                .unwrap(),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;
        let base = json!({"session_key":key, "expected_request_id":request_id, "expected_question_id":"q1"});
        let mut cases = vec![];
        for extra in [
            json!({}),
            json!({"reply":"","answers":[]}),
            json!({"reply":"answer","answers":[]}),
            json!({"reply":" \t "}),
            json!({"answers":["first", ""]}),
            json!({"answers":["first", "second", "extra"]}),
            json!({"reply":"old", "action":null}),
            json!({"reply":null, "action":{"kind":"cancel"}}),
            json!({"answers":[], "action":{"kind":"cancel"}}),
            json!({"reply":"first", "expected_request_id":""}),
            json!({"reply":"first", "expected_question_id":" "}),
        ] {
            let mut params = base.clone();
            params
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            cases.push(params);
        }
        for id in ["expected_request_id", "expected_question_id"] {
            let mut missing = base.clone();
            missing.as_object_mut().unwrap().remove(id);
            missing["reply"] = json!("first");
            cases.push(missing);
        }
        for invalid in cases {
            let rpc =
                JsonRpcRequest::with_id("clarification.resolve", Some(invalid.clone()), json!(1));
            let response = handle_resolve(rpc, Arc::clone(&mgr), Arc::clone(&sess)).await;
            assert_eq!(
                response
                    .error
                    .unwrap_or_else(|| panic!("unsupported input acknowledged: {invalid}"))
                    .code,
                INVALID_PARAMS
            );
            assert_eq!(mgr.list_pending().await[0].answered, 0);
        }
        let mut malformed = base;
        malformed["action"] = json!({"kind":"answer", "answer":"private-input-marker"});
        let request = JsonRpcRequest::with_id("clarification.resolve", Some(malformed), json!(1));
        let error = handle_resolve(request, Arc::clone(&mgr), Arc::clone(&sess))
            .await
            .error
            .unwrap();
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(!error.message.contains("private-input-marker"));
        assert_eq!(mgr.list_pending().await[0].answered, 0);

        let partial = JsonRpcRequest::with_id(
            "clarification.resolve",
            Some(json!({
                "session_key":key, "expected_request_id":request_id, "expected_question_id":"q1",
                "answers":["first"],
            })),
            json!(1),
        );
        let response = handle_resolve(partial, Arc::clone(&mgr), Arc::clone(&sess)).await;
        assert_eq!(
            response.result.unwrap(),
            json!({"resolved":true, "pending_questions":1, "stale":false})
        );
        let last = JsonRpcRequest::with_id(
            "clarification.resolve",
            Some(json!({
                "session_key":key, "expected_request_id":request_id, "expected_question_id":"q2", "reply":"second",
            })),
            json!(1),
        );
        let response = handle_resolve(last, mgr, sess).await;
        assert_eq!(
            response.result.unwrap(),
            json!({"resolved":true, "pending_questions":0, "stale":false})
        );
        let result = rx.await.unwrap();
        assert_eq!(
            result
                .answers
                .iter()
                .map(|a| a.value.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    /// The whole point of the RPC: it is the Panel's only path to the oneshot
    /// the `ask_user` tool is blocked on.
    #[tokio::test]
    async fn resolve_unblocks_the_parked_ask_user() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;
        let (id, rx) = mgr
            .register_with_identity(
                "agent:main:main",
                ClarificationRequest::select(
                    "Deploy where?",
                    vec![
                        ClarificationOption::new("staging", "staging"),
                        ClarificationOption::new("production", "production"),
                    ],
                ),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;

        // A guarded legacy reply can still use a 1-based option number.
        let response =
            handle_resolve(resolve_request("agent:main:main", &id, "2"), mgr, sess).await;
        assert!(response.is_success(), "{:?}", response.error);
        assert_eq!(response.result.unwrap()["resolved"], true);

        let result = rx.await.expect("the parked tool must be unblocked");
        assert_eq!(result.selected_index(), Some(1));
        assert_eq!(result.value(), Some("production"));
    }

    #[tokio::test]
    async fn resolve_free_text_is_taken_verbatim() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;
        let (id, rx) = mgr
            .register_with_identity(
                "agent:main:main",
                ClarificationRequest::text("Which file?"),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;

        let response = handle_resolve(
            resolve_request("agent:main:main", &id, "src/main.rs"),
            mgr,
            sess,
        )
        .await;
        assert_eq!(response.result.unwrap()["resolved"], true);
        assert_eq!(rx.await.unwrap().value(), Some("src/main.rs"));
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
            resolve_request("agent:main:main", "not-present", "yes"),
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
        let (id, rx) = mgr
            .register_with_identity(
                "agent:main:main",
                ClarificationRequest::text("Which file?"),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
            )
            .await;
        let first = handle_resolve(
            resolve_request("agent:main:main", &id, "a.rs"),
            mgr.clone(),
            sess.clone(),
        )
        .await;
        assert_eq!(first.result.unwrap()["resolved"], true);
        let _ = rx.await;

        let second =
            handle_resolve(resolve_request("agent:main:main", &id, "b.rs"), mgr, sess).await;
        assert!(second.is_success());
        assert_eq!(second.result.unwrap()["resolved"], false);
    }

    /// A malformed `session_key` is a validation error, not an existence
    /// question — same convention `artifacts.rs`/`sessions.*` use.
    #[tokio::test]
    async fn resolve_rejects_a_malformed_session_key() {
        let mgr = manager();
        let (_tmp, sess) = sessions();

        let response = handle_resolve(
            resolve_request("not-a-session", "not-present", "yes"),
            mgr,
            sess,
        )
        .await;
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
        let (request_id, rx) = mgr
            .register_with_identity(
                "agent:main:main",
                ClarificationRequest::text("Which file?"),
                DEFAULT_CLARIFY_TIMEOUT,
                "run-alice",
            )
            .await;
        let pending = mgr.list_pending().await;
        let question_id = &pending[0].questions[0].id;
        let guarded = |reply: &str| {
            JsonRpcRequest::new(
                "clarification.resolve",
                Some(json!({ "session_key": "agent:main:main", "reply": reply,
                "expected_request_id": request_id, "expected_question_id": question_id })),
                Some(json!(1)),
            )
        };

        let bob_resp = CALLER_USER
            .scope(Some("u-bob".to_string()), async {
                handle_resolve(guarded("src/evil.rs"), mgr.clone(), sess.clone()).await
            })
            .await;
        assert_eq!(
            bob_resp.error.expect("expected error").code,
            RESOURCE_NOT_FOUND
        );

        // The waiter is untouched — alice can still answer it.
        let alice_resp = CALLER_USER
            .scope(Some("u-alice".to_string()), async {
                handle_resolve(guarded("src/main.rs"), mgr, sess).await
            })
            .await;
        assert_eq!(alice_resp.result.unwrap()["resolved"], true);
        assert_eq!(rx.await.unwrap().value(), Some("src/main.rs"));
    }

    #[tokio::test]
    async fn guarded_rpc_rejects_a_superseded_request_without_consuming_its_successor() {
        let mgr = manager();
        let (_tmp, sess) = sessions();
        create_session(&sess, "agent:main:main", None).await;
        let (old, old_rx) = mgr
            .register_with_identity(
                "agent:main:main",
                ClarificationRequest::text("Old?"),
                DEFAULT_CLARIFY_TIMEOUT,
                "r1",
            )
            .await;
        let (current, current_rx) = mgr
            .register_with_identity(
                "agent:main:main",
                ClarificationRequest::text("New?"),
                DEFAULT_CLARIFY_TIMEOUT,
                "r2",
            )
            .await;
        assert_eq!(
            old_rx.await.unwrap().result_type,
            crate::clarification::ClarificationResultType::Cancelled
        );
        let guarded = |id: &str| {
            JsonRpcRequest::new(
                "clarification.resolve",
                Some(json!({ "session_key": "agent:main:main", "reply": "answer",
                "expected_request_id": id, "expected_question_id": "answer" })),
                Some(json!(1)),
            )
        };
        let stale = handle_resolve(guarded(&old), mgr.clone(), sess.clone()).await;
        assert!(stale.is_success());
        assert_eq!(stale.result.unwrap()["stale"], true);
        assert_eq!(mgr.list_pending().await[0].answered, 0);
        let done = handle_resolve(guarded(&current), mgr, sess).await;
        assert_eq!(done.result.unwrap()["resolved"], true);
        assert_eq!(current_rx.await.unwrap().value(), Some("answer"));
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
                "",
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
                "",
            )
            .await;
        let _bob_rx = mgr
            .register(
                "agent:main:main:s1",
                ClarificationRequest::text("Bob's question?"),
                DEFAULT_CLARIFY_TIMEOUT,
                "",
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
