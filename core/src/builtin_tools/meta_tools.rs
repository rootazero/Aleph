//! Meta tools for smart tool discovery
//!
//! These tools allow the LLM to discover and query available tools at runtime,
//! enabling a two-stage tool discovery pattern that reduces token consumption.
//!
//! # Tools
//!
//! - [`ListToolsTool`] - List available tools by category
//! - [`GetToolSchemaTool`] - Get full schema for a specific tool
//!
//! # Usage Pattern
//!
//! 1. LLM receives a compact tool index with basic info (name + summary)
//! 2. If LLM needs a tool not in its full-schema set, it calls `get_tool_schema`
//! 3. System returns the full JSON Schema
//! 4. LLM can then call the tool with correct parameters

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

use super::error::ToolError;
use crate::dispatcher::{ToolIndexEntry, ToolRegistry};
use crate::error::Result;
use crate::tools::AlephTool;

// ============================================================================
// ListToolsTool
// ============================================================================

/// Arguments for list_tools
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListToolsArgs {
    /// Category filter (optional): core, builtin, mcp, skill, custom
    /// If not specified, returns all tools grouped by category
    #[serde(default)]
    pub category: Option<String>,
}

/// Output from list_tools containing categorized tool lists
#[derive(Debug, Clone, Serialize)]
pub struct ListToolsOutput {
    /// Total number of tools
    pub total_count: usize,
    /// Tools organized by category
    pub categories: Value,
    /// Flat list of tool entries (for programmatic access)
    pub tools: Vec<ToolIndexEntry>,
}

/// Meta tool for listing available tools
///
/// Allows the LLM to discover what tools are available without
/// having all their schemas in context.
///
/// # Example Response
///
/// ```json
/// {
///   "total_count": 45,
///   "categories": {
///     "core": ["search", "file_ops"],
///     "mcp": ["github:pr_list", "github:issue_create"],
///     "skill": ["code-review", "refine-text"]
///   }
/// }
/// ```
pub struct ListToolsTool {
    registry: Arc<RwLock<ToolRegistry>>,
}

impl ListToolsTool {
    /// Tool identifier
    pub const NAME: &'static str = "list_tools";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "List available tools by category. Use this to discover what tools are available before calling get_tool_schema for specific tools.";

    /// Create a new ListToolsTool with registry reference
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self { registry }
    }

    /// Execute the list operation (internal implementation)
    async fn call_impl(&self, args: ListToolsArgs) -> std::result::Result<ListToolsOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let category_filter = args.category.as_deref().unwrap_or("all");
        notify_tool_start(Self::NAME, &format!("列出工具: {}", category_filter));

        let registry = self.registry.read().await;
        let tools = registry
            .list_tools_by_category(args.category.as_deref())
            .await;

        // Group tools by category
        let mut categories: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for tool in &tools {
            let cat = tool.category.display_name().to_string();
            categories.entry(cat).or_default().push(tool.name.clone());
        }

        let total_count = tools.len();
        let categories_json = serde_json::to_value(&categories).unwrap_or_default();

        notify_tool_result(
            Self::NAME,
            &format!("找到 {} 个工具", total_count),
            true,
        );

        Ok(ListToolsOutput {
            total_count,
            categories: categories_json,
            tools,
        })
    }
}

impl Clone for ListToolsTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

/// Implementation of AlephTool trait for ListToolsTool
#[async_trait]
impl AlephTool for ListToolsTool {
    const NAME: &'static str = "list_tools";
    const DESCRIPTION: &'static str = "List available tools by category. Use this to discover what tools are available before calling get_tool_schema for specific tools.";

    type Args = ListToolsArgs;
    type Output = ListToolsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// ============================================================================
// GetToolSchemaTool
// ============================================================================

/// Arguments for get_tool_schema
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct GetToolSchemaArgs {
    /// Name of the tool to get schema for
    pub tool_name: String,
}

/// Output from get_tool_schema containing full tool definition
#[derive(Debug, Clone, Serialize)]
pub struct GetToolSchemaOutput {
    /// Whether the tool was found
    pub found: bool,
    /// Tool name (may differ from input if alias matched)
    pub name: String,
    /// Full description
    pub description: String,
    /// JSON Schema for parameters
    pub parameters: Value,
    /// Tool category
    pub category: String,
    /// Whether tool requires confirmation
    pub requires_confirmation: bool,
    /// Usage example
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// Error message if tool not found
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Similar tool suggestions if not found
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

/// Meta tool for getting full tool schema
///
/// Allows the LLM to get the complete JSON Schema for a tool
/// when it needs to call a tool that's only in the index.
///
/// # Example Response
///
/// ```json
/// {
///   "found": true,
///   "name": "github:pr_create",
///   "description": "Create a pull request on GitHub",
///   "parameters": {
///     "type": "object",
///     "properties": {
///       "repo": { "type": "string" },
///       "title": { "type": "string" },
///       "body": { "type": "string" }
///     },
///     "required": ["repo", "title"]
///   }
/// }
/// ```
pub struct GetToolSchemaTool {
    registry: Arc<RwLock<ToolRegistry>>,
}

impl GetToolSchemaTool {
    /// Tool identifier
    pub const NAME: &'static str = "get_tool_schema";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Get the full JSON Schema definition for a specific tool. Use this before calling a tool that's not in your full-schema set.";

