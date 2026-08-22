//! Group Chat RPC handlers.
//!
//! Handlers for group chat operations: start, continue, mention, end, list, history.
//!
//! Each method has two variants:
//! - `handle_xxx_placeholder`: stateless placeholders returning errors (used in `HandlerRegistry::new()`)
//! - `handle_xxx`: real handlers that delegate to `GroupChatOrchestrator` + `GroupChatExecutor`
//!
//! All real handlers follow the per-session locking pattern:
//!  1. Briefly lock the orchestrator to obtain a `SharedSession` handle
//!  2. Drop the orchestrator lock
//!  3. Lock only the target session for the duration of the operation
//!
//! This allows different sessions to proceed concurrently.
//!
//! ## P1 visibility (final-review finding C3)
//!
//! A group-chat session is per-user state: `list` used to enumerate EVERY
//! user's active sessions (id + topic + participants), and the id it handed
//! out is the only thing `continue`/`mention`/`history`/`end` need — so the
//! list was an enumeration oracle feeding four addressed surfaces, one of
//! which returns the full conversation and two of which mutate it.
//!
//! Ownership comes from the stamp `GroupChatSession::new` takes off the
//! ambient scope, read through the one shared predicate
//! [`visibility::stamped_owner_visible`] (never re-derived here). `list`
//! filters per item; the four addressed methods reuse each one's OWN existing
//! "no such session" response verbatim, so a session that belongs to someone
//! else is indistinguishable from one that never existed.

use crate::sync_primitives::Arc;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::visibility;
use crate::group_chat::{
    GroupChatExecutor, GroupChatMessage, GroupChatOrchestrator, GroupChatStatus, PersonaSource,
};

/// Shared `GroupChatOrchestrator` handle for real handlers
pub type SharedOrchestrator = Arc<Mutex<GroupChatOrchestrator>>;

// ============================================================================
// Helper functions
// ============================================================================

/// Extract a string parameter from a JSON-RPC request
fn extract_str(request: &JsonRpcRequest, key: &str) -> Option<String> {
    match &request.params {
        Some(Value::Object(map)) => map.get(key).and_then(|v| v.as_str()).map(|s| s.to_string()),
        _ => None,
    }
}

/// Extract a string array parameter from a JSON-RPC request
fn extract_str_array(request: &JsonRpcRequest, key: &str) -> Vec<String> {
    match &request.params {
        Some(Value::Object(map)) => map
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Serialize a `GroupChatMessage` to JSON
fn message_to_json(msg: &GroupChatMessage) -> Value {
    json!({
        "session_id": msg.session_id,
        "speaker": msg.speaker.name(),
        "content": msg.content,
        "round": msg.round,
        "sequence": msg.sequence,
        "is_final": msg.is_final,
    })
}

// ============================================================================
// Real handlers (backed by GroupChatOrchestrator + GroupChatExecutor)
// ============================================================================

/// Handle `group_chat.start` RPC request (real)
///
/// Creates a new group chat session. If `initial_message` is provided,
/// executes the first round immediately and returns messages.
pub async fn handle_start(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map.clone(),
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    // Extract personas (required)
    let personas_value = match params.get("personas") {
        Some(v) => v.clone(),
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing personas");
        }
    };

    let personas: Vec<PersonaSource> = match serde_json::from_value(personas_value) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid personas: {e}"),
            );
        }
    };

    // Extract optional params
    let topic = params
        .get("topic")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let initial_message = params
        .get("initial_message")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let source_channel = params
        .get("source_channel")
        .and_then(|v| v.as_str())
        .unwrap_or("rpc")
        .to_string();
    let source_session_key = params
        .get("source_session_key")
        .and_then(|v| v.as_str())
        .unwrap_or("rpc:direct")
        .to_string();

    // Brief orch lock: create session and get handle
    let (session_id, session_handle) = {
        let mut orch_guard = orch.lock().await;
        match orch_guard
            .create_session(personas, topic, source_channel, source_session_key)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to create session: {e}"),
                );
            }
        }
    }; // orch lock dropped

    // If initial_message provided, execute the first round (only session locked)
    if let Some(msg) = initial_message {
        let mut session = session_handle.lock().await;
        match executor.execute_round(&mut session, &msg, &[]).await {
            Ok(messages) => {
                let messages_json: Vec<Value> = messages.iter().map(message_to_json).collect();
                JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_id": session_id,
                        "messages": messages_json,
                    }),
                )
            }
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to execute initial round: {e}"),
            ),
        }
    } else {
        JsonRpcResponse::success(
            request.id,
            json!({
                "session_id": session_id,
            }),
        )
    }
}

