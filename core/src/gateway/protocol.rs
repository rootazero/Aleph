//! JSON-RPC 2.0 Protocol Implementation
//!
//! This module re-exports protocol types from `aleph-protocol` and provides
//! additional Gateway-specific functionality with backward-compatible APIs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// Re-export core protocol types from aleph-protocol
pub use aleph_protocol::jsonrpc::{
    JsonRpcError,
    // Error codes
    AUTH_REQUIRED, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
    PERMISSION_DENIED, RATE_LIMITED, RESOURCE_NOT_FOUND, TIMEOUT_ERROR,
    // Tool call types (from aleph-protocol)
    ToolCallContext as ProtoToolCallContext, ToolCallParams as ProtoToolCallParams,
    ToolCallResult as ProtoToolCallResult,
};

// Additional error codes specific to Gateway (not in aleph-protocol)
/// Authentication failed
pub const AUTH_FAILED: i32 = -32001;
/// Operation timeout (alias for TIMEOUT_ERROR)
pub const TIMEOUT: i32 = TIMEOUT_ERROR;

// ============================================================================
// JsonRpcRequest with backward-compatible API
// ============================================================================

/// JSON-RPC 2.0 Request
///
/// A JSON-RPC 2.0 request object. Notifications (requests without an `id`)
/// are used for one-way messages that don't expect a response.
/// This is a Gateway-specific version with backward-compatible API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version, must be "2.0"
    pub jsonrpc: String,
    /// Method name to invoke
    pub method: String,
    /// Optional parameters for the method
    #[serde(default)]
    pub params: Option<Value>,
    /// Request identifier (None for notifications)
    pub id: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC 2.0 request (backward-compatible 3-arg version)
    pub fn new(method: impl Into<String>, params: Option<Value>, id: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id,
        }
    }

    /// Create a request with specific ID
    pub fn with_id(method: impl Into<String>, params: Option<Value>, id: Value) -> Self {
        Self::new(method, params, Some(id))
    }

    /// Create a notification (request without id)
    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self::new(method, params, None)
    }

    /// Check if this is a notification (no response expected)
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// Validate the request format
    pub fn validate(&self) -> Result<(), JsonRpcError> {
        if self.jsonrpc != "2.0" {
            return Err(JsonRpcError::invalid_request("Invalid JSON-RPC version"));
        }
        if self.method.is_empty() {
            return Err(JsonRpcError::invalid_request("Method name is required"));
        }
        Ok(())
    }
}

/// JSON-RPC 2.0 Response
///
/// A JSON-RPC 2.0 response object containing either a result or an error.
/// This is a Gateway-specific version with backward-compatible API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version, always "2.0"
    pub jsonrpc: String,
    /// Success result (mutually exclusive with error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object (mutually exclusive with result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    /// Request identifier (same as request)
    pub id: Option<Value>,
}

impl JsonRpcResponse {
    /// Create a success response
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(result),
            error: None,
            id,
        }
    }

    /// Create an error response
    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
            id,
        }
    }

    /// Create an error response with additional data
    pub fn error_with_data(
        id: Option<Value>,
        code: i32,
        message: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
            id,
        }
    }

    /// Check if this is a successful response
    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }

    /// Check if this is an error response
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Create a new response with a different id
    ///
    /// Useful for setting the id on a pre-constructed error response.
    pub fn with_id(mut self, id: Option<Value>) -> Self {
        self.id = id;
        self
    }
}

/// JSON-RPC 2.0 Batch Request
///
/// A batch request is an array of request objects.
pub type JsonRpcBatchRequest = Vec<JsonRpcRequest>;

/// JSON-RPC 2.0 Batch Response
///
/// A batch response is an array of response objects.
pub type JsonRpcBatchResponse = Vec<JsonRpcResponse>;

// ============================================================================
// Gateway-specific extensions to protocol types
// ============================================================================

/// Extension trait for JsonRpcError with Gateway-specific methods
pub trait JsonRpcErrorExt {
    /// Create an authentication required error
    fn auth_required() -> JsonRpcError;
    /// Create an authentication failed error
    fn auth_failed(reason: impl Into<String>) -> JsonRpcError;
}