    /// Create a new GetToolSchemaTool with registry reference
    pub fn new(registry: Arc<RwLock<ToolRegistry>>) -> Self {
        Self { registry }
    }

    /// Execute the schema lookup (internal implementation)
    async fn call_impl(&self, args: GetToolSchemaArgs) -> std::result::Result<GetToolSchemaOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        notify_tool_start(Self::NAME, &format!("获取工具定义: {}", args.tool_name));

        let registry = self.registry.read().await;

        // Try to find the tool
        if let Some(tool) = registry.get_tool_definition(&args.tool_name).await {
            debug!(tool_name = %args.tool_name, "Found tool definition");

            let output = GetToolSchemaOutput {
                found: true,
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters_schema.clone().unwrap_or_else(|| {
                    json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    })
                }),
                category: tool.source.label().to_string(),
                requires_confirmation: tool.requires_confirmation,
                usage: tool.usage.clone(),
                error: None,
                suggestions: vec![],
            };

            notify_tool_result(Self::NAME, &format!("已获取 {} 的定义", tool.name), true);

            return Ok(output);
        }

        // Tool not found - try to find similar tools
        debug!(tool_name = %args.tool_name, "Tool not found, searching for similar");

        let all_tools = registry.list_all().await;
        let query_lower = args.tool_name.to_lowercase();

        let suggestions: Vec<String> = all_tools
            .iter()
            .filter(|t| {
                t.name.to_lowercase().contains(&query_lower)
                    || query_lower.contains(&t.name.to_lowercase())
                    || levenshtein_distance(&t.name.to_lowercase(), &query_lower) <= 3
            })
            .take(5)
            .map(|t| t.name.clone())
            .collect();

        let error_msg = format!("Tool not found: {}", args.tool_name);
        notify_tool_result(Self::NAME, &error_msg, false);

        Ok(GetToolSchemaOutput {
            found: false,
            name: args.tool_name.clone(),
            description: String::new(),
            parameters: json!({}),
            category: String::new(),
            requires_confirmation: false,
            usage: None,
            error: Some(error_msg),
            suggestions,
        })
    }
}

impl Clone for GetToolSchemaTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

/// Implementation of AlephTool trait for GetToolSchemaTool
#[async_trait]
impl AlephTool for GetToolSchemaTool {
    const NAME: &'static str = "get_tool_schema";
    const DESCRIPTION: &'static str = "Get the full JSON Schema definition for a specific tool. Use this before calling a tool that's not in your full-schema set.";

    type Args = GetToolSchemaArgs;
    type Output = GetToolSchemaOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Simple Levenshtein distance for fuzzy matching
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for i in 0..=a_len {
        matrix[i][0] = i;
    }
    for j in 0..=b_len {
        matrix[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[a_len][b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_tools_args_default() {
        let args: ListToolsArgs = serde_json::from_str(r#"{}"#).unwrap();
        assert!(args.category.is_none());
    }

    #[test]
    fn test_list_tools_args_with_category() {
        let args: ListToolsArgs = serde_json::from_str(r#"{"category": "mcp"}"#).unwrap();
        assert_eq!(args.category, Some("mcp".to_string()));
    }

    #[test]
    fn test_get_tool_schema_args() {
        let args: GetToolSchemaArgs =
            serde_json::from_str(r#"{"tool_name": "github:pr_list"}"#).unwrap();
        assert_eq!(args.tool_name, "github:pr_list");
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("abc", "abd"), 1);
        assert_eq!(levenshtein_distance("search", "serach"), 2);
        assert_eq!(levenshtein_distance("github", "githu"), 1);
    }
}