/// Handle `group_chat.continue` RPC request (real)
///
/// Continues an existing group chat session with a new message.
pub async fn handle_continue(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    handle_continue_with_targets(request, orch, executor, &[]).await
}

/// Internal handler for continue/mention with optional targets.
async fn handle_continue_with_targets(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
    targets: &[String],
) -> JsonRpcResponse {
    let session_id = match extract_str(&request, "session_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_id");
        }
    };

    let message = match extract_str(&request, "message") {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing message");
        }
    };

    // Brief orch lock: get session handle + max_rounds config
    let (session_handle, max_rounds) = {
        let orch_guard = orch.lock().await;
        let handle = match orch_guard.get_session(&session_id) {
            Some(h) => h,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Session not found: {session_id}"),
                );
            }
        };
        (handle, orch_guard.max_rounds())
    }; // orch lock dropped

    // Lock session, check round limit, execute
    let mut session = session_handle.lock().await;

    // P1: a foreign session gets this method's OWN not-found response, so a
    // denial and a nonexistent id are indistinguishable. Checked before the
    // round-limit branch — that branch ENDS the session, which would let a
    // non-owner kill somebody else's group chat and learn from the distinct
    // error that it existed.
    if !visibility::stamped_owner_visible(session.owner_user_id.as_deref()) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Session not found: {session_id}"),
        );
    }

    if session.current_round >= max_rounds {
        session.end();
        drop(session);

        {
            let mut orch_guard = orch.lock().await;
            orch_guard.end_session(&session_id);
        }

        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Round limit exceeded: maximum rounds reached: {max_rounds}"),
        );
    }

    match executor
        .execute_round(&mut session, &message, targets)
        .await
    {
        Ok(messages) => {
            let messages_json: Vec<Value> = messages.iter().map(message_to_json).collect();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_id": session_id,
                    "messages": messages_json,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to execute round: {e}"),
        ),
    }
}

/// Handle `group_chat.mention` RPC request (real)
///
/// Like continue, but extracts `targets` and passes them to the coordinator
/// so it prioritizes the mentioned personas.
pub async fn handle_mention(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    let targets = extract_str_array(&request, "targets");
    handle_continue_with_targets(request, orch, executor, &targets).await
}

/// Handle `group_chat.end` RPC request (real)
///
/// Ends a group chat session.
pub async fn handle_end(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
    let session_id = match extract_str(&request, "session_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_id");
        }
    };

    // P1: resolve and check ownership BEFORE removing anything — `end_session`
    // is destructive, so the check cannot live after it. Peek through
    // `get_session` first (orch lock taken and dropped before the session lock,
    // preserving this module's never-hold-both rule), and reuse this method's
    // own not-found response for a foreign owner.
    let not_found = |id: Option<Value>| {
        JsonRpcResponse::error(
            id,
            INTERNAL_ERROR,
            format!("Session not found: {session_id}"),
        )
    };

    let peek = {
        let orch_guard = orch.lock().await;
        orch_guard.get_session(&session_id)
    }; // orch lock dropped
    let Some(peek) = peek else {
        return not_found(request.id);
    };
    {
        let s = peek.lock().await;
        if !visibility::stamped_owner_visible(s.owner_user_id.as_deref()) {
            return not_found(request.id);
        }
    } // session lock dropped

    // Lock orchestrator: end session and remove from map
    let session_handle = {
        let mut orch_guard = orch.lock().await;
        match orch_guard.end_session(&session_id).await {
            Some(h) => h,
            None => return not_found(request.id),
        }
    }; // orch lock dropped

    // Mark session as ended
    let mut session = session_handle.lock().await;
    session.end();

    JsonRpcResponse::success(request.id, json!({ "ended": session_id }))
}

