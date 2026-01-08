//! Dispatcher Type Definitions
//!
//! Core data structures for the Dispatcher Layer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool source origin
///
/// Identifies where a tool comes from, enabling proper routing
/// and UI grouping (badges, icons).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ToolSource {
    /// Built-in native capabilities (Search, Video)
    /// These are always available without any configuration.
    Native,

    /// MCP (Model Context Protocol) server
    /// External or builtin MCP servers providing tools.
    Mcp {
        /// Server identifier (e.g., "github", "filesystem")
        server: String,
    },

    /// Claude Agent Skill
    /// Instruction-injection skills from ~/.config/aether/skills/
    Skill {
        /// Skill directory ID (e.g., "refine-text")
        id: String,
    },

    /// User-defined custom command from config.toml
    /// These are [[rules]] entries with ^/ prefix patterns.
    Custom {
        /// Index in the rules array for reference
        rule_index: usize,
    },
}

impl ToolSource {
    /// Get a short type label for UI display
    pub fn label(&self) -> &'static str {
        match self {
            ToolSource::Native => "Native",
            ToolSource::Mcp { .. } => "MCP",
            ToolSource::Skill { .. } => "Skill",
            ToolSource::Custom { .. } => "Custom",
        }
    }

    /// Get an icon hint for UI (SF Symbol name suggestion)
    pub fn icon_hint(&self) -> &'static str {
        match self {
            ToolSource::Native => "star.fill",
            ToolSource::Mcp { .. } => "bolt.fill",
            ToolSource::Skill { .. } => "lightbulb.fill",
            ToolSource::Custom { .. } => "command",
        }
    }
}

/// Unified tool representation
///
/// All tools (Native, MCP, Skills, Custom) are normalized to this structure
/// for consistent handling across routing, UI display, and prompt generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedTool {
    /// Unique identifier
    /// Format: "{source_type}:{name}" (e.g., "native:search", "mcp:github:git_status")
    pub id: String,

    /// Command/tool name for invocation
    /// This is the name used in slash commands or LLM tool calls.
    pub name: String,

    /// Human-readable display name
    /// May include formatting for UI presentation.
    pub display_name: String,

    /// Tool description
    /// Used for both human display and LLM prompt generation.
    pub description: String,

    /// Tool origin source
    pub source: ToolSource,

    /// JSON Schema for input parameters (optional)
    /// MCP tools provide this; Native tools may define manually.
    /// Format follows JSON Schema Draft 7.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters_schema: Option<Value>,

    /// Whether this tool is currently active/enabled
    /// Disabled tools are excluded from routing and prompt generation.
    pub is_active: bool,

    /// Whether this tool requires user confirmation before execution
    /// Tools with destructive operations should set this to true.
    pub requires_confirmation: bool,

    /// Parent service name (for MCP sub-tools)
    /// e.g., "fs" for "fs:read_file"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
}

