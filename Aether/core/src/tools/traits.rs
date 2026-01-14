//! Tool Type Definitions
//!
//! Core data structures for tool metadata.
//!
//! **Note**: The `AgentTool` trait has been removed. Tools now use rig-core's
//! `Tool` trait for AI agent integration.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

// =============================================================================
// Tool Category
// =============================================================================

/// Tool category for UI grouping and filtering
///
/// Tools are classified into 5 categories based on their source:
/// - **Builtin**: Built-in rig-core tools (search, web_fetch, youtube)
/// - **Native**: Legacy native tools (deprecated)
/// - **Skills**: User-configured skills (instruction injection)
/// - **Mcp**: MCP server tools (dynamically loaded)
/// - **Custom**: User-defined custom tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    /// Built-in rig-core tools
    Builtin,
    /// Legacy native tools (deprecated)
    #[deprecated(note = "Use rig-core tools instead")]
    Native,
    /// User-configured skills (via UI settings)
    Skills,
    /// MCP server tools (via UI settings)
    Mcp,
    /// User-defined custom tools (via UI settings)
    Custom,
}

impl ToolCategory {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            ToolCategory::Builtin => "Builtin",
            #[allow(deprecated)]
            ToolCategory::Native => "Native",
            ToolCategory::Skills => "Skills",
            ToolCategory::Mcp => "MCP",
            ToolCategory::Custom => "Custom",
        }
    }

    /// Get SF Symbol icon name
    pub fn icon(&self) -> &'static str {
        match self {
            ToolCategory::Builtin => "command.square.fill",
            #[allow(deprecated)]
            ToolCategory::Native => "wrench.and.screwdriver.fill",
            ToolCategory::Skills => "sparkles",
            ToolCategory::Mcp => "server.rack",
            ToolCategory::Custom => "slider.horizontal.3",
        }
    }
}

impl fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// =============================================================================
// Tool Definition
// =============================================================================

/// Tool definition for LLM function calling
///
/// Contains all metadata needed for:
/// - LLM to understand and invoke the tool
/// - UI to display tool information
/// - Registry to route tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool name used in function calls (e.g., "search")
    pub name: String,

    /// Human-readable description for LLM
    pub description: String,

    /// JSON Schema for input parameters
    pub parameters: Value,

    /// Whether tool operation requires user confirmation
    pub requires_confirmation: bool,

    /// Tool category for UI grouping
    pub category: ToolCategory,
}

impl ToolDefinition {
    /// Create a new tool definition
    #[allow(deprecated)]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        category: ToolCategory,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            requires_confirmation: false,
            category,
        }
    }

    /// Set requires_confirmation flag
    pub fn with_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Create a definition with empty parameters
    #[allow(deprecated)]
    pub fn no_params(
        name: impl Into<String>,
        description: impl Into<String>,
        category: ToolCategory,
    ) -> Self {
        Self::new(
            name,
            description,
            serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            category,
        )
    }

    /// Convert to OpenAI function calling format
    pub fn to_openai_function(&self) -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters
            }
        })
    }

    /// Convert to Anthropic tool format
    pub fn to_anthropic_tool(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.parameters
        })
    }
}

// =============================================================================
// Tool Result (kept for compatibility)
// =============================================================================

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

impl From<crate::error::AetherError> for ToolResult {
    fn from(err: crate::error::AetherError) -> Self {
        ToolResult::error(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_category_display() {
        assert_eq!(ToolCategory::Builtin.display_name(), "Builtin");
        assert_eq!(ToolCategory::Skills.display_name(), "Skills");
        assert_eq!(ToolCategory::Mcp.display_name(), "MCP");
        assert_eq!(ToolCategory::Custom.display_name(), "Custom");
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("OK");
        assert!(result.is_success());
        assert_eq!(result.content, "OK");
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("Failed");
        assert!(!result.is_success());
        assert_eq!(result.error_message(), Some("Failed"));
    }
}
