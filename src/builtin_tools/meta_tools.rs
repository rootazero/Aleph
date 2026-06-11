//! Meta tools for smart tool discovery
//!
//! These tools allow the LLM to discover and query available tools at runtime,
//! enabling a two-stage tool discovery pattern that reduces token consumption.
//!
//! # Tools
//!
//! - [`ListToolsTool`] - List available tools by category
//! - [`SearchToolsTool`] - Search tools by free-text keyword/intent
//! - [`GetToolSchemaTool`] - Get full schema for a specific tool
//!
//! # Usage Pattern
//!
//! 1. LLM receives a compact tool index with basic info (name + summary)
//! 2. To find a tool by intent, it calls `search_tools(query)`; to browse a
//!    category it calls `list_tools(category)`
//! 3. If LLM needs a tool not in its full-schema set, it calls `get_tool_schema`
//! 4. System returns the full JSON Schema
//! 5. LLM can then call the tool with correct parameters

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::debug;

use super::error::ToolError;
use crate::error::Result;
use crate::tool_metadata::{ToolCatalog, ToolIndexEntry, UnifiedTool};
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
    registry: Arc<RwLock<ToolCatalog>>,
}

impl ListToolsTool {
    /// Tool identifier
    pub const NAME: &'static str = "list_tools";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "List available tools by category. Use this to discover what tools are available before calling get_tool_schema for specific tools.";

    /// Create a new ListToolsTool with registry reference
    pub fn new(registry: Arc<RwLock<ToolCatalog>>) -> Self {
        Self { registry }
    }

    /// Execute the list operation (internal implementation)
    async fn call_impl(
        &self,
        args: ListToolsArgs,
    ) -> std::result::Result<ListToolsOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let category_filter = args.category.as_deref().unwrap_or("all");
        notify_tool_start(Self::NAME, &format!("列出工具: {category_filter}"));

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

        notify_tool_result(Self::NAME, &format!("找到 {total_count} 个工具"), true);

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
    registry: Arc<RwLock<ToolCatalog>>,
}

impl GetToolSchemaTool {
    /// Tool identifier
    pub const NAME: &'static str = "get_tool_schema";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Get the full JSON Schema definition for a specific tool. Use this before calling a tool that's not in your full-schema set.";

    /// Create a new GetToolSchemaTool with registry reference
    pub fn new(registry: Arc<RwLock<ToolCatalog>>) -> Self {
        Self { registry }
    }

