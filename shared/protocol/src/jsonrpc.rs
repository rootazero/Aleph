//! JSON-RPC 2.0 Protocol Types
//!
//! Standard JSON-RPC 2.0 request/response types for Aleph Gateway communication.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Error Codes (JSON-RPC 2.0 Standard + Aleph Extensions)
// ============================================================================

/// Parse error - Invalid JSON was received
pub const PARSE_ERROR: i32 = -32700;
/// Invalid Request - The JSON sent is not a valid Request object
pub const INVALID_REQUEST: i32 = -32600;
/// Method not found - The method does not exist / is not available
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Invalid params - Invalid method parameter(s)
pub const INVALID_PARAMS: i32 = -32602;
/// Internal error - Internal JSON-RPC error
pub const INTERNAL_ERROR: i32 = -32603;

// Server error range: -32000 to -32099 (reserved for implementation-defined server-errors)

/// Authentication required
pub const AUTH_REQUIRED: i32 = -32000;
/// Session not found
pub const SESSION_NOT_FOUND: i32 = -32001;
/// Rate limit exceeded
pub const RATE_LIMITED: i32 = -32002;
/// Agent execution error
pub const AGENT_ERROR: i32 = -32003;
/// Tool execution error
pub const TOOL_ERROR: i32 = -32004;
/// Provider (LLM) error
pub const PROVIDER_ERROR: i32 = -32005;
/// Memory system error
pub const MEMORY_ERROR: i32 = -32006;
/// Configuration error
pub const CONFIG_ERROR: i32 = -32007;
/// Permission denied
pub const PERMISSION_DENIED: i32 = -32008;
/// Resource not found
pub const RESOURCE_NOT_FOUND: i32 = -32009;
/// Operation timeout
pub const TIMEOUT_ERROR: i32 = -32010;

// ============================================================================
// JSON-RPC 2.0 Types
// ============================================================================

/// JSON-RPC 2.0 Request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version, always "2.0"
    pub jsonrpc: String,
    /// Method name
    pub method: String,
    /// Optional parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// Request ID (null for notifications)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new request with auto-generated ID
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id: Some(Value::String(uuid_v4())),
        }
    }

    /// Create a notification (no response expected)
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id: None,
        }
    }

    /// Create a request with specific ID
    pub fn with_id(method: impl Into<String>, params: Option<Value>, id: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id: Some(id),
        }
    }

    /// Check if this is a notification (no ID)
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// JSON-RPC 2.0 Response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version, always "2.0"
    pub jsonrpc: String,
    /// Result (mutually exclusive with error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (mutually exclusive with result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    /// Request ID this response corresponds to
    pub id: Value,
}

impl JsonRpcResponse {
    /// Create a success response
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Create an error response
    pub fn error(id: Value, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(error),
            id,
        }
    }

    /// Check if this response is an error
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

/// JSON-RPC 2.0 Error Object
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
    /// Optional additional data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// Create a new error
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Create an error with additional data
    pub fn with_data(code: i32, message: impl Into<String>, data: Value) -> Self {
        Self {
            code,
            message: message.into(),
            data: Some(data),
        }
    }

    /// Create a parse error
    pub fn parse_error(details: impl Into<String>) -> Self {
        Self::new(PARSE_ERROR, format!("Parse error: {}", details.into()))
    }

    /// Create an invalid request error
    pub fn invalid_request(details: impl Into<String>) -> Self {
        Self::new(
            INVALID_REQUEST,
            format!("Invalid request: {}", details.into()),
        )
    }

    /// Create a method not found error
    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("Method not found: {}", method))
    }

    /// Create an invalid params error
    pub fn invalid_params(details: impl Into<String>) -> Self {
        Self::new(
            INVALID_PARAMS,
            format!("Invalid params: {}", details.into()),
        )
    }

    /// Create an internal error
    pub fn internal_error(details: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, format!("Internal error: {}", details.into()))
    }
}

// ============================================================================
// Reverse RPC Types (Server → Client tool calls)
// ============================================================================

/// Parameters for Server → Client tool call request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallParams {
    /// Unique tool call ID for correlation
    pub call_id: String,
    /// Tool name to execute
    pub tool_name: String,
    /// Tool parameters
    pub params: Value,
    /// Execution context
    #[serde(default)]
    pub context: ToolCallContext,
}

/// Context for tool execution
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolCallContext {
    /// Current run ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Session key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// Timeout in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Result of Client → Server tool call response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Tool call ID (must match request)
    pub call_id: String,
    /// Whether execution succeeded
    pub success: bool,
    /// Result value (if success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Execution duration in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ToolCallResult {
    /// Create a success result
    pub fn success(call_id: impl Into<String>, result: Value) -> Self {
        Self {
            call_id: call_id.into(),
            success: true,
            result: Some(result),
            error: None,
            duration_ms: None,
        }
    }

    /// Create an error result
    pub fn error(call_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            success: false,
            result: None,
            error: Some(error.into()),
            duration_ms: None,
        }
    }

    /// Add duration to result
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Generate a simple UUID v4 (without external dependency)
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.as_nanos();
    let random = (nanos ^ (nanos >> 64)) as u64;
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (random >> 32) as u32,
        ((random >> 16) as u16),
        random as u16 & 0x0FFF,
        ((random >> 48) as u16 & 0x3FFF) | 0x8000,
        random & 0xFFFFFFFFFFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_creation() {
        let req = JsonRpcRequest::new("test.method", Some(serde_json::json!({"key": "value"})));
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "test.method");
        assert!(req.id.is_some());
        assert!(!req.is_notification());
    }

    #[test]
    fn test_notification_creation() {
        let notif = JsonRpcRequest::notification("stream.event", None);
        assert!(notif.is_notification());
        assert!(notif.id.is_none());
    }

    #[test]
    fn test_response_success() {
        let resp = JsonRpcResponse::success(Value::Number(1.into()), serde_json::json!({"ok": true}));
        assert!(!resp.is_error());
        assert!(resp.result.is_some());
    }

    #[test]
    fn test_response_error() {
        let resp = JsonRpcResponse::error(
            Value::Number(1.into()),
            JsonRpcError::method_not_found("unknown"),
        );
        assert!(resp.is_error());
        assert!(resp.result.is_none());
    }

    #[test]
    fn test_tool_call_result() {
        let result = ToolCallResult::success("call-1", serde_json::json!({"output": "done"}))
            .with_duration(150);
        assert!(result.success);
        assert_eq!(result.duration_ms, Some(150));
    }

    #[test]
    fn test_serde_roundtrip() {
        let req = JsonRpcRequest::new("test", Some(serde_json::json!({})));
        let json = serde_json::to_string(&req).unwrap();
        let parsed: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "test");
    }
}
