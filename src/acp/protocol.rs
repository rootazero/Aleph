//! ACP protocol message types (NDJSON-based JSON-RPC 2.0)

use crate::sync_primitives::{AtomicU64, Ordering};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

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
    #[must_use]
    pub fn initialize() -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "aleph",
                    "version": env!("ALEPH_VERSION"),
                },
                // Advertise the client-side capabilities Aleph can service for
                // agent→client requests. `fs.*` and `session.requestPermission` are
                // backed by `crate::acp::incoming::IncomingHandler`; `terminal` is
                // not yet implemented so it stays absent (spec-compliant agents won't
                // send `terminal/*`, and a stray request gets a clean -32601).
                "capabilities": {
                    "fs": {
                        "readTextFile": true,
                        "writeTextFile": true,
                    },
                    "session": {
                        "requestPermission": true,
                    },
                },
            })),
        }
    }

    /// Create a `session/new` request.
    ///
    /// Requires `cwd` (working directory) and `mcpServers` (array, can be empty).
    #[must_use]
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
    #[must_use]
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
    #[must_use]
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

    /// Create a `session/load` request (best-effort session restore).
    #[must_use]
    pub fn load_session(session_id: &str, cwd: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/load".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "cwd": cwd,
                "mcpServers": [],
            })),
        }
    }

    /// Create a `session/set_mode` request — switches the agent's interaction
    /// mode (e.g. "chat" vs "code"). The available mode IDs are adapter-specific
    /// and advertised in the `session/new` response.
    #[must_use]
    pub fn set_mode(session_id: &str, mode_id: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/set_mode".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "modeId": mode_id,
            })),
        }
    }

    /// Create a `session/set_model` request — switches the underlying model.
    /// The available model IDs are adapter-specific and advertised in the
    /// `session/new` response under `availableModels`.
    #[must_use]
    pub fn set_model(session_id: &str, model_id: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/set_model".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "modelId": model_id,
            })),
        }
    }

    /// Create a `session/set_config_option` request — sets an adapter-specific
    /// runtime knob (e.g. `temperature`, `tools.allowFileWrite`).
    #[must_use]
    pub fn set_config_option(session_id: &str, key: &str, value: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "session/set_config_option".to_string(),
            params: Some(serde_json::json!({
                "sessionId": session_id,
                "key": key,
                "value": value,
            })),
        }
    }

    /// Create an `authenticate` request — performs adapter-side credential
    /// handshake. `method_id` selects which auth method the adapter advertised
    /// (e.g. `"api_key"`, `"oauth2"`); `credential` is the opaque token value.
    #[must_use]
    pub fn authenticate(method_id: &str, credential: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: next_id(),
            method: "authenticate".to_string(),
            params: Some(serde_json::json!({
                "methodId": method_id,
                "credential": credential,
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
    #[must_use]
    pub const fn is_result(&self) -> bool {
        self.id.is_some()
    }

    /// Returns true if this is a notification (has method, no id).
    #[must_use]
    pub const fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }

    /// Returns true if this is an **agent→client request** — a JSON-RPC frame
    /// that carries both a `method` and an `id`, meaning the agent expects us
    /// to send a response back (`fs/read_text_file`, `session/request_permission`,
    /// …). A frame is a *request* iff it has a `method`; the `id` namespace of
    /// inbound requests overlaps our own outbound request ids, so callers MUST
    /// test `method` presence before matching ids to avoid mistaking an agent
    /// request for our own response.
    #[must_use]
    pub const fn is_incoming_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }

    /// Borrow `(id, method, params)` when this frame is an agent→client
    /// request. Returns `None` otherwise.
    #[must_use]
    pub fn as_incoming_request(&self) -> Option<(u64, &str, &Value)> {
        const NULL: &Value = &Value::Null;
        match (self.id, self.method.as_deref()) {
            (Some(id), Some(method)) => Some((id, method, self.params.as_ref().unwrap_or(NULL))),
            _ => None,
        }
    }

    /// Extract text content from the result, if present.
    ///
    /// Looks for `result.content` as a string, or `result.text`,
    /// or falls back to the stringified result value.
    #[must_use]
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
// Structured ACP Errors
// =============================================================================

/// Classifies ACP operation failures for programmatic handling.
///
/// Mirrors acpx's `OutputErrorCode` surface so callers (LLM, panel, gateway)
/// can branch on a stable classification instead of substring-matching error
/// strings. See `as_str()` for the canonical wire token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpErrorCode {
    HarnessNotFound,
    HarnessDenied,
    SessionDead,
    Timeout,
    ProtocolError {
        code: i64,
    },
    ModeUnsupported,
    SpawnFailed,
    /// Adapter advertised it requires auth and the caller has no credential.
    AuthRequired,
    /// Session control RPC (`session/set_mode`, `session/set_model`,
    /// `session/set_config_option`) failed because the adapter doesn't
    /// implement it. Maps to acpx's session-control-errors classification.
    SessionControlUnsupported,
}

impl AcpErrorCode {
    /// Stable wire token used in error envelopes and tracing logs.
    ///
    /// These strings are part of the gateway contract — never rename without
    /// migrating panel + downstream consumers.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::HarnessNotFound => "harness_not_found",
            Self::HarnessDenied => "harness_denied",
            Self::SessionDead => "session_dead",
            Self::Timeout => "timeout",
            Self::ProtocolError { .. } => "protocol_error",
            Self::ModeUnsupported => "mode_unsupported",
            Self::SpawnFailed => "spawn_failed",
            Self::AuthRequired => "auth_required",
            Self::SessionControlUnsupported => "session_control_unsupported",
        }
    }

    /// Heuristic for callers deciding whether to retry. Mirrors acpx's
    /// `retryable` flag on `NormalizedOutputError`.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::SessionDead | Self::Timeout | Self::SpawnFailed)
    }
}

