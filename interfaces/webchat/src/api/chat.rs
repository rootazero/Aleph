//! Chat API — wraps chat.send / chat.abort / chat.history / chat.clear RPC methods.

use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single chat message (from history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" | "assistant" | "system"
    pub content: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// Response from chat.send
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSendResponse {
    pub run_id: String,
    pub session_key: String,
    pub streaming: bool,
}

/// A file attachment to send with a chat message.
#[derive(Debug, Clone)]
pub struct ChatAttachment {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
    pub size: u64,
}

pub struct ChatApi;

impl ChatApi {
    /// Send a message and start an agent run.
    ///
    /// `agent_id` — explicit target agent (bypasses channel binding resolution).
    /// Extracted from the current session_key when available.
    pub async fn send(
        state: &DashboardState,
        message: &str,
        session_key: Option<&str>,
        attachments: Vec<ChatAttachment>,
        agent_id: Option<&str>,
    ) -> Result<ChatSendResponse, String> {
        let attachments_json: Vec<Value> = attachments
            .iter()
            .map(|a| {
                serde_json::json!({
                    "name": a.name,
                    "mime_type": a.mime_type,
                    "data": a.data_base64,
                })
            })
            .collect();

        let params = serde_json::json!({
            "message": message,
            "session_key": session_key,
            "channel": "gui:chat",
            "stream": true,
            "attachments": attachments_json,
            "agent_id": agent_id,
        });
        let result = state.rpc_call("chat.send", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Abort a running agent.
    pub async fn abort(state: &DashboardState, run_id: &str) -> Result<(), String> {
        let params = serde_json::json!({ "run_id": run_id });
        state.rpc_call("chat.abort", params).await?;
        Ok(())
    }

    /// Get chat history for a session.
    pub async fn history(
        state: &DashboardState,
        session_key: &str,
        limit: Option<usize>,
    ) -> Result<Vec<ChatMessage>, String> {
        let params = serde_json::json!({
            "session_key": session_key,
            "limit": limit,
        });
        let result = state.rpc_call("chat.history", params).await?;
        let messages = result
            .get("messages")
            .cloned()
            .unwrap_or(Value::Array(vec![]));
        serde_json::from_value(messages).map_err(|e| e.to_string())
    }

    /// Clear chat history for a session.
    pub async fn clear(state: &DashboardState, session_key: &str) -> Result<(), String> {
        let params = serde_json::json!({ "session_key": session_key });
        state.rpc_call("chat.clear", params).await?;
        Ok(())
    }

    /// Create a new session by closing the current one and incrementing the epoch.
    /// Returns the new session key.
    pub async fn new_session(
        state: &DashboardState,
        current_session_key: &str,
        topic: Option<&str>,
    ) -> Result<String, String> {
        let params = serde_json::json!({
            "session_key": current_session_key,
            "topic": topic,
        });
        let result = state.rpc_call("sessions.new", params).await?;
        result
            .get("new_session_key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Missing new_session_key in response".to_string())
    }
}
