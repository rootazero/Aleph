//! Session usage statistics RPC handler
//!
//! Returns token counts and message statistics for a session.

use serde::Deserialize;
use serde_json::json;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::sync_primitives::Arc;

use super::session::SessionStore;

#[derive(Debug, Deserialize)]
struct UsageParams {
    session_key: String,
}

/// Handle session.usage -- return token/message stats for a session
pub async fn handle(request: JsonRpcRequest, store: Arc<SessionStore>) -> JsonRpcResponse {
    let params: UsageParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let sessions = store.list(None).await;
    let session_info = sessions.iter().find(|s| s.key == params.session_key);

    match session_info {
        Some(info) => {
            let history = store.get_history(&params.session_key, None).await;
            let (input_tokens, output_tokens) = match &history {
                Some(messages) => estimate_tokens(messages),
                None => (0u64, 0u64),
            };

            let total = input_tokens + output_tokens;
            let message_count = history.as_ref().map(|h| h.len()).unwrap_or(0);

            JsonRpcResponse::success(
                request.id,
                json!({
                    "session_key": params.session_key,
                    "tokens": total,
                    "input_tokens": input_tokens,
                    "output_tokens": output_tokens,
                    "messages": message_count,
                    "created_at": info.created_at,
                    "last_active_at": info.last_active_at,
                }),
            )
        }
        None => JsonRpcResponse::error(
            request.id,
            -32001,
            format!("Session '{}' not found", params.session_key),
        ),
    }
}

/// Estimate token counts from message history.
/// Uses rough approximation: ~3 bytes per token (byte-based, not char-based).
fn estimate_tokens(messages: &[super::session::HistoryMessage]) -> (u64, u64) {
    let mut input = 0u64;
    let mut output = 0u64;
    for msg in messages {
        let tokens = (msg.content.len() as u64) / 3;
        match msg.role.as_str() {
            "user" => input += tokens,
            "assistant" => output += tokens,
            _ => input += tokens,
        }
    }
    (input, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::session::HistoryMessage;

    #[test]
    fn estimate_tokens_basic() {
        let messages = vec![
            HistoryMessage {
                role: "user".to_string(),
                content: "Hello, how are you?".to_string(),
                timestamp: "2026-01-01T00:00:00Z".to_string(),
                metadata: None,
            },
            HistoryMessage {
                role: "assistant".to_string(),
                content: "I am doing well, thank you for asking!".to_string(),
                timestamp: "2026-01-01T00:00:01Z".to_string(),
                metadata: None,
            },
        ];
        let (input, output) = estimate_tokens(&messages);
        assert!(input > 0);
        assert!(output > input);
    }

    #[test]
    fn estimate_tokens_empty() {
        let messages: Vec<HistoryMessage> = vec![];
        let (input, output) = estimate_tokens(&messages);
        assert_eq!(input, 0);
        assert_eq!(output, 0);
    }
}