/// Handle `group_chat.list` RPC request (real)
///
/// Returns a list of all active group chat sessions.
pub async fn handle_list(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
    // Brief orch lock: snapshot all session handles
    let all = {
        let orch_guard = orch.lock().await;
        orch_guard.all_sessions()
    }; // orch lock dropped

    // Lock each session individually to read data
    let mut sessions_json: Vec<Value> = Vec::with_capacity(all.len());
    for (_id, handle) in &all {
        let s = handle.lock().await;
        if s.status != GroupChatStatus::Active {
            continue;
        }
        // P1: per-item filter, the `ListFiltered` shape. An unrestricted
        // caller keeps the pre-P1 whole-process view; a scoped caller sees
        // only their own sessions, and never an error — an empty list is a
        // valid answer, and it is what stops this method being the
        // enumeration oracle the four addressed methods below feed off.
        if !visibility::stamped_owner_visible(s.owner_user_id.as_deref()) {
            continue;
        }

        let participants: Vec<Value> = s
            .participants
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                })
            })
            .collect();

        sessions_json.push(json!({
            "id": s.id,
            "topic": s.topic,
            "participants": participants,
            "current_round": s.current_round,
            "status": s.status.as_str(),
            "created_at": s.created_at,
        }));
    }

    JsonRpcResponse::success(request.id, json!({ "sessions": sessions_json }))
}

/// Handle `group_chat.history` RPC request (real)
///
/// Returns the conversation history for a group chat session.
pub async fn handle_history(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
    let session_id = match extract_str(&request, "session_id") {
        Some(id) => id,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_id");
        }
    };

    // Brief orch lock: get session handle
    let session_handle = {
        let orch_guard = orch.lock().await;
        match orch_guard.get_session(&session_id) {
            Some(h) => h,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Session not found: {session_id}"),
                );
            }
        }
    }; // orch lock dropped

    // Lock session, read history
    let session = session_handle.lock().await;

    // P1: the whole conversation is behind this call, so the check comes
    // before a single turn is serialized. Same not-found shape as a missing
    // id above.
    if !visibility::stamped_owner_visible(session.owner_user_id.as_deref()) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Session not found: {session_id}"),
        );
    }

    let history: Vec<Value> = session
        .history
        .iter()
        .map(|turn| {
            json!({
                "round": turn.round,
                "speaker": turn.speaker.name(),
                "content": turn.content,
                "timestamp": turn.timestamp,
            })
        })
        .collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "session_id": session_id,
            "history": history,
            "current_round": session.current_round,
        }),
    )
}

// ============================================================================
// Placeholder handlers (stateless, for HandlerRegistry::new())
// ============================================================================

const RUNTIME_REQUIRED: &str = "requires GroupChatOrchestrator runtime - wire Gateway first";

pub async fn handle_start_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.start {RUNTIME_REQUIRED}"),
    )
}

pub async fn handle_continue_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.continue {RUNTIME_REQUIRED}"),
    )
}

pub async fn handle_mention_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.mention {RUNTIME_REQUIRED}"),
    )
}

pub async fn handle_end_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.end {RUNTIME_REQUIRED}"),
    )
}

pub async fn handle_list_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.list {RUNTIME_REQUIRED}"),
    )
}

