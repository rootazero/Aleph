//! Dynamic Prompt Builder for L3 AI Routing
//!
//! This module provides flexible prompt generation for the Dispatcher Layer:
//!
//! - Tool list formatting for LLM prompts
//! - L3 routing system prompt templates
//! - Confidence scoring instructions
//! - JSON output format specifications
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{PromptBuilder, ToolRegistry};
//!
//! let registry = ToolRegistry::new();
//! registry.register_native_tools().await;
//!
//! // Build tool list for L3 prompt
//! let tools = registry.list_all().await;
//! let tool_list = PromptBuilder::build_tool_list(&tools);
//!
//! // Build complete L3 routing prompt
//! let system_prompt = PromptBuilder::build_l3_routing_prompt(&tools, None);
//! ```

use super::types::{ToolSource, UnifiedTool};
use crate::utils::json_extract::extract_json_robust;

/// Prompt format options for tool list generation
#[derive(Debug, Clone, Copy, Default)]
pub enum PromptFormat {
    /// Markdown list format (default)
    /// - **name**: description (args: param1, param2)
    #[default]
    Markdown,

    /// Compact format for limited context
    /// name: description
    Compact,

    /// XML-style format for structured parsing
    /// <tool name="..." description="..."/>
    Xml,

    /// JSON format for programmatic use
    /// {"name": "...", "description": "...", "parameters": {...}}
    Json,
}

/// Filter options for tool list generation
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    /// Only include active tools (default: true)
    pub active_only: bool,

    /// Filter by source types (empty = all)
    pub source_types: Vec<String>,

    /// Exclude specific tool IDs
    pub exclude_ids: Vec<String>,

    /// Maximum number of tools to include (0 = unlimited)
    pub max_tools: usize,
}

impl ToolFilter {
    /// Create a filter for active tools only
    pub fn active() -> Self {
        Self {
            active_only: true,
            ..Default::default()
        }
    }

    /// Create a filter for specific source types
    pub fn source_types(types: Vec<String>) -> Self {
        Self {
            active_only: true,
            source_types: types,
            ..Default::default()
        }
    }
}

/// Dynamic Prompt Builder for tool-related prompts
pub struct PromptBuilder;

impl PromptBuilder {
    // =========================================================================
    // Tool List Generation
    // =========================================================================

    /// Build a formatted tool list from UnifiedTool slice
    ///
    /// # Arguments
    ///
    /// * `tools` - Slice of tools to include
    /// * `format` - Output format (default: Markdown)
    /// * `filter` - Optional filter options
    ///
    /// # Returns
    ///
    /// Formatted string suitable for LLM prompt injection
    pub fn build_tool_list(
        tools: &[UnifiedTool],
        format: PromptFormat,
        filter: Option<&ToolFilter>,
    ) -> String {
        let filter = filter.cloned().unwrap_or_else(ToolFilter::active);
        let filtered = Self::apply_filter(tools, &filter);

        match format {
            PromptFormat::Markdown => Self::format_markdown(&filtered),
            PromptFormat::Compact => Self::format_compact(&filtered),
            PromptFormat::Xml => Self::format_xml(&filtered),
            PromptFormat::Json => Self::format_json(&filtered),
        }
    }

    /// Build tool list with default markdown format
    pub fn build_tool_list_markdown(tools: &[UnifiedTool]) -> String {
        Self::build_tool_list(tools, PromptFormat::Markdown, None)
    }

    fn apply_filter(tools: &[UnifiedTool], filter: &ToolFilter) -> Vec<UnifiedTool> {
        let mut result: Vec<_> = tools
            .iter()
            .filter(|t| {
                // Active filter
                if filter.active_only && !t.is_active {
                    return false;
                }

                // Source type filter
                if !filter.source_types.is_empty()
                    && !filter.source_types.contains(&t.source.label().to_string())
                {
                    return false;
                }

                // Exclude filter
                if filter.exclude_ids.contains(&t.id) {
                    return false;
                }

                true
            })
            .cloned()
            .collect();

        // Sort alphabetically by name
        result.sort_by(|a, b| a.name.cmp(&b.name));

        // Apply max limit
        if filter.max_tools > 0 && result.len() > filter.max_tools {
            result.truncate(filter.max_tools);
        }

        result
    }

