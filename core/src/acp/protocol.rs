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
    pub fn initialize() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "initialize".to_string(),
            params: None,
        }
    }

    /// Create a `session/new` request.
    pub fn new_session() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/new".to_string(),
            params: None,
        }
    }

    /// Create a `prompt` request with session id, text, and working directory.
    pub fn prompt(session_id: &str, text: &str, cwd: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "prompt".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "text": text,
                "cwd": cwd,
            })),
        }
    }

    /// Create a `cancel` request.
    pub fn cancel() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "cancel".to_string(),
            params: None,
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
        assert!(init.params.is_none());

        let session = AcpRequest::new_session();
        assert_eq!(session.method, "session/new");

        let prompt = AcpRequest::prompt("sess-1", "hello", "/tmp");
        assert_eq!(prompt.method, "prompt");
        let params = prompt.params.unwrap();
        assert_eq!(params["sessionId"], "sess-1");
        assert_eq!(params["text"], "hello");
        assert_eq!(params["cwd"], "/tmp");

        let cancel = AcpRequest::cancel();
        assert_eq!(cancel.method, "cancel");
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
            method: Some("progress".to_string()),
            params: Some(serde_json::json!({"percent": 50})),
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
        let req = AcpRequest::prompt("s1", "test", "/home");
        let json = serde_json::to_string(&req).unwrap();
        let parsed: AcpRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "prompt");
        assert_eq!(parsed.id, req.id);
    }
}
