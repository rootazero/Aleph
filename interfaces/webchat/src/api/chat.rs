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
    /// Last-turn context-window occupancy persisted on assistant turns, so the
    /// gauge re-projects when a session is reloaded from history.
    #[serde(default)]
    pub context_tokens: Option<u32>,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

/// Response from chat.send
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSendResponse {
    pub run_id: String,
    pub session_key: String,
    pub streaming: bool,
}

/// Response from chat.context_estimate — a pre-run occupancy estimate for a
/// session that never ran an LLM turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEstimateResponse {
    pub used_tokens: u32,
    pub window_tokens: u32,
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
    /// Extracted from the current `session_key` when available.
    ///
    /// `project_root` — absolute path of the active project folder when
    /// the user has entered project mode via "enter project workspace". Forwarded as
    /// `RunRequest.workspace_override` so the agent's tool calls run
    /// inside that folder instead of `~/.aleph/workspaces/{agent_id}`.
    ///
    /// `voice_input` — true when `message` is an ASR-transcribed spoken
    /// utterance (voice loop / dictation). Core then arms the session's
    /// voice-mode prompt layer and the `[voice]` low-TTFT model pin.
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        state: &DashboardState,
        message: &str,
        session_key: Option<&str>,
        attachments: Vec<ChatAttachment>,
        agent_id: Option<&str>,
        project_root: Option<&str>,
        model_override: Option<&crate::api::providers::ModelOverride>,
        exec_tier: Option<&str>,
        mode: Option<&str>,
        voice_input: bool,
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
            "project_root": project_root,
            "model_override": model_override,
            // The tier rides on the message because a brand-new conversation
            // has no session to write it to yet — and the first turn is exactly
            // the one the picker was armed for. The server stamps it onto the
            // session, so later turns need not resend it.
            "exec_tier": exec_tier,
            // Same first-message carriage for the usage mode (mode pill).
            "mode": mode,
            "voice_input": voice_input,
        });
        let result = state.rpc_call("chat.send", params).await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }

    /// Abort a running agent, and abandon whatever that session still has
    /// waiting in the gateway's busy lane.
    ///
    /// `session_key` is what makes Stop mean "I do not want this work" rather
    /// than "stop this one run": without it the cancel frees the session slot
    /// and the server-side backlog starts firing a fresh agent run per queued
    /// message. Pass `None` only when there is no session to scope to (a run
    /// aborted before its first `chat.send` returned).
    pub async fn abort(
        state: &DashboardState,
        run_id: &str,
        session_key: Option<&str>,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "run_id": run_id, "session_key": session_key });
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

    /// Estimate a session's next-prompt occupancy (sessions with no real
    /// occupancy recorded). `Ok(None)` when core returns null (unresolvable
    /// session/model) → caller keeps the gauge hidden.
    pub async fn context_estimate(
        state: &DashboardState,
        session_key: &str,
    ) -> Result<Option<ContextEstimateResponse>, String> {
        let params = serde_json::json!({ "session_key": session_key });
        let result = state.rpc_call("chat.context_estimate", params).await?;
        if result.is_null() {
            return Ok(None);
        }
        serde_json::from_value(result)
            .map(Some)
            .map_err(|e| e.to_string())
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
            .map(std::string::ToString::to_string)
            .ok_or_else(|| "Missing new_session_key in response".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_response_round_trips() {
        let v = serde_json::json!({ "used_tokens": 12_000, "window_tokens": 200_000 });
        let r: ContextEstimateResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.used_tokens, 12_000);
        assert_eq!(r.window_tokens, 200_000);
    }
}
