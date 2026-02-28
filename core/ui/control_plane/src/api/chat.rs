//! Chat API — wraps chat.send / chat.abort / chat.history / chat.clear RPC methods.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::context::DashboardState;

/// A single chat message (from history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,        // "user" | "assistant" | "system"
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

pub struct ChatApi;

impl ChatApi {
    /// Send a message and start an agent run.
    pub async fn send(
        state: &DashboardState,
        message: &str,
        session_key: Option<&str>,
    ) -> Result<ChatSendResponse, String> {
        let params = serde_json::json!({
            "message": message,
            "session_key": session_key,
            "channel": "gui:chat",
            "stream": true,
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
        let messages = result.get("messages").cloned().unwrap_or(Value::Array(vec![]));
        serde_json::from_value(messages).map_err(|e| e.to_string())
    }

    /// Clear chat history for a session.
    pub async fn clear(state: &DashboardState, session_key: &str) -> Result<(), String> {
        let params = serde_json::json!({ "session_key": session_key });
        state.rpc_call("chat.clear", params).await?;
        Ok(())
    }
}