    fn format_markdown(tools: &[UnifiedTool]) -> String {
        tools
            .iter()
            .map(|t| t.to_prompt_line())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn format_compact(tools: &[UnifiedTool]) -> String {
        tools
            .iter()
            .map(|t| format!("{}: {}", t.name, t.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn format_xml(tools: &[UnifiedTool]) -> String {
        let mut xml = String::from("<tools>\n");

        for tool in tools {
            let source = match &tool.source {
                ToolSource::Native => "native".to_string(),
                ToolSource::Builtin => "builtin".to_string(),
                ToolSource::Mcp { server } => format!("mcp:{}", server),
                ToolSource::Skill { id } => format!("skill:{}", id),
                ToolSource::Custom { rule_index } => format!("custom:{}", rule_index),
            };

            xml.push_str(&format!(
                "  <tool name=\"{}\" source=\"{}\" description=\"{}\"/>\n",
                escape_xml(&tool.name),
                escape_xml(&source),
                escape_xml(&tool.description)
            ));
        }

        xml.push_str("</tools>");
        xml
    }

    fn format_json(tools: &[UnifiedTool]) -> String {
        let tools_json: Vec<_> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "source": t.source.label(),
                    "parameters": t.parameters_schema
                })
            })
            .collect();

        serde_json::to_string_pretty(&tools_json).unwrap_or_else(|_| "[]".to_string())
    }

    // =========================================================================
    // L3 Routing Prompt Templates
    // =========================================================================

    /// Build complete L3 routing system prompt
    ///
    /// This generates a system prompt for AI-based tool routing that includes:
    /// - Role definition
    /// - Available tools list
    /// - Confidence scoring instructions
    /// - Output format specification (JSON)
    ///
    /// # Arguments
    ///
    /// * `tools` - Available tools to include in the prompt
    /// * `conversation_context` - Optional conversation history for context
    ///
    /// # Returns
    ///
    /// Complete system prompt string for L3 routing
    pub fn build_l3_routing_prompt(
        tools: &[UnifiedTool],
        conversation_context: Option<&str>,
    ) -> String {
        let tool_list = Self::build_tool_list_markdown(tools);

        let context_section = conversation_context
            .map(|ctx| {
                format!(
                    r#"
## Recent Conversation Context

{ctx}
"#
                )
            })
            .unwrap_or_default();

        format!(
            r#"You are an intelligent tool router for the Aether AI assistant.

Your task is to analyze user input and determine which tool (if any) should handle the request.

## Available Tools

{tool_list}
{context_section}
## Instructions

1. Analyze the user's input to understand their intent
2. Determine if any available tool is appropriate for handling this request
3. If a tool matches, extract any parameters from the input
4. Provide a confidence score (0.0 - 1.0) for your decision

## Confidence Scoring Guidelines

- **1.0**: Explicit tool invocation (e.g., "/search weather today")
- **0.9**: Clear intent match with high certainty
- **0.7-0.8**: Good match but some ambiguity
- **0.5-0.6**: Possible match, user confirmation recommended
- **0.3-0.4**: Weak match, likely not the intended tool
- **0.0-0.2**: No relevant tool found

## Output Format

Respond with a JSON object ONLY (no markdown, no explanation):

```json
{{
  "tool": "tool_name or null if no match",
  "confidence": 0.0-1.0,
  "parameters": {{ "param_name": "value" }},
  "reason": "Brief explanation of your decision"
}}
```

If no tool is appropriate, respond with:

```json
{{
  "tool": null,
  "confidence": 0.0,
  "parameters": {{}},
  "reason": "No matching tool found - treat as general chat"
}}
```
"#
        )
    }

    /// Build a minimal routing prompt (for lower latency)
    ///
    /// This is a condensed version for when latency is critical.
    pub fn build_l3_routing_prompt_minimal(tools: &[UnifiedTool]) -> String {
        let tool_list = Self::build_tool_list(tools, PromptFormat::Compact, None);

        format!(
            r#"Route user input to a tool. Available tools:
{tool_list}

Respond JSON only: {{"tool": "name|null", "confidence": 0.0-1.0, "parameters": {{}}, "reason": "why"}}"#
        )
    }

    /// Build prompt for parameter extraction only
    ///
    /// Used when tool is already determined but parameters need extraction.
    pub fn build_parameter_extraction_prompt(tool: &UnifiedTool, user_input: &str) -> String {
        let schema = tool
            .parameters_schema
            .as_ref()
            .map(|s| serde_json::to_string_pretty(s).unwrap_or_default())
            .unwrap_or_else(|| "{}".to_string());

        format!(
            r#"Extract parameters for the "{}" tool from the user input.

Tool: {name}
Description: {description}
Parameter Schema:
{schema}

User Input: "{user_input}"

Respond with a JSON object containing extracted parameters only:

```json
{{
  "param_name": "extracted_value"
}}
```

If a required parameter cannot be extracted, use null."#,
            tool.name,
            name = tool.name,
            description = tool.description,
            schema = schema,
            user_input = user_input
        )
    }

