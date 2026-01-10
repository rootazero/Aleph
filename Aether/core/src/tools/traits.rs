//! AgentTool Trait and Core Types
//!
//! Unified interface for LLM function calling tools. All native tools
//! implement this trait for direct invocation without MCP wrapper overhead.
//!
//! # Design Philosophy
//!
//! - **Direct Invocation**: No string-based dispatch, call `tool.execute()` directly
//! - **Type Safety**: Each tool deserializes its own typed parameters
//! - **JSON Schema**: Tool definitions include parameter schemas for LLM
//! - **Unified Interface**: Same trait for native and MCP-bridged tools

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

// =============================================================================
// Tool Category
// =============================================================================

/// Tool category for UI grouping and filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    /// File and directory operations
    Filesystem,
    /// Version control operations
    Git,
    /// Command execution
    Shell,
    /// System information
    System,
    /// Clipboard operations
    Clipboard,
    /// Screen capture
    Screen,
    /// Web search
    Search,
    /// External MCP server tools
    External,
    /// Miscellaneous tools
    Other,
}

impl ToolCategory {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            ToolCategory::Filesystem => "Filesystem",
            ToolCategory::Git => "Git",
            ToolCategory::Shell => "Shell",
            ToolCategory::System => "System",
            ToolCategory::Clipboard => "Clipboard",
            ToolCategory::Screen => "Screen",
            ToolCategory::Search => "Search",
            ToolCategory::External => "External",
            ToolCategory::Other => "Other",
        }
    }

    /// Get SF Symbol icon name
    pub fn icon(&self) -> &'static str {
        match self {
            ToolCategory::Filesystem => "folder.fill",
            ToolCategory::Git => "arrow.triangle.branch",
            ToolCategory::Shell => "terminal.fill",
            ToolCategory::System => "gearshape.fill",
            ToolCategory::Clipboard => "doc.on.clipboard",
            ToolCategory::Screen => "camera.viewfinder",
            ToolCategory::Search => "magnifyingglass",
            ToolCategory::External => "puzzlepiece.extension.fill",
            ToolCategory::Other => "wrench.fill",
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
    /// Unique tool name used in function calls (e.g., "file_read")
    pub name: String,

    /// Human-readable description for LLM
    pub description: String,

    /// JSON Schema for input parameters
    ///
    /// Must be a valid JSON Schema Draft-07 object with:
    /// - `type: "object"` at root
    /// - `properties` defining each parameter
    /// - `required` listing mandatory parameters
    pub parameters: Value,

    /// Whether tool operation is destructive and requires user confirmation
    pub requires_confirmation: bool,

    /// Tool category for UI grouping
    pub category: ToolCategory,
}

impl ToolDefinition {
    /// Create a new tool definition
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
// Tool Result
// =============================================================================

/// Tool execution result
///
/// Standardized result format for all tool executions.
/// Designed for both LLM consumption and UI display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Whether the operation succeeded
    pub success: bool,

    /// Human-readable result content for LLM
    pub content: String,

    /// Optional structured data (e.g., file listing as JSON array)
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

