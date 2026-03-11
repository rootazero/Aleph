//! Tool Definition Types
//!
//! Contains tool definition and structured metadata types:
//! - ToolDefinition: Basic tool definition for LLM function calling
//! - Capability: Precise capability description
//! - ToolDiff: Tool differentiation for similar tools
//! - StructuredToolMeta: Grouped metadata for tool selection

use super::category::ToolCategory;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    /// Additional LLM context (examples, usage notes)
    /// Injected into system prompt when tool is available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_context: Option<String>,

    /// Whether this tool's schema is strict-mode compatible.
    /// When true, the schema will be strictified (additionalProperties: false,
    /// all properties required) and the strict flag sent to providers that support it.
    #[serde(default)]
    pub strict: bool,
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
            llm_context: None,
            strict: false,
        }
    }

    /// Set requires_confirmation flag
    pub fn with_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Set LLM context (examples, usage notes)
    pub fn with_llm_context(mut self, context: String) -> Self {
        self.llm_context = Some(context);
        self
    }

    /// Set strict mode flag
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
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
        let mut func = serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters
            }
        });
        if self.strict {
            func["function"]["strict"] = serde_json::json!(true);
        }
        func
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
// Structured Tool Description Types (for LLM tool selection)
// =============================================================================

/// Capability description for structured tool definitions
///
/// Provides precise enumeration of what a tool can do, helping LLM
/// make accurate tool selection decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// Action verb (e.g., "search", "read", "write")
    pub action: String,
    /// Target of action (e.g., "file names", "file content")
    pub target: String,
    /// Scope limitation (e.g., "project directory", "current file")
    pub scope: String,
    /// Output type (e.g., "list of paths", "file content string")
    pub output: String,
}

impl Capability {
    /// Create a new capability
    pub fn new(
        action: impl Into<String>,
        target: impl Into<String>,
        scope: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            target: target.into(),
            scope: scope.into(),
            output: output.into(),
        }
    }

    /// Format for LLM prompt
    pub fn to_prompt(&self) -> String {
        format!(
            "{} {} within {} → {}",
            self.action, self.target, self.scope, self.output
        )
    }
}

/// Tool differentiation for distinguishing similar tools
///
/// Helps LLM choose between tools with overlapping functionality
/// by explicitly stating when to use this tool vs. another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDiff {
    /// The other tool being compared
    pub other_tool: String,
    /// What this tool does (brief)
    pub this_tool: String,
    /// What the other tool does (brief)
    pub other_is: String,
    /// When to choose this tool
    pub choose_this_when: String,
    /// When to choose the other tool
    pub choose_other_when: String,
}

impl ToolDiff {
    /// Create a new tool differentiation
    pub fn new(
        other_tool: impl Into<String>,
        this_tool: impl Into<String>,
        other_is: impl Into<String>,
        choose_this_when: impl Into<String>,
        choose_other_when: impl Into<String>,
    ) -> Self {
        Self {
            other_tool: other_tool.into(),
            this_tool: this_tool.into(),
            other_is: other_is.into(),
            choose_this_when: choose_this_when.into(),
            choose_other_when: choose_other_when.into(),
        }
    }

    /// Format for LLM prompt
    pub fn to_prompt(&self) -> String {
        format!(
            "vs {}: this={}, that={}. Choose this when: {}",
            self.other_tool, self.this_tool, self.other_is, self.choose_this_when
        )
    }
}

/// Structured metadata for enhanced tool descriptions
///
/// Groups all structured metadata that helps LLM make accurate
/// tool selection decisions. This includes:
/// - Precise capability enumeration
/// - Explicitly unsuitable scenarios (prevent misuse)
/// - Differentiation from similar tools
/// - Typical use cases (positive examples)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuredToolMeta {
    /// Core capabilities (precise enumeration)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,

    /// Explicitly unsuitable scenarios (prevent misuse)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_suitable_for: Vec<String>,

    /// Differentiation from similar tools
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differentiation: Vec<ToolDiff>,

    /// Typical use cases (positive examples)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub use_when: Vec<String>,
}

impl StructuredToolMeta {
    /// Check if this metadata is empty
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
            && self.not_suitable_for.is_empty()
            && self.differentiation.is_empty()
            && self.use_when.is_empty()
    }

    /// Format for LLM prompt
    pub fn to_prompt(&self) -> String {
        let mut parts = Vec::new();

        if !self.capabilities.is_empty() {
            let caps = self
                .capabilities
                .iter()
                .map(|c| c.to_prompt())
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(format!("Can: {}", caps));
        }

        if !self.not_suitable_for.is_empty() {
            parts.push(format!("NOT for: {}", self.not_suitable_for.join(", ")));
        }

        if !self.differentiation.is_empty() {
            let diffs = self
                .differentiation
                .iter()
                .map(|d| d.to_prompt())
                .collect::<Vec<_>>()
                .join("; ");
            parts.push(diffs);
        }

        if !self.use_when.is_empty() {
            parts.push(format!("Use when: {}", self.use_when.join("; ")));
        }

        parts.join(" | ")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_creation() {
        let cap = Capability::new("search", "file names", "project directory", "list of paths");
        assert_eq!(cap.action, "search");
        assert_eq!(cap.target, "file names");
    }

    #[test]
    fn test_tool_diff_creation() {
        let diff = ToolDiff::new(
            "search_content",
            "matches file name/path",
            "matches file content",
            "know file name",
            "know content",
        );
        assert_eq!(diff.other_tool, "search_content");
        assert_eq!(diff.choose_this_when, "know file name");
    }

    #[test]
    fn test_capability_to_prompt() {
        let cap = Capability::new("search", "file names", "project directory", "list of paths");
        let prompt = cap.to_prompt();
        assert_eq!(
            prompt,
            "search file names within project directory → list of paths"
        );
    }

    #[test]
    fn test_tool_diff_to_prompt() {
        let diff = ToolDiff::new(
            "search_content",
            "matches names",
            "matches content",
            "know file name",
            "know content",
        );
        let prompt = diff.to_prompt();
        assert_eq!(
            prompt,
            "vs search_content: this=matches names, that=matches content. Choose this when: know file name"
        );
    }

    #[test]
    fn test_structured_tool_meta_is_empty() {
        let meta = StructuredToolMeta::default();
        assert!(meta.is_empty());

        let meta_with_cap = StructuredToolMeta {
            capabilities: vec![Capability::new("search", "files", "dir", "paths")],
            ..Default::default()
        };
        assert!(!meta_with_cap.is_empty());
    }

    #[test]
    fn test_structured_tool_meta_to_prompt() {
        let meta = StructuredToolMeta {
            capabilities: vec![Capability::new(
                "search",
                "file names",
                "project",
                "file paths",
            )],
            not_suitable_for: vec!["searching file content".to_string()],
            differentiation: vec![ToolDiff::new(
                "search_content",
                "matches names",
                "matches content",
                "know file name",
                "know content",
            )],
            use_when: vec!["user mentions specific file name".to_string()],
        };

        let prompt = meta.to_prompt();
        assert!(prompt.contains("Can:"));
        assert!(prompt.contains("NOT for:"));
        assert!(prompt.contains("vs search_content"));
        assert!(prompt.contains("Use when:"));
    }
}
