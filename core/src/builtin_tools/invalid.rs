//! Invalid Tool Handler
//!
//! Provides a fallback tool that handles invalid/unknown tool calls gracefully.
//! This allows the LLM to receive feedback about invalid tool names and
//! available alternatives, rather than causing hard errors.
//!
//! Inspired by OpenCode's experimental_repairToolCall pattern.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dispatcher::ToolCategory;
use crate::error::Result;
use crate::AlephTool;

/// Arguments for the Invalid tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvalidToolArgs {
    /// The tool name that was not found
    pub tool: String,
    /// Error message explaining why the tool call failed
    pub error: String,
}

/// Output from the Invalid tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvalidToolOutput {
    /// Always false for invalid tool calls
    pub success: bool,
    /// Error message
    pub message: String,
    /// Suggestion with available tools
    pub suggestion: String,
}

/// Invalid Tool - handles unknown/invalid tool calls
///
/// This tool is automatically invoked when:
/// 1. A tool name is not found in the registry
/// 2. Case-insensitive matching also fails
///
/// It provides helpful feedback to the LLM about what went wrong
/// and what tools are available.
#[derive(Clone)]
pub struct InvalidTool {
    /// List of available tool names for suggestions
    available_tools: Vec<String>,
}

impl InvalidTool {
    /// Create a new InvalidTool with a list of available tools
    pub fn new(available_tools: Vec<String>) -> Self {
        Self { available_tools }
    }

    /// Create an InvalidTool with no available tools (will be updated later)
    pub fn empty() -> Self {
        Self {
            available_tools: Vec::new(),
        }
    }

    /// Update the list of available tools
    pub fn update_available_tools(&mut self, tools: Vec<String>) {
        self.available_tools = tools;
    }

    /// Get the list of available tools
    pub fn available_tools(&self) -> &[String] {
        &self.available_tools
    }
}

#[async_trait]
impl AlephTool for InvalidTool {
    const NAME: &'static str = "invalid";
    const DESCRIPTION: &'static str =
        "Internal tool for handling invalid tool calls. Returns error information and available alternatives.";

    type Args = InvalidToolArgs;
    type Output = InvalidToolOutput;

    fn category(&self) -> ToolCategory {
        ToolCategory::Builtin
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let suggestion = if self.available_tools.is_empty() {
            "No tools are currently available.".to_string()
        } else {
            // Limit to first 20 tools for readability
            let tool_list: Vec<&str> = self
                .available_tools
                .iter()
                .take(20)
                .map(|s| s.as_str())
                .collect();
            let suffix = if self.available_tools.len() > 20 {
                format!(" (and {} more)", self.available_tools.len() - 20)
            } else {
                String::new()
            };
            format!(
                "Available tools: {}{}. Please use one of these.",
                tool_list.join(", "),
                suffix
            )
        };

        Ok(InvalidToolOutput {
            success: false,
            message: format!("Tool '{}' not found. Error: {}", args.tool, args.error),
            suggestion,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_invalid_tool_basic() {
        let tool = InvalidTool::new(vec!["search".to_string(), "read_file".to_string()]);

        let args = InvalidToolArgs {
            tool: "unknown_tool".to_string(),
            error: "Tool not found in registry".to_string(),
        };

        let result = AlephTool::call(&tool, args).await.unwrap();

        assert!(!result.success);
        assert!(result.message.contains("unknown_tool"));
        assert!(result.suggestion.contains("search"));
        assert!(result.suggestion.contains("read_file"));
    }

    #[tokio::test]
    async fn test_invalid_tool_empty() {
        let tool = InvalidTool::empty();

        let args = InvalidToolArgs {
            tool: "nonexistent".to_string(),
            error: "Not found".to_string(),
        };

        let result = AlephTool::call(&tool, args).await.unwrap();

        assert!(!result.success);
        assert!(result.suggestion.contains("No tools are currently available"));
    }

    #[tokio::test]
    async fn test_invalid_tool_many_tools() {
        let tools: Vec<String> = (0..30).map(|i| format!("tool_{}", i)).collect();
        let tool = InvalidTool::new(tools);

        let args = InvalidToolArgs {
            tool: "unknown".to_string(),
            error: "Not found".to_string(),
        };

        let result = AlephTool::call(&tool, args).await.unwrap();

        // Should show first 20 and indicate more exist
        assert!(result.suggestion.contains("tool_0"));
        assert!(result.suggestion.contains("tool_19"));
        assert!(result.suggestion.contains("and 10 more"));
    }

    #[test]
    fn test_invalid_tool_definition() {
        let tool = InvalidTool::empty();
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "invalid");
        assert_eq!(def.category, ToolCategory::Builtin);
    }
}