impl UnifiedTool {
    /// Create a new UnifiedTool with required fields
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        source: ToolSource,
    ) -> Self {
        let name = name.into();
        let display_name = name.clone();
        Self {
            id: id.into(),
            name,
            display_name,
            description: description.into(),
            source,
            parameters_schema: None,
            is_active: true,
            requires_confirmation: false,
            service_name: None,
        }
    }

    /// Builder method: set display name
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    /// Builder method: set parameters schema
    pub fn with_parameters_schema(mut self, schema: Value) -> Self {
        self.parameters_schema = Some(schema);
        self
    }

    /// Builder method: set requires confirmation
    pub fn with_requires_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Builder method: set service name
    pub fn with_service_name(mut self, service: impl Into<String>) -> Self {
        self.service_name = Some(service.into());
        self
    }

    /// Builder method: set active state
    pub fn with_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    /// Format tool for LLM prompt inclusion
    ///
    /// Returns a markdown-formatted line for system prompt injection.
    pub fn to_prompt_line(&self) -> String {
        let source_badge = match &self.source {
            ToolSource::Native => String::new(),
            ToolSource::Mcp { server } => format!(" [MCP:{}]", server),
            ToolSource::Skill { id } => format!(" [Skill:{}]", id),
            ToolSource::Custom { .. } => " [Custom]".to_string(),
        };

        let params = match &self.parameters_schema {
            Some(schema) => {
                // Extract parameter hints from schema
                if let Some(props) = schema.get("properties") {
                    let hints: Vec<String> = props
                        .as_object()
                        .map(|obj| {
                            obj.keys()
                                .map(|k| k.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    if !hints.is_empty() {
                        format!(" (args: {})", hints.join(", "))
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            }
            None => String::new(),
        };

        format!(
            "- **{}**{}: {}{}",
            self.name, source_badge, self.description, params
        )
    }
}

/// Routing layer indicator
///
/// Tracks which routing layer produced a match, useful for
/// debugging, metrics, and determining confidence levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoutingLayer {
    /// L1: Regex pattern match (fastest, <10ms)
    /// Highest confidence (1.0) for explicit slash commands.
    L1Rule,

    /// L2: Semantic/keyword matching (200-500ms)
    /// Medium confidence based on keyword overlap.
    L2Semantic,

    /// L3: LLM-based inference (>1s)
    /// Variable confidence from model output.
    L3Inference,

    /// Default provider fallback
    /// Used when no layer matches.
    #[default]
    Default,
}

impl RoutingLayer {
    /// Get the typical latency range for this layer
    pub fn latency_hint(&self) -> &'static str {
        match self {
            RoutingLayer::L1Rule => "<10ms",
            RoutingLayer::L2Semantic => "200-500ms",
            RoutingLayer::L3Inference => ">1s",
            RoutingLayer::Default => "0ms",
        }
    }

    /// Get the default confidence for this layer
    pub fn default_confidence(&self) -> f32 {
        match self {
            RoutingLayer::L1Rule => 1.0,
            RoutingLayer::L2Semantic => 0.7,
            RoutingLayer::L3Inference => 0.5,
            RoutingLayer::Default => 0.0,
        }
    }
}

// =============================================================================
// FFI Types (UniFFI Interop)
// =============================================================================

/// Tool source type for FFI (simplified enum without associated data)
///
/// UniFFI doesn't support enums with associated data, so we use a simple
/// enum type with a separate source_id field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSourceType {
    /// Built-in native capabilities (Search, Video)
    Native,
    /// MCP server tool
    Mcp,
    /// Claude Agent Skill
    Skill,
    /// User-defined custom command
    Custom,
}

impl From<&ToolSource> for ToolSourceType {
    fn from(source: &ToolSource) -> Self {
        match source {
            ToolSource::Native => ToolSourceType::Native,
            ToolSource::Mcp { .. } => ToolSourceType::Mcp,
            ToolSource::Skill { .. } => ToolSourceType::Skill,
            ToolSource::Custom { .. } => ToolSourceType::Custom,
        }
    }
}

/// Unified tool representation for FFI
///
/// This is a simplified version of UnifiedTool for Swift/Kotlin interop.
#[derive(Debug, Clone)]
pub struct UnifiedToolInfo {
    /// Unique identifier (e.g., "native:search")
    pub id: String,
    /// Command/tool name for invocation
    pub name: String,
    /// Human-readable display name
    pub display_name: String,
    /// Tool description
    pub description: String,
    /// Tool source type
    pub source_type: ToolSourceType,
    /// Source-specific ID (server for MCP, skill ID for Skill)
    pub source_id: Option<String>,
    /// JSON Schema string for input parameters
    pub parameters_schema: Option<String>,
    /// Whether tool is enabled
    pub is_active: bool,
    /// Whether requires user confirmation
    pub requires_confirmation: bool,
    /// Parent service name (for MCP sub-tools)
    pub service_name: Option<String>,
}

impl From<&UnifiedTool> for UnifiedToolInfo {
    fn from(tool: &UnifiedTool) -> Self {
        let (source_type, source_id) = match &tool.source {
            ToolSource::Native => (ToolSourceType::Native, None),
            ToolSource::Mcp { server } => (ToolSourceType::Mcp, Some(server.clone())),
            ToolSource::Skill { id } => (ToolSourceType::Skill, Some(id.clone())),
            ToolSource::Custom { rule_index } => {
                (ToolSourceType::Custom, Some(rule_index.to_string()))
            }
        };

        let parameters_schema = tool
            .parameters_schema
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());

        Self {
            id: tool.id.clone(),
            name: tool.name.clone(),
            display_name: tool.display_name.clone(),
            description: tool.description.clone(),
            source_type,
            source_id,
            parameters_schema,
            is_active: tool.is_active,
            requires_confirmation: tool.requires_confirmation,
            service_name: tool.service_name.clone(),
        }
    }
}

impl From<UnifiedTool> for UnifiedToolInfo {
    fn from(tool: UnifiedTool) -> Self {
        UnifiedToolInfo::from(&tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_source_label() {
        assert_eq!(ToolSource::Native.label(), "Native");
        assert_eq!(
            ToolSource::Mcp {
                server: "test".into()
            }
            .label(),
            "MCP"
        );
        assert_eq!(
            ToolSource::Skill { id: "test".into() }.label(),
            "Skill"
        );
        assert_eq!(ToolSource::Custom { rule_index: 0 }.label(), "Custom");
    }

    #[test]
    fn test_unified_tool_builder() {
        let tool = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web for information",
            ToolSource::Native,
        )
        .with_display_name("Web Search")
        .with_parameters_schema(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer" }
            }
        }))
        .with_requires_confirmation(false);

        assert_eq!(tool.id, "native:search");
        assert_eq!(tool.name, "search");
        assert_eq!(tool.display_name, "Web Search");
        assert!(tool.parameters_schema.is_some());
        assert!(tool.is_active);
    }

    #[test]
    fn test_tool_to_prompt_line() {
        let tool = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web",
            ToolSource::Native,
        )
        .with_parameters_schema(json!({
            "properties": {
                "query": {},
                "limit": {}
            }
        }));

        let line = tool.to_prompt_line();
        assert!(line.contains("**search**"));
        assert!(line.contains("Search the web"));
        assert!(line.contains("query"));
    }

    #[test]
    fn test_tool_source_mcp_prompt_line() {
        let tool = UnifiedTool::new(
            "mcp:github:git_status",
            "git_status",
            "Get git repository status",
            ToolSource::Mcp {
                server: "github".into(),
            },
        );

        let line = tool.to_prompt_line();
        assert!(line.contains("[MCP:github]"));
    }

    #[test]
    fn test_routing_layer_defaults() {
        assert_eq!(RoutingLayer::L1Rule.default_confidence(), 1.0);
        assert_eq!(RoutingLayer::L2Semantic.default_confidence(), 0.7);
        assert_eq!(RoutingLayer::L3Inference.default_confidence(), 0.5);
        assert_eq!(RoutingLayer::Default.default_confidence(), 0.0);
    }

    #[test]
    fn test_tool_source_serialization() {
        let native = ToolSource::Native;
        let json = serde_json::to_string(&native).unwrap();
        assert!(json.contains("Native"));

        let mcp = ToolSource::Mcp {
            server: "test".into(),
        };
        let json = serde_json::to_string(&mcp).unwrap();
        assert!(json.contains("Mcp"));
        assert!(json.contains("test"));
    }
}
