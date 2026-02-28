//! Chat Handlers
//!
//! High-level chat control handlers that wrap agent operations with
//! simpler semantics for chat-focused clients.
//!
//! Methods:
//! - `chat.send` - Send a message (wraps agent.run)
//! - `chat.abort` - Abort message generation
//! - `chat.history` - Get chat history
//! - `chat.clear` - Clear chat history

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::debug;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::super::router::SessionKey;
use super::super::session_manager::SessionManager;
use super::agent::{AgentRunManager, AgentRunParams, Attachment};

// ============================================================================
// Request/Response Types
// ============================================================================

/// Parameters for chat.send request
#[derive(Debug, Clone, Deserialize)]
pub struct SendParams {
    /// User message content
    pub message: String,
    /// Optional session key (auto-generated if not provided)
    #[serde(default)]
    pub session_key: Option<String>,
    /// Channel identifier (e.g., "gui:window1", "cli:term1")
    #[serde(default)]
    pub channel: Option<String>,
    /// Whether to stream events (default: true)
    #[serde(default = "default_stream")]
    pub stream: bool,
    /// Thinking level for LLM reasoning depth
    #[serde(default)]
    pub thinking: Option<String>,
    /// File attachments sent with the message
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

fn default_stream() -> bool {
    true
}

/// Response for chat.send request
#[derive(Debug, Clone, Serialize)]
pub struct SendResponse {
    /// Unique run identifier
    pub run_id: String,
    /// Resolved session key
    pub session_key: String,
    /// Whether streaming is enabled
    pub streaming: bool,
}

/// Parameters for chat.abort request
#[derive(Debug, Clone, Deserialize)]
pub struct AbortParams {
    /// Run ID to abort
    pub run_id: String,
}

/// Parameters for chat.history request
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryParams {
    /// Session key to get history for
    pub session_key: String,
    /// Maximum number of messages to return
    #[serde(default)]
    pub limit: Option<usize>,
    /// Get messages before this timestamp (ISO 8601 or Unix timestamp)
    #[serde(default)]
    pub before: Option<String>,
}

/// Parameters for chat.clear request
#[derive(Debug, Clone, Deserialize)]
pub struct ClearParams {
    /// Session key to clear
    pub session_key: String,
    /// Whether to keep system messages (default: true)
    #[serde(default = "default_keep_system")]
    pub keep_system: bool,
}

fn default_keep_system() -> bool {
    true
}

/// A chat message in the history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role (user, assistant, system)
    pub role: String,
    /// Message content
    pub content: String,
    /// Timestamp (ISO 8601)
    pub timestamp: String,
    /// Optional run ID that generated this message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Handle chat.send RPC request
///
/// Sends a message and starts agent execution. This is a high-level wrapper
/// around `agent.run` with simpler semantics for chat-focused clients.
pub async fn handle_send(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: SendParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Validate message
    if params.message.trim().is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Message cannot be empty");
    }

    // Convert to AgentRunParams
    let agent_params = AgentRunParams {
        input: params.message,
        session_key: params.session_key,
        channel: params.channel,
        peer_id: None,
        stream: params.stream,
        thinking: params.thinking,
        attachments: params.attachments,
    };

