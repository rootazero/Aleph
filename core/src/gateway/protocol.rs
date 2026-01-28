//! JSON-RPC 2.0 Protocol Implementation
//!
//! This module defines the core types for JSON-RPC 2.0 communication
//! over WebSocket connections.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 Request
///
/// A JSON-RPC 2.0 request object. Notifications (requests without an `id`)
/// are used for one-way messages that don't expect a response.
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
    /// Create a new JSON-RPC 2.0 request
    pub fn new(method: impl Into<String>, params: Option<Value>, id: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
            id,
        }
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
}

/// JSON-RPC 2.0 Error Object
///
/// Error codes in the range -32000 to -32099 are reserved for
/// implementation-defined server errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code
    pub code: i32,
    /// Human-readable error message
    pub message: String,
    /// Optional additional error data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// Standard JSON-RPC 2.0 error codes
/// Invalid JSON was received by the server
pub const PARSE_ERROR: i32 = -32700;
/// The JSON sent is not a valid Request object
pub const INVALID_REQUEST: i32 = -32600;
/// The method does not exist / is not available
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Invalid method parameter(s)
pub const INVALID_PARAMS: i32 = -32602;
/// Internal JSON-RPC error
pub const INTERNAL_ERROR: i32 = -32603;

// Custom error codes (reserved range: -32000 to -32099)
/// Authentication required
pub const AUTH_REQUIRED: i32 = -32000;
/// Authentication failed
pub const AUTH_FAILED: i32 = -32001;
/// Permission denied
pub const PERMISSION_DENIED: i32 = -32002;
/// Rate limit exceeded
pub const RATE_LIMITED: i32 = -32003;
/// Resource not found
pub const RESOURCE_NOT_FOUND: i32 = -32004;
/// Operation timeout
pub const TIMEOUT: i32 = -32005;

impl JsonRpcError {
    /// Create a new error with code and message
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Create a new error with additional data
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
        Self::new(INVALID_REQUEST, format!("Invalid request: {}", details.into()))
    }

    /// Create a method not found error
    pub fn method_not_found(method: &str) -> Self {
        Self::new(METHOD_NOT_FOUND, format!("Method not found: {}", method))
    }

    /// Create an invalid params error
    pub fn invalid_params(details: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, format!("Invalid params: {}", details.into()))
    }

    /// Create an internal error
    pub fn internal_error(details: impl Into<String>) -> Self {
        Self::new(INTERNAL_ERROR, format!("Internal error: {}", details.into()))
    }

    /// Create an authentication required error
    pub fn auth_required() -> Self {
        Self::new(AUTH_REQUIRED, "Authentication required")
    }

    /// Create an authentication failed error
    pub fn auth_failed(reason: impl Into<String>) -> Self {
        Self::new(AUTH_FAILED, format!("Authentication failed: {}", reason.into()))
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
        assert!(json.contains("\"id\":1"));
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
        let valid = JsonRpcRequest::new("test", None, Some(json!(1)));
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
}