/// Structured ACP operation error with classification.
#[derive(Debug)]
pub struct AcpOperationError {
    pub code: AcpErrorCode,
    pub message: String,
    pub remote_error: Option<AcpError>,
}

impl fmt::Display for AcpOperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ACP {}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AcpOperationError {}

impl From<AcpOperationError> for crate::error::AlephError {
    fn from(e: AcpOperationError) -> Self {
        Self::AcpError {
            code: e.code.as_str().to_string(),
            message: e.message,
            retryable: e.code.is_retryable(),
        }
    }
}

impl AcpOperationError {
    pub fn new(code: AcpErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            remote_error: None,
        }
    }

    pub fn with_remote(code: AcpErrorCode, message: impl Into<String>, remote: AcpError) -> Self {
        Self {
            code,
            message: message.into(),
            remote_error: Some(remote),
        }
    }
}

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

    #[test]
    fn test_acp_error_code_copy() {
        let code = AcpErrorCode::Timeout;
        let code2 = code;
        assert_eq!(code, code2);

        let proto = AcpErrorCode::ProtocolError { code: -32600 };
        let proto2 = proto;
        assert_eq!(proto, proto2);
    }

    #[test]
    fn test_acp_operation_error_display() {
        let err = AcpOperationError::new(AcpErrorCode::Timeout, "timed out after 5m");
        // After acpx-parity: stable wire token in Display
        assert!(err.to_string().contains("timeout"));
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn test_acp_operation_error_into_aleph_error_preserves_code() {
        let err = AcpOperationError::new(AcpErrorCode::HarnessNotFound, "not found");
        let aleph_err: crate::error::AlephError = err.into();
        match aleph_err {
            crate::error::AlephError::AcpError {
                code,
                message,
                retryable,
            } => {
                assert_eq!(code, "harness_not_found");
                assert_eq!(message, "not found");
                assert!(!retryable, "HarnessNotFound is not retryable");
            }
            other => panic!("expected AlephError::AcpError, got {other:?}"),
        }
    }

    #[test]
    fn test_acp_error_code_retryable() {
        assert!(AcpErrorCode::SessionDead.is_retryable());
        assert!(AcpErrorCode::Timeout.is_retryable());
        assert!(AcpErrorCode::SpawnFailed.is_retryable());
        assert!(!AcpErrorCode::HarnessNotFound.is_retryable());
        assert!(!AcpErrorCode::HarnessDenied.is_retryable());
        assert!(!AcpErrorCode::AuthRequired.is_retryable());
        assert!(!AcpErrorCode::SessionControlUnsupported.is_retryable());
    }

    #[test]
    fn test_acp_error_code_as_str_stable_tokens() {
        // These are part of the gateway contract — must never silently
        // rename without migrating panel + downstream consumers.
        assert_eq!(AcpErrorCode::HarnessNotFound.as_str(), "harness_not_found");
        assert_eq!(AcpErrorCode::Timeout.as_str(), "timeout");
        assert_eq!(AcpErrorCode::SessionDead.as_str(), "session_dead");
        assert_eq!(AcpErrorCode::AuthRequired.as_str(), "auth_required");
        assert_eq!(
            AcpErrorCode::SessionControlUnsupported.as_str(),
            "session_control_unsupported"
        );
        assert_eq!(
            AcpErrorCode::ProtocolError { code: -32601 }.as_str(),
            "protocol_error"
        );
    }

    #[test]
    fn test_set_mode_request() {
        let req = AcpRequest::set_mode("sess-1", "code");
        assert_eq!(req.method, "session/set_mode");
        let p = req.params.unwrap();
        assert_eq!(p["sessionId"], "sess-1");
        assert_eq!(p["modeId"], "code");
    }

    #[test]
    fn test_set_model_request() {
        let req = AcpRequest::set_model("sess-1", "claude-opus-4-7");
        assert_eq!(req.method, "session/set_model");
        let p = req.params.unwrap();
        assert_eq!(p["sessionId"], "sess-1");
        assert_eq!(p["modelId"], "claude-opus-4-7");
    }

    #[test]
    fn test_set_config_option_request() {
        let req = AcpRequest::set_config_option("sess-1", "temperature", serde_json::json!(0.7));
        assert_eq!(req.method, "session/set_config_option");
        let p = req.params.unwrap();
        assert_eq!(p["sessionId"], "sess-1");
        assert_eq!(p["key"], "temperature");
        assert_eq!(p["value"], serde_json::json!(0.7));
    }

    #[test]
    fn test_authenticate_request() {
        let req = AcpRequest::authenticate("api_key", "secret-token");
        assert_eq!(req.method, "authenticate");
        let p = req.params.unwrap();
        assert_eq!(p["methodId"], "api_key");
        assert_eq!(p["credential"], "secret-token");
    }

    #[test]
    fn test_load_session_request() {
        let req = AcpRequest::load_session("sess-42", "/tmp");
        assert_eq!(req.method, "session/load");
        let p = req.params.unwrap();
        assert_eq!(p["sessionId"], "sess-42");
        assert_eq!(p["cwd"], "/tmp");
    }

    #[test]
    fn test_incoming_request_classification() {
        // Agent→client request: has both id and method.
        let req = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(42),
            result: None,
            error: None,
            method: Some("fs/read_text_file".to_string()),
            params: Some(serde_json::json!({ "path": "/tmp/x" })),
        };
        assert!(req.is_incoming_request());
        assert!(!req.is_notification());
        let (id, method, params) = req.as_incoming_request().unwrap();
        assert_eq!(id, 42);
        assert_eq!(method, "fs/read_text_file");
        assert_eq!(params["path"], "/tmp/x");
    }

    #[test]
    fn test_response_is_not_incoming_request() {
        // Our own response: id present, NO method — must not be misclassified.
        let resp = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(serde_json::json!({"content": "hi"})),
            error: None,
            method: None,
            params: None,
        };
        assert!(!resp.is_incoming_request());
        assert!(resp.as_incoming_request().is_none());
    }

    #[test]
    fn test_notification_is_not_incoming_request() {
        // Notification: method present, NO id.
        let notif = AcpResponse {
            jsonrpc: "2.0".to_string(),
            id: None,
            result: None,
            error: None,
            method: Some("session/update".to_string()),
            params: Some(serde_json::json!({})),
        };
        assert!(!notif.is_incoming_request());
        assert!(notif.is_notification());
    }

    #[test]
    fn test_initialize_advertises_fs_capabilities() {
        let init = AcpRequest::initialize();
        let caps = &init.params.unwrap()["capabilities"];
        assert_eq!(caps["fs"]["readTextFile"], true);
        assert_eq!(caps["fs"]["writeTextFile"], true);
        assert_eq!(caps["session"]["requestPermission"], true);
    }
}