pub async fn handle_history_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.history {RUNTIME_REQUIRED}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::group_chat::GroupChatConfig;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::protocol::JsonRpcRequest;
    use crate::group_chat::Persona;
    use serde_json::json;

    fn orch() -> SharedOrchestrator {
        Arc::new(Mutex::new(GroupChatOrchestrator::new(
            GroupChatConfig::default(),
            &[],
        )))
    }

    fn personas() -> Vec<PersonaSource> {
        vec![PersonaSource::Inline(Persona {
            id: "p1".into(),
            name: "Analyst".into(),
            system_prompt: "you analyse".into(),
            provider: None,
            model: None,
            thinking_level: None,
        })]
    }

    /// Create a session attributed to `owner`, exactly the way a dispatch
    /// does — the stamp is taken off the ambient scope inside
    /// `GroupChatSession::new`.
    async fn session_owned_by(orch: &SharedOrchestrator, owner: &str) -> String {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(owner)),
            async {
                let mut g = orch.lock().await;
                g.create_session(
                    personas(),
                    Some("secret topic".into()),
                    "rpc".into(),
                    "rpc:direct".into(),
                )
                .await
                .map(|(id, _)| id)
                .unwrap()
            },
        )
        .await
    }

    fn rpc(method: &str, params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id(method, Some(params), json!(1))
    }

    /// C3: `list` used to hand every user's active session id + topic to
    /// anyone who asked, which is what made the four addressed methods
    /// reachable at all.
    #[tokio::test]
    async fn list_hides_another_users_sessions_but_keeps_your_own() {
        let orch = orch();
        let alice = session_owned_by(&orch, "u-alice").await;
        let bob = session_owned_by(&orch, "u-bob").await;

        let seen = |resp: JsonRpcResponse| -> Vec<String> {
            resp.result.expect("success, never an error")["sessions"]
                .as_array()
                .expect("sessions array")
                .iter()
                .map(|s| s["id"].as_str().unwrap_or_default().to_string())
                .collect()
        };

        let bobs = seen(
            CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_list(rpc("group_chat.list", json!({})), orch.clone()),
                )
                .await,
        );
        assert!(bobs.contains(&bob), "bob must see his own session");
        assert!(
            !bobs.contains(&alice),
            "bob must not see alice's session: {bobs:?}"
        );

        // An unrestricted (internal) caller keeps the pre-P1 whole view.
        let all = seen(handle_list(rpc("group_chat.list", json!({})), orch).await);
        assert!(all.contains(&alice) && all.contains(&bob));
    }

    /// The addressed reads/mutations deny with each method's OWN not-found
    /// response, so a foreign id and an unknown id are indistinguishable —
    /// and the destructive one leaves the session intact.
    #[tokio::test]
    async fn addressed_methods_deny_a_foreign_session_as_not_found() {
        let orch = orch();
        let alice = session_owned_by(&orch, "u-alice").await;
        let unknown = "gc-does-not-exist";

        let history_denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_history(
                    rpc("group_chat.history", json!({ "session_id": alice })),
                    orch.clone(),
                ),
            )
            .await;
        let history_unknown = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_history(
                    rpc("group_chat.history", json!({ "session_id": unknown })),
                    orch.clone(),
                ),
            )
            .await;
        assert!(history_denied.result.is_none(), "no history may be served");
        assert_eq!(
            history_denied.error.as_ref().map(|e| e.message.clone()),
            Some(format!("Session not found: {alice}")),
        );
        assert_eq!(
            history_denied.error.as_ref().map(|e| e.code),
            history_unknown.error.as_ref().map(|e| e.code),
            "a foreign session and an unknown one must share the error shape"
        );

        let end_denied = CALLER_USER
            .scope(
                Some("u-bob".to_string()),
                handle_end(
                    rpc("group_chat.end", json!({ "session_id": alice })),
                    orch.clone(),
                ),
            )
            .await;
        assert!(end_denied.result.is_none());
        assert_eq!(
            end_denied.error.as_ref().map(|e| e.message.clone()),
            Some(format!("Session not found: {alice}")),
        );

        // Destructive denial must leave the data intact: alice still has it.
        let alice_list = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_list(rpc("group_chat.list", json!({})), orch),
            )
            .await;
        let ids: Vec<String> = alice_list.result.expect("success")["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            ids.contains(&alice),
            "a denied group_chat.end must not have ended the session: {ids:?}"
        );
    }

    #[test]
    fn test_group_chat_handlers_registered() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        assert!(registry.has_method("group_chat.start"));
        assert!(registry.has_method("group_chat.continue"));
        assert!(registry.has_method("group_chat.mention"));
        assert!(registry.has_method("group_chat.end"));
        assert!(registry.has_method("group_chat.list"));
        assert!(registry.has_method("group_chat.history"));
    }

    #[tokio::test]
    async fn test_start_placeholder_returns_error() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        let req = JsonRpcRequest::with_id("group_chat.start", Some(json!({})), json!(1));
        let resp = registry.handle(&req).await;
        assert!(resp.is_error());
    }
}
