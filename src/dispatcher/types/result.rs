//! Tool Result
//!
//! Standardized result format for tool executions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool execution result
///
/// Standardized result format for tool executions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the operation succeeded
    pub success: bool,

    /// Human-readable result content
    pub content: String,

    /// Optional structured data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,

    /// Error message if operation failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolResult {
    /// Create a successful result with content
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            success: true,
            content: content.into(),
            data: None,
            error: None,
        }
    }

    /// Create a successful result with content and structured data
    pub fn success_with_data(content: impl Into<String>, data: Value) -> Self {
        Self {
            success: true,
            content: content.into(),
            data: Some(data),
            error: None,
        }
    }

    /// Create a failed result with error message
    pub fn error(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            success: false,
            content: String::new(),
            data: None,
            error: Some(msg),
        }
    }

    /// Check if result is successful
    pub fn is_success(&self) -> bool {
        self.success
    }

    /// Get error message if failed
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Convert to JSON
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(serde_json::json!({
            "success": false,
            "error": "Failed to serialize result"
        }))
    }
}

impl From<crate::error::AlephError> for ToolResult {
    fn from(err: crate::error::AlephError) -> Self {
        ToolResult::error(err.to_string())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("Operation completed");
        assert!(result.is_success());
        assert_eq!(result.content, "Operation completed");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_success_with_data() {
        let data = serde_json::json!({"count": 42});
        let result = ToolResult::success_with_data("Found items", data.clone());
        assert!(result.is_success());
        assert_eq!(result.data, Some(data));
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("Something went wrong");
        assert!(!result.is_success());
        assert_eq!(result.error_message(), Some("Something went wrong"));
    }

    #[test]
    fn test_tool_result_to_json() {
        let result = ToolResult::success("OK");
        let json = result.to_json();
        assert_eq!(json["success"], true);
        assert_eq!(json["content"], "OK");
    }
}