impl JsonRpcErrorExt for JsonRpcError {
    fn auth_required() -> JsonRpcError {
        JsonRpcError::new(AUTH_REQUIRED, "Authentication required")
    }

    fn auth_failed(reason: impl Into<String>) -> JsonRpcError {
        JsonRpcError::new(AUTH_FAILED, format!("Authentication failed: {}", reason.into()))
    }
}

// ============================================================================
// Tool Call Protocol Types (Server-to-Client reverse RPC)
// ============================================================================

/// Parameters for tool.call reverse RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    /// Tool name to execute
    pub tool: String,

    /// Tool arguments as JSON
    pub args: Value,

    /// Optional execution context
    #[serde(default)]
    pub context: Option<ToolCallContext>,
}

/// Execution context for tool.call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolCallContext {
    /// Request ID for correlation
    pub request_id: Option<String>,

    /// Session ID
    pub session_id: Option<String>,

    /// Timeout override in milliseconds
    pub timeout_ms: Option<u64>,
}

/// Result of tool.call execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Tool execution output
    pub output: Value,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,

    /// Whether execution succeeded
    pub success: bool,

    /// Error message if failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolCallResult {
    /// Create a successful result.
    pub fn success(output: Value, execution_time_ms: u64) -> Self {
        Self {
            output,
            execution_time_ms,
            success: true,
            error: None,
        }
    }

    /// Create a failed result.
    pub fn failure(error: String, execution_time_ms: u64) -> Self {
        Self {
            output: Value::Null,
            execution_time_ms,
            success: false,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_serialization() {
        let request = JsonRpcRequest::new("echo", Some(json!({"message": "hello"})), Some(json!(1)));
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"echo\""));
    }

    #[test]
    fn test_request_deserialization() {
        let json = r#"{"jsonrpc":"2.0","method":"health","id":1}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, "health");
        assert_eq!(request.id, Some(json!(1)));
    }

    #[test]
    fn test_notification() {
        let notification = JsonRpcRequest::notification("event", Some(json!({"type": "update"})));
        assert!(notification.is_notification());
    }

    #[test]
    fn test_success_response() {
        let response = JsonRpcResponse::success(Some(json!(1)), json!({"status": "ok"}));
        assert!(response.is_success());
        assert!(!response.is_error());
    }

    #[test]
    fn test_error_response() {
        let response = JsonRpcResponse::error(Some(json!(1)), METHOD_NOT_FOUND, "Method not found");
        assert!(response.is_error());
        assert!(!response.is_success());
    }

    #[test]
    fn test_validation() {
        let valid = JsonRpcRequest::with_id("test", None, json!(1));
        assert!(valid.validate().is_ok());

        let invalid_version = JsonRpcRequest {
            jsonrpc: "1.0".to_string(),
            method: "test".to_string(),
            params: None,
            id: Some(json!(1)),
        };
        assert!(invalid_version.validate().is_err());

        let empty_method = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: String::new(),
            params: None,
            id: Some(json!(1)),
        };
        assert!(empty_method.validate().is_err());
    }

    #[test]
    fn test_tool_call_params_serde() {
        let params = ToolCallParams {
            tool: "shell:exec".to_string(),
            args: json!({"command": "ls -la"}),
            context: Some(ToolCallContext {
                request_id: Some("req_123".to_string()),
                ..Default::default()
            }),
        };

        let json = serde_json::to_string(&params).unwrap();
        let parsed: ToolCallParams = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.tool, "shell:exec");
    }

    #[test]
    fn test_tool_call_result_success() {
        let result = ToolCallResult::success(json!({"files": ["a.txt", "b.txt"]}), 150);

        assert!(result.success);
        assert!(result.error.is_none());
        assert_eq!(result.execution_time_ms, 150);
    }

    #[test]
    fn test_tool_call_result_failure() {
        let result = ToolCallResult::failure("Permission denied".to_string(), 50);

        assert!(!result.success);
        assert_eq!(result.error, Some("Permission denied".to_string()));
    }
}