    // =========================================================================
    // Response Parsing Helpers
    // =========================================================================

    /// Parse L3 routing response JSON
    ///
    /// Uses the centralized `extract_json_robust()` utility which handles:
    /// - Pure JSON responses
    /// - JSON in markdown code blocks
    /// - JSON mixed with explanatory text
    /// - Multiple JSON objects (extracts the first complete one)
    ///
    /// # Arguments
    ///
    /// * `response` - Raw LLM response text
    ///
    /// # Returns
    ///
    /// Parsed routing decision or None if parsing failed
    pub fn parse_l3_response(response: &str) -> Option<L3RoutingResponse> {
        // Use centralized robust JSON extraction
        let json_value = extract_json_robust(response)?;

        // Try to deserialize into L3RoutingResponse
        serde_json::from_value(json_value).ok()
    }
}

/// L3 routing response structure
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct L3RoutingResponse {
    /// Selected tool name (None if no match)
    pub tool: Option<String>,

    /// Confidence score (0.0 - 1.0)
    pub confidence: f32,

    /// Extracted parameters
    #[serde(default)]
    pub parameters: serde_json::Value,

    /// Routing decision explanation
    #[serde(default)]
    pub reason: String,
}

impl L3RoutingResponse {
    /// Check if a tool was matched
    pub fn has_match(&self) -> bool {
        self.tool.is_some() && self.confidence > 0.0
    }

