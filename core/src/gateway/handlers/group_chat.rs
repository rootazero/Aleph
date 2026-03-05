//! Group Chat RPC handlers.
//!
//! Handlers for group chat operations: start, continue, mention, end, list, history.
//!
//! Each method has two variants:
//! - `handle_xxx_placeholder`: stateless placeholders returning errors (used in HandlerRegistry::new())
//! - `handle_xxx`: real handlers that delegate to `GroupChatOrchestrator` + `GroupChatExecutor`
//!
//! All real handlers follow the per-session locking pattern:
//!   1. Briefly lock the orchestrator to obtain a `SharedSession` handle
//!   2. Drop the orchestrator lock
//!   3. Lock only the target session for the duration of the operation
//! This allows different sessions to proceed concurrently.

use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::group_chat::{
    GroupChatExecutor, GroupChatMessage, GroupChatOrchestrator, GroupChatStatus, PersonaSource,
};

/// Shared GroupChatOrchestrator handle for real handlers
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

/// Serialize a GroupChatMessage to JSON
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

/// Handle group_chat.start RPC request (real)
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
                format!("Invalid personas: {}", e),
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
        match orch_guard.create_session(personas, topic, source_channel, source_session_key) {
            Ok(pair) => pair,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to create session: {}", e),
                );
            }
        }
    }; // orch lock dropped

    // If initial_message provided, execute the first round (only session locked)
    if let Some(msg) = initial_message {
        let mut session = session_handle.lock().await;
        match executor.execute_round(&mut session, &msg).await {
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
                format!("Failed to execute initial round: {}", e),
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

/// Handle group_chat.continue RPC request (real)
///
/// Continues an existing group chat session with a new message.
pub async fn handle_continue(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
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
                    format!("Session not found: {}", session_id),
                );
            }
        };
        (handle, orch_guard.max_rounds())
    }; // orch lock dropped

    // Lock session, check round limit, execute
    let mut session = session_handle.lock().await;

    if session.current_round >= max_rounds {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!(
                "Round limit exceeded: maximum rounds reached: {}",
                max_rounds
            ),
        );
    }

    match executor.execute_round(&mut session, &message).await {
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
            format!("Failed to execute round: {}", e),
        ),
    }
}

/// Handle group_chat.mention RPC request (real)
///
/// Same as continue — the coordinator decides who responds based on message content.
pub async fn handle_mention(
    request: JsonRpcRequest,
    orch: SharedOrchestrator,
    executor: Arc<GroupChatExecutor>,
) -> JsonRpcResponse {
    // Mention works the same as continue: the coordinator decides who responds
    handle_continue(request, orch, executor).await
}

/// Handle group_chat.end RPC request (real)
///
/// Ends a group chat session.
pub async fn handle_end(request: JsonRpcRequest, orch: SharedOrchestrator) -> JsonRpcResponse {
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
                    INTERNAL_ERROR,
                    format!("Session not found: {}", session_id),
                );
            }
        }
    }; // orch lock dropped

    // Lock session, end it
    let mut session = session_handle.lock().await;
    session.end();

    JsonRpcResponse::success(request.id, json!({ "ended": session_id }))
}

/// Handle group_chat.list RPC request (real)
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

/// Handle group_chat.history RPC request (real)
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
                    format!("Session not found: {}", session_id),
                );
            }
        }
    }; // orch lock dropped

    // Lock session, read history
    let session = session_handle.lock().await;

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
        format!("group_chat.start {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_continue_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.continue {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_mention_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.mention {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_end_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.end {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_list_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.list {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_history_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.history {}", RUNTIME_REQUIRED),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use crate::gateway::protocol::JsonRpcRequest;

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
