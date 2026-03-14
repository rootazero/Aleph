//! ACP protocol message types (NDJSON-based JSON-RPC 2.0)

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global request ID counter for JSON-RPC requests.
static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

// =============================================================================
// AcpRequest
// =============================================================================

/// JSON-RPC 2.0 request sent to a CLI subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl AcpRequest {
    /// Create an `initialize` request.
    ///
    /// Sends `protocolVersion: 1` (number) as required by the ACP spec.
    pub fn initialize() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "aleph",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {},
            })),
        }
    }

    /// Create a `session/new` request.
    ///
    /// Requires `cwd` (working directory) and `mcpServers` (array, can be empty).
    pub fn new_session(cwd: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/new".to_string(),
            params: Some(serde_json::json!({
                "cwd": cwd,
                "mcpServers": [],
            })),
        }
    }

    /// Create a `session/prompt` request.
    ///
    /// The `prompt` field must be an array of content parts (e.g. `[{type: "text", text: "..."}]`).
    pub fn prompt(session_id: &str, text: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/prompt".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": text}],
            })),
        }
    }

    /// Create a `session/cancel` request.
    pub fn cancel(session_id: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/cancel".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
            })),
        }
    }
}

// =============================================================================
// AcpResponse
// =============================================================================

/// JSON-RPC 2.0 response or notification from a CLI subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    /// Present for responses, absent for notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AcpError>,
    /// Present for notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl AcpResponse {
    /// Returns true if this is a result response (has id and result/error).
    pub fn is_result(&self) -> bool {
        self.id.is_some()
    }

    /// Returns true if this is a notification (has method, no id).
    pub fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }

    /// Extract text content from the result, if present.
    ///
    /// Looks for `result.content` as a string, or `result.text`,
    /// or falls back to the stringified result value.
    pub fn text_content(&self) -> Option<String> {
        let result = self.result.as_ref()?;

        // Try result.content (string)
        if let Some(content) = result.get("content").and_then(|v| v.as_str()) {
            return Some(content.to_string());
        }

        // Try result.text
        if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }

        // Fall back to stringified result
        Some(result.to_string())
    }

    /// Extract text from a `session/update` notification's `agent_message_chunk`.
    ///
    /// Returns `Some(text)` if this is a streaming text chunk, `None` otherwise.
    pub fn streaming_text(&self) -> Option<String> {
        if self.method.as_deref() != Some("session/update") {
            return None;
        }
        let params = self.params.as_ref()?;
        let update = params.get("update")?;
        if update.get("sessionUpdate")?.as_str()? != "agent_message_chunk" {
            return None;
        }
        let content = update.get("content")?;
        if content.get("type")?.as_str()? == "text" {
            return content.get("text")?.as_str().map(String::from);
        }
        None
    }

    /// Check if this notification signals that the agent's turn is complete.
    pub fn is_turn_complete(&self) -> bool {
        if self.method.as_deref() != Some("session/update") {
            return false;
        }
        self.params
            .as_ref()
            .and_then(|p| p.get("update"))
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(|s| s.as_str())
            == Some("turn_complete")
    }
}

// =============================================================================
// AcpError
// =============================================================================

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ACP error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for AcpError {}

// =============================================================================
// AcpSessionState
// =============================================================================

/// State of an ACP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcpSessionState {
    Idle,
    Busy,
    Error,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_constructors() {
        let init = AcpRequest::initialize();
        assert_eq!(init.jsonrpc, "2.0");
        assert_eq!(init.method, "initialize");
        let params = init.params.unwrap();
        assert_eq!(params["protocolVersion"], 1);
        assert!(params["clientInfo"]["name"].as_str().is_some());

        let session = AcpRequest::new_session("/tmp");
        assert_eq!(session.method, "session/new");
        let p = session.params.unwrap();
        assert_eq!(p["cwd"], "/tmp");

        let prompt = AcpRequest::prompt("sess-1", "hello");
        assert_eq!(prompt.method, "session/prompt");
        let p = prompt.params.unwrap();
        assert_eq!(p["sessionId"], "sess-1");
        assert!(p["prompt"].is_array());
        assert_eq!(p["prompt"][0]["text"], "hello");

        let cancel = AcpRequest::cancel("sess-1");
        assert_eq!(cancel.method, "session/cancel");
    }

    #[test]
    fn test_request_ids_increment() {
        let r1 = AcpRequest::initialize();
        let r2 = AcpRequest::initialize();
        assert!(r2.id > r1.id);
    }

    #[test]
    fn test_response_is_result() {
        let resp = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(serde_json::json!({"content": "hi"})),
            error: None,
            method: None,
            params: None,
        };
        assert!(resp.is_result());
        assert!(!resp.is_notification());
    }

    #[test]
    fn test_response_is_notification() {
        let resp = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({"sessionId": "s1"})),
        };
        assert!(!resp.is_result());
        assert!(resp.is_notification());
    }

    #[test]
    fn test_text_content_extraction() {
        // From "content" field
        let resp = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(serde_json::json!({"content": "hello world"})),
            error: None,
            method: None,
            params: None,
        };
        assert_eq!(resp.text_content(), Some("hello world".to_string()));

        // From "text" field
        let resp2 = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(2),
            result: Some(serde_json::json!({"text": "from text"})),
            error: None,
            method: None,
            params: None,
        };
        assert_eq!(resp2.text_content(), Some("from text".to_string()));

        // No result
        let resp3 = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(3),
            result: None,
            error: None,
            method: None,
            params: None,
        };
        assert_eq!(resp3.text_content(), None);
    }

    #[test]
    fn test_streaming_text_extraction() {
        let notif = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": "hello"}
                }
            })),
        };
        assert_eq!(notif.streaming_text(), Some("hello".to_string()));

        // Non-text chunk
        let notif2 = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": []
                }
            })),
        };
        assert_eq!(notif2.streaming_text(), None);
    }

    #[test]
    fn test_is_turn_complete() {
        let notif = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({
                "sessionId": "s1",
                "update": {"sessionUpdate": "turn_complete"}
            })),
        };
        assert!(notif.is_turn_complete());
    }

    #[test]
    fn test_acp_error_display() {
        let err = AcpError {
            code: -32600,
            message: "Invalid Request".to_string(),
            data: None,
        };
        assert_eq!(err.to_string(), "ACP error -32600: Invalid Request");
    }

    #[test]
    fn test_session_state_serde() {
        let idle = AcpSessionState::Idle;
        let json = serde_json::to_string(&idle).unwrap();
        assert_eq!(json, "\"idle\"");

        let deserialized: AcpSessionState = serde_json::from_str("\"busy\"").unwrap();
        assert_eq!(deserialized, AcpSessionState::Busy);
    }

    #[test]
    fn test_roundtrip_serialization() {
        let req = AcpRequest::prompt("s1", "test");
        let json = serde_json::to_string(&req).unwrap();
        let parsed: AcpRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "session/prompt");
        assert_eq!(parsed.id, req.id);
    }
}