    /// Check if confirmation is recommended based on confidence
    pub fn needs_confirmation(&self, threshold: f32) -> bool {
        self.has_match() && self.confidence < threshold
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Escape XML special characters
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// Note: The old `extract_json_from_response()` function has been removed.
// JSON extraction is now handled by the centralized `crate::utils::json_extract::extract_json_robust()`
// which uses proper brace-matching instead of the vulnerable greedy `rfind('}')` approach.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_tools() -> Vec<UnifiedTool> {
        vec![
            UnifiedTool::new(
                "native:search",
                "search",
                "Search the web for information",
                ToolSource::Native,
            )
            .with_parameters_schema(json!({
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                }
            })),
            UnifiedTool::new(
                "mcp:fs:read_file",
                "read_file",
                "Read contents of a file",
                ToolSource::Mcp {
                    server: "fs".to_string(),
                },
            ),
            UnifiedTool::new(
                "skill:refine-text",
                "refine-text",
                "Improve and polish writing",
                ToolSource::Skill {
                    id: "refine-text".to_string(),
                },
            )
            .with_active(false), // Inactive tool
        ]
    }

    #[test]
    fn test_build_tool_list_markdown() {
        let tools = create_test_tools();
        let result = PromptBuilder::build_tool_list_markdown(&tools);

        // Should include active tools
        assert!(result.contains("**search**"));
        assert!(result.contains("**read_file**"));
        // Should NOT include inactive tool
        assert!(!result.contains("**refine-text**"));
    }

    #[test]
    fn test_build_tool_list_compact() {
        let tools = create_test_tools();
        let result = PromptBuilder::build_tool_list(&tools, PromptFormat::Compact, None);

        assert!(result.contains("search: Search the web"));
        assert!(result.contains("read_file: Read contents"));
    }

    #[test]
    fn test_build_tool_list_xml() {
        let tools = create_test_tools();
        let result = PromptBuilder::build_tool_list(&tools, PromptFormat::Xml, None);

        assert!(result.contains("<tools>"));
        assert!(result.contains("</tools>"));
        assert!(result.contains("name=\"search\""));
        assert!(result.contains("source=\"native\""));
    }

    #[test]
    fn test_build_tool_list_json() {
        let tools = create_test_tools();
        let result = PromptBuilder::build_tool_list(&tools, PromptFormat::Json, None);

        let parsed: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.len(), 2); // Only active tools
        assert_eq!(parsed[1]["name"], "search"); // Sorted alphabetically
    }

    #[test]
    fn test_tool_filter_source_types() {
        let tools = create_test_tools();
        let filter = ToolFilter::source_types(vec!["Native".to_string()]);
        let result = PromptBuilder::build_tool_list(&tools, PromptFormat::Compact, Some(&filter));

        assert!(result.contains("search"));
        assert!(!result.contains("read_file")); // MCP tool filtered out
    }

    #[test]
    fn test_tool_filter_max_tools() {
        let tools = create_test_tools();
        let filter = ToolFilter {
            active_only: true,
            max_tools: 1,
            ..Default::default()
        };
        let result = PromptBuilder::build_tool_list(&tools, PromptFormat::Compact, Some(&filter));

        // Should only have 1 tool (first alphabetically)
        assert_eq!(result.lines().count(), 1);
    }

    #[test]
    fn test_build_l3_routing_prompt() {
        let tools = create_test_tools();
        let prompt = PromptBuilder::build_l3_routing_prompt(&tools, None);

        // Check required sections
        assert!(prompt.contains("## Available Tools"));
        assert!(prompt.contains("## Confidence Scoring Guidelines"));
        assert!(prompt.contains("## Output Format"));
        assert!(prompt.contains("\"tool\":"));
        assert!(prompt.contains("\"confidence\":"));
    }

    #[test]
    fn test_build_l3_routing_prompt_with_context() {
        let tools = create_test_tools();
        let context = "User asked about weather earlier";
        let prompt = PromptBuilder::build_l3_routing_prompt(&tools, Some(context));

        assert!(prompt.contains("## Recent Conversation Context"));
        assert!(prompt.contains("weather earlier"));
    }

    #[test]
    fn test_build_l3_routing_prompt_minimal() {
        let tools = create_test_tools();
        let prompt = PromptBuilder::build_l3_routing_prompt_minimal(&tools);

        // Should be much shorter
        assert!(prompt.len() < 500);
        assert!(prompt.contains("search:"));
        assert!(prompt.contains("Respond JSON only"));
    }

    #[test]
    fn test_build_parameter_extraction_prompt() {
        let tool = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web",
            ToolSource::Native,
        )
        .with_parameters_schema(json!({
            "properties": {
                "query": { "type": "string" }
            }
        }));

        let prompt = PromptBuilder::build_parameter_extraction_prompt(&tool, "find news about AI");

        assert!(prompt.contains("search"));
        assert!(prompt.contains("query"));
        assert!(prompt.contains("find news about AI"));
    }

    #[test]
    fn test_parse_l3_response_raw_json() {
        let response = r#"{"tool": "search", "confidence": 0.9, "parameters": {"query": "test"}, "reason": "matched"}"#;
        let parsed = PromptBuilder::parse_l3_response(response).unwrap();

        assert_eq!(parsed.tool, Some("search".to_string()));
        assert_eq!(parsed.confidence, 0.9);
        assert!(parsed.has_match());
    }

    #[test]
    fn test_parse_l3_response_markdown_block() {
        let response = r#"
Here is my analysis:

```json
{
  "tool": "search",
  "confidence": 0.8,
  "parameters": {},
  "reason": "User wants to search"
}
```
"#;
        let parsed = PromptBuilder::parse_l3_response(response).unwrap();

        assert_eq!(parsed.tool, Some("search".to_string()));
        assert_eq!(parsed.confidence, 0.8);
    }

    #[test]
    fn test_parse_l3_response_no_match() {
        let response = r#"{"tool": null, "confidence": 0.0, "parameters": {}, "reason": "No match"}"#;
        let parsed = PromptBuilder::parse_l3_response(response).unwrap();

        assert!(parsed.tool.is_none());
        assert!(!parsed.has_match());
    }

    #[test]
    fn test_l3_response_needs_confirmation() {
        let response = L3RoutingResponse {
            tool: Some("search".to_string()),
            confidence: 0.6,
            parameters: json!({}),
            reason: "Possible match".to_string(),
        };

        assert!(response.needs_confirmation(0.8));
        assert!(!response.needs_confirmation(0.5));
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("<test>"), "&lt;test&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("\"quote\""), "&quot;quote&quot;");
    }

    #[test]
    fn test_parse_l3_response_multiple_objects() {
        // This is the key test case - should extract FIRST complete JSON object
        // The old greedy rfind('}') would have incorrectly extracted the second object
        let multiple = r#"First: {"tool": "a", "confidence": 0.9, "parameters": {}, "reason": "first"} Second: {"tool": "b", "confidence": 0.8, "parameters": {}, "reason": "second"}"#;
        let result = PromptBuilder::parse_l3_response(multiple);
        assert!(result.is_some());
        let response = result.unwrap();
        // Should get the FIRST one, not the second
        assert_eq!(response.tool, Some("a".to_string()));
    }

    #[test]
    fn test_parse_l3_response_invalid() {
        assert!(PromptBuilder::parse_l3_response("not json").is_none());
    }
}