    // Start the run
    match run_manager.start_run(agent_params).await {
        Ok(result) => {
            let response = SendResponse {
                run_id: result.run_id,
                session_key: result.session_key,
                streaming: params.stream,
            };
            JsonRpcResponse::success(request.id, json!(response))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

/// Handle chat.abort RPC request
///
/// Aborts an in-progress message generation.
pub async fn handle_abort(
    request: JsonRpcRequest,
    run_manager: Arc<AgentRunManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: AbortParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Cancel the run
    let cancelled = run_manager.cancel_run(&params.run_id).await;

    debug!(run_id = %params.run_id, cancelled = cancelled, "Chat abort requested");

    JsonRpcResponse::success(
        request.id,
        json!({
            "run_id": params.run_id,
            "aborted": cancelled,
        }),
    )
}

/// Handle chat.history RPC request
///
/// Returns the chat history for a session.
pub async fn handle_history(
    request: JsonRpcRequest,
    session_manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: HistoryParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse session key
    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    // Get history from session manager
    match session_manager.get_history(&session_key, params.limit).await {
        Ok(messages) => {
            let chat_messages: Vec<ChatMessage> = messages
                .into_iter()
                .map(|m| ChatMessage {
                    role: m.role,
                    content: m.content,
                    timestamp: chrono::DateTime::from_timestamp(m.timestamp, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                    run_id: m.metadata.and_then(|meta| {
                        serde_json::from_str::<Value>(&meta)
                            .ok()
                            .and_then(|v| v.get("run_id").and_then(|r| r.as_str()).map(String::from))
                    }),
                })
                .collect();

            let count = chat_messages.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": params.session_key,
                    "messages": chat_messages,
                    "count": count,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to get history: {}", e),
        ),
    }
}

/// Handle chat.clear RPC request
///
/// Clears the chat history for a session.
pub async fn handle_clear(
    request: JsonRpcRequest,
    session_manager: Arc<SessionManager>,
) -> JsonRpcResponse {
    // Parse params
    let params: ClearParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse session key
    let session_key = match SessionKey::from_key_string(&params.session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    debug!(
        session_key = %params.session_key,
        keep_system = params.keep_system,
        "Clearing chat history"
    );

    // Reset the session
    // Note: keep_system is currently not implemented in SessionManager
    // For now, we just reset all messages
    match session_manager.reset_session(&session_key).await {
        Ok(cleared) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": params.session_key,
                "cleared": cleared,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to clear history: {}", e),
        ),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_send_params_deserialization() {
        let json = json!({
            "message": "Hello, world!",
            "session_key": "agent:main:main",
            "stream": true
        });

        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Hello, world!");
        assert_eq!(params.session_key, Some("agent:main:main".to_string()));
        assert!(params.stream);
        assert!(params.thinking.is_none());
    }

    #[test]
    fn test_send_params_defaults() {
        let json = json!({
            "message": "Test"
        });

        let params: SendParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.message, "Test");
        assert!(params.session_key.is_none());
        assert!(params.channel.is_none());
        assert!(params.stream); // default true
        assert!(params.thinking.is_none());
    }

    #[test]
    fn test_send_response_serialization() {
        let response = SendResponse {
            run_id: "run-123".to_string(),
            session_key: "agent:main:main".to_string(),
            streaming: true,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["run_id"], "run-123");
        assert_eq!(json["session_key"], "agent:main:main");
        assert_eq!(json["streaming"], true);
    }

    #[test]
    fn test_abort_params_deserialization() {
        let json = json!({
            "run_id": "run-456"
        });

        let params: AbortParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.run_id, "run-456");
    }

    #[test]
    fn test_history_params_deserialization() {
        let json = json!({
            "session_key": "agent:main:main",
            "limit": 50,
            "before": "2024-01-01T00:00:00Z"
        });

        let params: HistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert_eq!(params.limit, Some(50));
        assert_eq!(params.before, Some("2024-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_history_params_minimal() {
        let json = json!({
            "session_key": "agent:main:main"
        });

        let params: HistoryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert!(params.limit.is_none());
        assert!(params.before.is_none());
    }

    #[test]
    fn test_clear_params_deserialization() {
        let json = json!({
            "session_key": "agent:main:main",
            "keep_system": false
        });

        let params: ClearParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert!(!params.keep_system);
    }

    #[test]
    fn test_clear_params_defaults() {
        let json = json!({
            "session_key": "agent:main:main"
        });

        let params: ClearParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_key, "agent:main:main");
        assert!(params.keep_system); // default true
    }

    #[test]
    fn test_chat_message_serialization() {
        let message = ChatMessage {
            role: "assistant".to_string(),
            content: "Hello!".to_string(),
            timestamp: "2024-01-01T12:00:00Z".to_string(),
            run_id: Some("run-789".to_string()),
        };

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["content"], "Hello!");
        assert_eq!(json["timestamp"], "2024-01-01T12:00:00Z");
        assert_eq!(json["run_id"], "run-789");
    }

    #[test]
    fn test_chat_message_without_run_id() {
        let message = ChatMessage {
            role: "user".to_string(),
            content: "Hi".to_string(),
            timestamp: "2024-01-01T12:00:00Z".to_string(),
            run_id: None,
        };

        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["role"], "user");
        assert!(!json.as_object().unwrap().contains_key("run_id"));
    }
}
