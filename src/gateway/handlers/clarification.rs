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

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::HandlerRegistry;
use crate::clarification::session::PendingClarification;
use crate::clarification::ClarificationManager;

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
pub fn register_handlers(registry: &mut HandlerRegistry, manager: Arc<ClarificationManager>) {
    {
        let m = manager.clone();
        registry.register("clarification.resolve", move |req| {
            let m = m.clone();
            async move { handle_resolve(req, m).await }
        });
    }
    {
        let m = manager.clone();
        registry.register("clarification.pending", move |req| {
            let m = m.clone();
            async move { handle_pending(req, m).await }
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
async fn handle_resolve(
    request: JsonRpcRequest,
    manager: Arc<ClarificationManager>,
) -> JsonRpcResponse {
    let params: ClarificationResolveParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

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
async fn handle_pending(
    request: JsonRpcRequest,
    manager: Arc<ClarificationManager>,
) -> JsonRpcResponse {
    let pending = manager.list_pending().await;
    JsonRpcResponse::success(request.id, json!(PendingListResponse { pending }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarification::{
        ClarificationOption, ClarificationRequest, DEFAULT_CLARIFY_TIMEOUT,
    };

    fn manager() -> Arc<ClarificationManager> {
        Arc::new(ClarificationManager::new())
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
        let rx = mgr
            .register(
                "gui:chat:main",
                ClarificationRequest::select(
                    "ask-1",
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
        let response = handle_resolve(resolve_request("gui:chat:main", "2"), mgr).await;
        assert!(response.is_success());
        assert_eq!(response.result.unwrap()["resolved"], true);

        let result = rx.await.expect("the parked tool must be unblocked");
        assert_eq!(result.selected_index, Some(1));
        assert_eq!(result.get_value(), Some("production"));
    }

    #[tokio::test]
    async fn resolve_free_text_is_taken_verbatim() {
        let mgr = manager();
        let rx = mgr
            .register(
                "gui:chat:main",
                ClarificationRequest::text("ask-2", "Which file?", None),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        let response = handle_resolve(resolve_request("gui:chat:main", "src/main.rs"), mgr).await;
        assert_eq!(response.result.unwrap()["resolved"], true);
        assert_eq!(rx.await.unwrap().get_value(), Some("src/main.rs"));
    }

    /// A stale answer (the question was already resolved elsewhere, superseded,
    /// or timed out) must be a silent no-op, not a JSON-RPC error — same as a
    /// stale `clarify:` tap in the inbound router.
    #[tokio::test]
    async fn stale_resolve_is_a_silent_no_op() {
        let mgr = manager();
        let response = handle_resolve(resolve_request("no-such-session", "yes"), mgr.clone()).await;
        assert!(
            response.is_success(),
            "a stale answer must not surface as an error"
        );
        assert_eq!(response.result.unwrap()["resolved"], false);

        // Same for a session whose question was already answered.
        let rx = mgr
            .register(
                "gui:chat:main",
                ClarificationRequest::text("ask-3", "Which file?", None),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;
        let first = handle_resolve(resolve_request("gui:chat:main", "a.rs"), mgr.clone()).await;
        assert_eq!(first.result.unwrap()["resolved"], true);
        let _ = rx.await;

        let second = handle_resolve(resolve_request("gui:chat:main", "b.rs"), mgr).await;
        assert!(second.is_success());
        assert_eq!(second.result.unwrap()["resolved"], false);
    }

    #[tokio::test]
    async fn pending_lists_the_live_question() {
        let mgr = manager();
        let _rx = mgr
            .register(
                "gui:chat:main",
                ClarificationRequest::select(
                    "ask-4",
                    "Deploy where?",
                    vec![ClarificationOption::new("staging", "staging")],
                ),
                DEFAULT_CLARIFY_TIMEOUT,
            )
            .await;

        let request = JsonRpcRequest::with_id("clarification.pending", None, json!(1));
        let response = handle_pending(request, mgr).await;

        assert!(response.is_success());
        let pending = response.result.unwrap();
        let pending = pending["pending"].as_array().expect("pending array");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["session_key"], "gui:chat:main");
        assert_eq!(pending[0]["question"], "Deploy where?");
        assert_eq!(pending[0]["options"][0], "staging");
    }

    #[tokio::test]
    async fn register_handlers_registers_all_methods() {
        let mut registry = HandlerRegistry::empty();
        register_handlers(&mut registry, manager());
        for m in ["clarification.resolve", "clarification.pending"] {
            assert!(registry.has_method(m), "method {m} not registered");
        }
    }
}