    /// Create a failed result with both content and error
    pub fn partial_error(content: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            success: false,
            content: content.into(),
            data: None,
            error: Some(error.into()),
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

    /// Convert to JSON for LLM consumption
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

// =============================================================================
// AgentTool Trait
// =============================================================================

/// Unified tool interface for LLM function calling
///
/// All tools (native and MCP-bridged) implement this trait.
/// This provides a consistent interface for:
/// - Tool discovery and definition
/// - Tool execution
/// - Confirmation requirements
///
/// # Example
///
/// ```rust,ignore
/// pub struct FileReadTool {
///     allowed_roots: Vec<PathBuf>,
/// }
///
/// #[async_trait]
/// impl AgentTool for FileReadTool {
///     fn name(&self) -> &str {
///         "file_read"
///     }
///
///     fn definition(&self) -> ToolDefinition {
///         ToolDefinition::new(
///             "file_read",
///             "Read file contents from the filesystem",
///             json!({
///                 "type": "object",
///                 "properties": {
///                     "path": { "type": "string", "description": "File path" }
///                 },
///                 "required": ["path"]
///             }),
///             ToolCategory::Filesystem,
///         )
///     }
///
///     async fn execute(&self, args: &str) -> Result<ToolResult> {
///         let params: FileReadParams = serde_json::from_str(args)?;
///         let content = std::fs::read_to_string(&params.path)?;
///         Ok(ToolResult::success(content))
///     }
/// }
/// ```
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Get the unique tool name
    ///
    /// This name is used for:
    /// - Function call identification
    /// - Registry lookup
    /// - Logging and debugging
    fn name(&self) -> &str;

    /// Get the tool definition for LLM
    ///
    /// Returns complete metadata including:
    /// - Name and description
    /// - JSON Schema for parameters
    /// - Confirmation requirements
    /// - Category for UI grouping
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with JSON arguments
    ///
    /// # Arguments
    ///
    /// * `args` - JSON string containing tool parameters
    ///
    /// # Returns
    ///
    /// * `Ok(ToolResult)` - Execution result (success or failure)
    /// * `Err(AetherError)` - Execution error (e.g., deserialization failure)
    ///
    /// # Implementation Notes
    ///
    /// Implementations should:
    /// 1. Deserialize `args` to typed parameters
    /// 2. Validate parameters (e.g., path security)
    /// 3. Perform the operation
    /// 4. Return appropriate ToolResult
    async fn execute(&self, args: &str) -> crate::error::Result<ToolResult>;

    /// Whether this tool requires user confirmation before execution
    ///
    /// Default implementation returns the value from `definition()`.
    /// Override if confirmation requirement is dynamic.
    fn requires_confirmation(&self) -> bool {
        self.definition().requires_confirmation
    }

    /// Get the tool category
    ///
    /// Default implementation returns the value from `definition()`.
    fn category(&self) -> ToolCategory {
        self.definition().category
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_category_display() {
        assert_eq!(ToolCategory::Filesystem.display_name(), "Filesystem");
        assert_eq!(ToolCategory::Git.display_name(), "Git");
        assert_eq!(ToolCategory::Shell.display_name(), "Shell");
    }

    #[test]
    fn test_tool_category_icon() {
        assert_eq!(ToolCategory::Filesystem.icon(), "folder.fill");
        assert_eq!(ToolCategory::Git.icon(), "arrow.triangle.branch");
        assert_eq!(ToolCategory::Search.icon(), "magnifyingglass");
    }

    #[test]
    fn test_tool_definition_new() {
        let def = ToolDefinition::new(
            "test_tool",
            "A test tool",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
            ToolCategory::Other,
        );

        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(!def.requires_confirmation);
        assert_eq!(def.category, ToolCategory::Other);
    }

    #[test]
    fn test_tool_definition_with_confirmation() {
        let def = ToolDefinition::no_params("delete_file", "Delete a file", ToolCategory::Filesystem)
            .with_confirmation(true);

        assert!(def.requires_confirmation);
    }

    #[test]
    fn test_tool_definition_to_openai() {
        let def = ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            ToolCategory::Search,
        );

        let openai = def.to_openai_function();
        assert_eq!(openai["type"], "function");
        assert_eq!(openai["function"]["name"], "search");
        assert_eq!(openai["function"]["description"], "Search the web");
    }

    #[test]
    fn test_tool_definition_to_anthropic() {
        let def = ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            ToolCategory::Search,
        );

        let anthropic = def.to_anthropic_tool();
        assert_eq!(anthropic["name"], "search");
        assert_eq!(anthropic["description"], "Search the web");
        assert!(anthropic["input_schema"].is_object());
    }

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("Operation completed");

        assert!(result.is_success());
        assert_eq!(result.content, "Operation completed");
        assert!(result.data.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_tool_result_success_with_data() {
        let data = serde_json::json!({"files": ["a.txt", "b.txt"]});
        let result = ToolResult::success_with_data("Found 2 files", data.clone());

        assert!(result.is_success());
        assert_eq!(result.content, "Found 2 files");
        assert_eq!(result.data, Some(data));
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("File not found");

        assert!(!result.is_success());
        assert_eq!(result.error_message(), Some("File not found"));
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_tool_result_partial_error() {
        let result = ToolResult::partial_error("Partial data", "Some operations failed");

        assert!(!result.is_success());
        assert_eq!(result.content, "Partial data");
        assert_eq!(result.error_message(), Some("Some operations failed"));
    }

    #[test]
    fn test_tool_result_to_json() {
        let result = ToolResult::success("OK");
        let json = result.to_json();

        assert_eq!(json["success"], true);
        assert_eq!(json["content"], "OK");
    }

    #[test]
    fn test_tool_category_serialization() {
        let category = ToolCategory::Filesystem;
        let json = serde_json::to_string(&category).unwrap();
        assert_eq!(json, "\"filesystem\"");

        let parsed: ToolCategory = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ToolCategory::Filesystem);
    }
}