    /// Execute the schema lookup (internal implementation)
    async fn call_impl(
        &self,
        args: GetToolSchemaArgs,
    ) -> std::result::Result<GetToolSchemaOutput, ToolError> {
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
        // Single suggestion source shared with the dispatch ToolNotFound hint
        // (containment + bounded edit distance, ranked best-first).
        let offered: Vec<&str> = all_tools.iter().map(|t| t.name.as_str()).collect();
        let suggestions =
            crate::tools::name_repair::suggest_candidates(&args.tool_name, &offered, 5);

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
// SearchToolsTool
// ============================================================================

/// Arguments for search_tools
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SearchToolsArgs {
    /// Free-text query describing the capability you need (e.g. "screenshot",
    /// "create pull request", "send email"). Matched against tool names and
    /// descriptions; typos are tolerated.
    pub query: String,
    /// Maximum number of matches to return (default 10, capped at 25).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// A single ranked hit from search_tools.
#[derive(Debug, Clone, Serialize)]
pub struct SearchToolHit {
    /// Canonical tool name to pass to `get_tool_schema` / invoke directly.
    pub name: String,
    /// Short description of what the tool does.
    pub description: String,
    /// Source category (builtin, mcp, skill, custom, plugin).
    pub category: String,
    /// Whether the tool requires explicit user confirmation before running.
    pub requires_confirmation: bool,
}

/// Output from search_tools.
#[derive(Debug, Clone, Serialize)]
pub struct SearchToolsOutput {
    /// The query that was searched.
    pub query: String,
    /// Number of matches returned (after the limit is applied).
    pub count: usize,
    /// Ranked matches (substring hits first, then typo-tolerant fuzzy hits).
    pub results: Vec<SearchToolHit>,
}

/// Default and ceiling for the result limit.
const SEARCH_TOOLS_DEFAULT_LIMIT: usize = 10;
const SEARCH_TOOLS_MAX_LIMIT: usize = 25;

/// Meta tool for searching the unified tool catalog by intent.
///
/// Complements [`ListToolsTool`] (browse by category) and
/// [`GetToolSchemaTool`] (fetch one schema): when the catalog is large
/// (many MCP servers / skills / plugins), the LLM can search by free-text
/// keyword to find the right tool, then call `get_tool_schema` for the
/// full parameter schema.
///
/// Ranking: exact/substring matches on name+description first (delegated to
/// [`ToolCatalog::search`]), falling back to Levenshtein typo tolerance only
/// when the substring pass finds nothing — so `"screenshto"` still resolves
/// to `screenshot`.
///
/// # Example Response
///
/// ```json
/// {
///   "query": "pull request",
///   "count": 2,
///   "results": [
///     {"name": "github:pr_create", "description": "Create a pull request", "category": "MCP", "requires_confirmation": true},
///     {"name": "github:pr_list", "description": "List pull requests", "category": "MCP", "requires_confirmation": false}
///   ]
/// }
/// ```
pub struct SearchToolsTool {
    registry: Arc<RwLock<ToolCatalog>>,
}

impl SearchToolsTool {
    /// Tool identifier
    pub const NAME: &'static str = "search_tools";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Search the available tools by a free-text keyword/intent (e.g. \"screenshot\", \"create pull request\"). Use this to find the right tool when you don't know its exact name, then call get_tool_schema for its parameters.";

    /// Create a new SearchToolsTool with registry reference
    pub fn new(registry: Arc<RwLock<ToolCatalog>>) -> Self {
        Self { registry }
    }

    /// Execute the search (internal implementation)
    async fn call_impl(
        &self,
        args: SearchToolsArgs,
    ) -> std::result::Result<SearchToolsOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        let query = args.query.trim().to_string();
        let limit = args
            .limit
            .unwrap_or(SEARCH_TOOLS_DEFAULT_LIMIT)
            .clamp(1, SEARCH_TOOLS_MAX_LIMIT);

        notify_tool_start(Self::NAME, &format!("搜索工具: {query}"));

        if query.is_empty() {
            notify_tool_result(Self::NAME, "空查询", false);
            return Ok(SearchToolsOutput {
                query,
                count: 0,
                results: vec![],
            });
        }

        let registry = self.registry.read().await;

        // Primary pass: reuse the catalog's substring search (name+description,
        // active-only, name-matches-first). This activates the previously
        // dormant `ToolCatalog::search` as a first-class LLM consumer.
        let mut hits = registry.search(&query).await;

        // Fallback: if the substring pass found nothing, try a typo-tolerant
        // Levenshtein scan over the full active catalog — mirrors the
        // not-found suggestion path in `GetToolSchemaTool`. This is what lets
        // a misspelled query still resolve, surpassing a pure substring match.
        if hits.is_empty() {
            let needle = query.to_lowercase();
            let threshold = if needle.len() <= 6 { 2 } else { 3 };
            let mut scored: Vec<(usize, UnifiedTool)> = registry
                .list_all()
                .await
                .into_iter()
                .filter_map(|t| {
                    let name_l = t.name.to_lowercase();
                    let d = levenshtein_distance(&name_l, &needle);
                    (d <= threshold).then_some((d, t))
                })
                .collect();
            scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
            hits = scored.into_iter().map(|(_, t)| t).collect();
        }

        let count_total = hits.len();
        let results: Vec<SearchToolHit> = hits
            .into_iter()
            .take(limit)
            .map(|t| SearchToolHit {
                name: t.name,
                description: t.description,
                category: t.source.label().to_string(),
                requires_confirmation: t.requires_confirmation,
            })
            .collect();

        notify_tool_result(
            Self::NAME,
            &format!("匹配 {} 个工具 (返回 {})", count_total, results.len()),
            true,
        );

        Ok(SearchToolsOutput {
            query,
            count: results.len(),
            results,
        })
    }
}

impl Clone for SearchToolsTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

/// Implementation of AlephTool trait for SearchToolsTool
#[async_trait]
impl AlephTool for SearchToolsTool {
    const NAME: &'static str = "search_tools";
    const DESCRIPTION: &'static str = "Search the available tools by a free-text keyword/intent (e.g. \"screenshot\", \"create pull request\"). Use this to find the right tool when you don't know its exact name, then call get_tool_schema for its parameters.";

    type Args = SearchToolsArgs;
    type Output = SearchToolsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Simple Levenshtein distance for fuzzy matching.
///
/// Exposed at `pub(crate)` so the inbound router can reuse it for slash-
/// command "did you mean" suggestions without duplicating the algorithm.
pub(crate) fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    // Prevent OOM on adversarial input
    if a_len > 500 || b_len > 500 {
        return a_len.abs_diff(b_len);
    }

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut matrix = vec![vec![0usize; b_len + 1]; a_len + 1];

    for (i, row) in matrix.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for (j, val) in matrix[0].iter_mut().enumerate().take(b_len + 1) {
        *val = j;
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
    fn test_search_tools_args_defaults() {
        let args: SearchToolsArgs = serde_json::from_str(r#"{"query": "screenshot"}"#).unwrap();
        assert_eq!(args.query, "screenshot");
        assert!(args.limit.is_none());
    }

    #[test]
    fn test_search_tools_args_with_limit() {
        let args: SearchToolsArgs = serde_json::from_str(r#"{"query": "pr", "limit": 5}"#).unwrap();
        assert_eq!(args.limit, Some(5));
    }

    #[tokio::test]
    async fn test_search_tools_substring_hit() {
        use crate::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};

        let catalog = ToolCatalog::new();
        catalog
            .register_with_conflict_resolution(UnifiedTool::new(
                "screenshot",
                "screenshot",
                "Capture a screenshot of the screen",
                ToolSource::Builtin,
            ))
            .await;
        catalog
            .register_with_conflict_resolution(UnifiedTool::new(
                "send_email",
                "send_email",
                "Send an email message",
                ToolSource::Builtin,
            ))
            .await;

        let tool = SearchToolsTool::new(Arc::new(RwLock::new(catalog)));
        let out = tool
            .call_impl(SearchToolsArgs {
                query: "screenshot".to_string(),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(out.count, 1);
        assert_eq!(out.results[0].name, "screenshot");
    }

    #[tokio::test]
    async fn test_search_tools_fuzzy_fallback_on_typo() {
        use crate::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};

        let catalog = ToolCatalog::new();
        catalog
            .register_with_conflict_resolution(UnifiedTool::new(
                "screenshot",
                "screenshot",
                "Capture a screenshot",
                ToolSource::Builtin,
            ))
            .await;

        let tool = SearchToolsTool::new(Arc::new(RwLock::new(catalog)));
        // "screenshto" is a typo: no substring match, fuzzy fallback resolves it.
        let out = tool
            .call_impl(SearchToolsArgs {
                query: "screenshto".to_string(),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(out.count, 1);
        assert_eq!(out.results[0].name, "screenshot");
    }

    #[tokio::test]
    async fn test_search_tools_empty_query() {
        use crate::tool_metadata::ToolCatalog;

        let tool = SearchToolsTool::new(Arc::new(RwLock::new(ToolCatalog::new())));
        let out = tool
            .call_impl(SearchToolsArgs {
                query: "   ".to_string(),
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(out.count, 0);
    }

    #[tokio::test]
    async fn test_search_tools_limit_clamped() {
        use crate::tool_metadata::{ToolCatalog, ToolSource, UnifiedTool};

        let catalog = ToolCatalog::new();
        for i in 0..30 {
            catalog
                .register_with_conflict_resolution(UnifiedTool::new(
                    format!("search_tool_{i}"),
                    format!("search_tool_{i}"),
                    "a searchable tool",
                    ToolSource::Builtin,
                ))
                .await;
        }
        let tool = SearchToolsTool::new(Arc::new(RwLock::new(catalog)));
        // Request 1000 → clamped to SEARCH_TOOLS_MAX_LIMIT (25).
        let out = tool
            .call_impl(SearchToolsArgs {
                query: "searchable".to_string(),
                limit: Some(1000),
            })
            .await
            .unwrap();
        assert_eq!(out.count, SEARCH_TOOLS_MAX_LIMIT);
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
