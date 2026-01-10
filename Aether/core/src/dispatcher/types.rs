//! Dispatcher Type Definitions
//!
//! Core data structures for the Dispatcher Layer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{ToolCategory, ToolDefinition};

// =============================================================================
// Conflict Resolution System (Flat Namespace)
// =============================================================================

/// Tool priority for conflict resolution
///
/// When multiple tools have the same name, the higher priority tool wins
/// and the lower priority tool is renamed with a suffix.
///
/// Priority order (highest to lowest):
/// 1. Builtin (5) - System commands like /search, /video, /chat
/// 2. Native (4) - System capabilities implementations
/// 3. Custom (3) - User-defined rules from config.toml
/// 4. Mcp (2) - External MCP server tools
/// 5. Skill (1) - Claude Agent skills
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolPriority {
    /// Lowest priority - Claude Agent skills
    Skill = 1,
    /// External MCP server tools
    Mcp = 2,
    /// User-defined custom rules
    Custom = 3,
    /// System native capabilities
    Native = 4,
    /// Highest priority - System builtin commands
    Builtin = 5,
}

/// Information about an existing tool that conflicts with a new registration
#[derive(Debug, Clone)]
pub struct ConflictInfo {
    /// ID of the existing tool
    pub existing_id: String,
    /// Name of the existing tool
    pub existing_name: String,
    /// Source of the existing tool
    pub existing_source: ToolSource,
    /// Priority of the existing tool
    pub existing_priority: ToolPriority,
}

/// Resolution strategy for naming conflicts
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Rename the existing tool (new tool has higher priority)
    RenameExisting {
        /// Original name before renaming
        original_name: String,
        /// New name after renaming (with suffix)
        new_name: String,
    },
    /// Rename the new tool (existing tool has higher priority)
    RenameNew {
        /// Original name before renaming
        original_name: String,
        /// New name after renaming (with suffix)
        new_name: String,
    },
    /// No conflict - tool can be registered with original name
    NoConflict,
}

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

    /// System builtin commands (/search, /mcp, /skill, /video, /chat)
    /// These are always-available slash commands that may or may not have
    /// special capability execution logic.
    Builtin,

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
            ToolSource::Builtin => "Builtin",
            ToolSource::Mcp { .. } => "MCP",
            ToolSource::Skill { .. } => "Skill",
            ToolSource::Custom { .. } => "Custom",
        }
    }

    /// Get an icon hint for UI (SF Symbol name suggestion)
    pub fn icon_hint(&self) -> &'static str {
        match self {
            ToolSource::Native => "star.fill",
            ToolSource::Builtin => "command.circle.fill",
            ToolSource::Mcp { .. } => "bolt.fill",
            ToolSource::Skill { .. } => "lightbulb.fill",
            ToolSource::Custom { .. } => "command",
        }
    }

    /// Get the priority level for conflict resolution
    ///
    /// Higher priority tools win name conflicts and lower priority tools
    /// are renamed with a suffix.
    pub fn priority(&self) -> ToolPriority {
        match self {
            ToolSource::Builtin => ToolPriority::Builtin,
            ToolSource::Native => ToolPriority::Native,
            ToolSource::Custom { .. } => ToolPriority::Custom,
            ToolSource::Mcp { .. } => ToolPriority::Mcp,
            ToolSource::Skill { .. } => ToolPriority::Skill,
        }
    }

    /// Get the suffix used when renaming a conflicting tool
    ///
    /// When a tool loses a name conflict, it's renamed to `{name}-{suffix}`.
    /// For example, an MCP tool named "search" becomes "search-mcp".
    pub fn suffix(&self) -> &'static str {
        match self {
            ToolSource::Builtin => "system",
            ToolSource::Native => "native",
            ToolSource::Custom { .. } => "custom",
            ToolSource::Mcp { .. } => "mcp",
            ToolSource::Skill { .. } => "skill",
        }
    }

    /// Check if this source is a builtin command
    pub fn is_builtin(&self) -> bool {
        matches!(self, ToolSource::Builtin)
    }

    /// Check if this source is an MCP tool
    pub fn is_mcp(&self) -> bool {
        matches!(self, ToolSource::Mcp { .. })
    }

    /// Check if this source is a skill
    pub fn is_skill(&self) -> bool {
        matches!(self, ToolSource::Skill { .. })
    }
}

/// Unified tool representation
///
/// All tools (Native, MCP, Skills, Custom, Builtin) are normalized to this structure
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

    // =========================================================================
    // UI Metadata Fields (for Settings UI and Command Completion)
    // =========================================================================

    /// SF Symbol icon name for UI display
    /// e.g., "magnifyingglass", "puzzlepiece.extension"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

    /// Usage example for documentation
    /// e.g., "/search <query>", "/mcp <tool> [params]"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,

    /// IDs of nested subtools (for namespace commands like /mcp, /skill)
    /// Empty for leaf commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtools: Vec<String>,

    /// Localization key for i18n lookup
    /// e.g., "tool.search" maps to "tool.search.hint", "tool.search.description"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localization_key: Option<String>,

    /// Quick check for builtin status
    /// True for system builtin commands (/search, /mcp, /skill, /video, /chat)
    #[serde(default)]
    pub is_builtin: bool,

    /// Display sort order (lower = first)
    /// Used to order commands in completion and UI lists.
    #[serde(default)]
    pub sort_order: i32,

    /// Whether this tool has dynamic subtools
    /// True for /mcp (has MCP server tools) and /skill (has installed skills)
    #[serde(default)]
    pub has_subtools: bool,

    // =========================================================================
    // Routing Configuration Fields (for builtin commands)
    // =========================================================================
    // These fields are only populated for builtin tools and define how
    // requests matching this command are routed and processed.

    /// Regex pattern for L1 routing match
    /// e.g., "^/search\\s+" for /search command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_regex: Option<String>,

    /// System prompt to inject for this command
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_system_prompt: Option<String>,

    /// Capabilities to enable for this command
    /// e.g., ["search"], ["memory", "skills"]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routing_capabilities: Vec<String>,

    /// Intent type for classification
    /// e.g., "builtin_search", "general_chat"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_intent_type: Option<String>,

    /// Whether to strip the command prefix from user input
    #[serde(default)]
    pub routing_strip_prefix: bool,

    /// Context format for prompt assembly
    /// Default: "markdown"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_context_format: Option<String>,

    // =========================================================================
    // Conflict Resolution Fields (Flat Namespace)
    // =========================================================================

    /// Original name before conflict resolution renaming
    ///
    /// If this tool was renamed due to a conflict, this field stores the
    /// original name. For example, if an MCP tool "search" was renamed to
    /// "search-mcp" because it conflicts with the builtin /search, this
    /// field would be "search".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,

    /// Whether this tool was renamed due to a conflict
    #[serde(default)]
    pub was_renamed: bool,
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
        let is_builtin = matches!(source, ToolSource::Builtin);
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
            // UI metadata defaults
            icon: None,
            usage: None,
            subtools: Vec::new(),
            localization_key: None,
            is_builtin,
            sort_order: 100, // Default sort order (user commands come after builtins)
            has_subtools: false,
            // Routing config defaults (only set for builtins)
            routing_regex: None,
            routing_system_prompt: None,
            routing_capabilities: Vec::new(),
            routing_intent_type: None,
            routing_strip_prefix: false,
            routing_context_format: None,
            // Conflict resolution defaults
            original_name: None,
            was_renamed: false,
        }
    }

    /// Create a builtin tool with standard prefix
    ///
    /// Convenience constructor for system builtin commands.
    /// ID is automatically prefixed with "builtin:".
    pub fn builtin(name: impl Into<String>) -> Self {
        let name = name.into();
        let id = format!("builtin:{}", name);
        Self::new(id, name, "", ToolSource::Builtin).with_builtin(true)
    }

    /// Builder method: set display name
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    /// Builder method: set description
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
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

    /// Builder method: set icon (SF Symbol name)
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Builder method: set usage example
    pub fn with_usage(mut self, usage: impl Into<String>) -> Self {
        self.usage = Some(usage.into());
        self
    }

    /// Builder method: set localization key
    pub fn with_localization_key(mut self, key: impl Into<String>) -> Self {
        self.localization_key = Some(key.into());
        self
    }

    /// Builder method: set builtin flag
    pub fn with_builtin(mut self, is_builtin: bool) -> Self {
        self.is_builtin = is_builtin;
        self
    }

    /// Builder method: set sort order
    pub fn with_sort_order(mut self, order: i32) -> Self {
        self.sort_order = order;
        self
    }

    /// Builder method: set has_subtools flag
    pub fn with_has_subtools(mut self, has: bool) -> Self {
        self.has_subtools = has;
        self
    }

    /// Builder method: add a subtool ID
    pub fn with_subtool(mut self, subtool_id: impl Into<String>) -> Self {
        self.subtools.push(subtool_id.into());
        self
    }

    // =========================================================================
    // Routing Config Builder Methods (for builtin commands)
    // =========================================================================

    /// Builder method: set routing regex pattern
    pub fn with_routing_regex(mut self, regex: impl Into<String>) -> Self {
        self.routing_regex = Some(regex.into());
        self
    }

    /// Builder method: set routing system prompt
    pub fn with_routing_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.routing_system_prompt = Some(prompt.into());
        self
    }

    /// Builder method: set routing capabilities
    pub fn with_routing_capabilities(mut self, caps: Vec<String>) -> Self {
        self.routing_capabilities = caps;
        self
    }

    /// Builder method: set routing intent type
    pub fn with_routing_intent_type(mut self, intent: impl Into<String>) -> Self {
        self.routing_intent_type = Some(intent.into());
        self
    }

    /// Builder method: set routing strip prefix
    pub fn with_routing_strip_prefix(mut self, strip: bool) -> Self {
        self.routing_strip_prefix = strip;
        self
    }

    /// Builder method: set routing context format
    pub fn with_routing_context_format(mut self, format: impl Into<String>) -> Self {
        self.routing_context_format = Some(format.into());
        self
    }

    // =========================================================================
    // Conflict Resolution Builder Methods
    // =========================================================================

    /// Builder method: set original name (before conflict resolution renaming)
    pub fn with_original_name(mut self, name: impl Into<String>) -> Self {
        self.original_name = Some(name.into());
        self.was_renamed = true;
        self
    }

    /// Builder method: mark as renamed due to conflict
    pub fn with_was_renamed(mut self, renamed: bool) -> Self {
        self.was_renamed = renamed;
        self
    }

    // =========================================================================
    // Conversion from AgentTool Types
    // =========================================================================

    /// Create UnifiedTool from ToolDefinition (AgentTool interface)
    ///
    /// Converts native `AgentTool` definitions to `UnifiedTool` for unified
    /// registry management. The source is set to `Native` and appropriate
    /// defaults are applied based on the tool category.
    ///
    /// # Arguments
    ///
    /// * `def` - The tool definition from an AgentTool implementation
    /// * `service_name` - Optional service grouping name (e.g., "filesystem", "git")
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let tool = FileReadTool::new(ctx);
    /// let unified = UnifiedTool::from_tool_definition(tool.definition(), Some("filesystem"));
    /// ```
    pub fn from_tool_definition(def: ToolDefinition, service_name: Option<&str>) -> Self {
        let id = match service_name {
            Some(svc) => format!("native:{}:{}", svc, def.name),
            None => format!("native:{}", def.name),
        };

        let icon = Self::icon_for_category(def.category);

        let mut tool = Self::new(&id, &def.name, &def.description, ToolSource::Native)
            .with_display_name(&def.name)
            .with_parameters_schema(def.parameters.clone())
            .with_requires_confirmation(def.requires_confirmation)
            .with_icon(icon)
            .with_usage(format!("/{} [args]", def.name))
            // Generate routing regex for flat namespace
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(&def.name)))
            .with_routing_intent_type(format!("native:{}", def.name))
            .with_routing_strip_prefix(true);

        if let Some(svc) = service_name {
            tool = tool.with_service_name(svc);
        }

        tool
    }

    /// Get icon for a tool category
    fn icon_for_category(category: ToolCategory) -> &'static str {
        match category {
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

    /// Format tool for LLM prompt inclusion
    ///
    /// Returns a markdown-formatted line for system prompt injection.
    pub fn to_prompt_line(&self) -> String {
        let source_badge = match &self.source {
            ToolSource::Native => String::new(),
            ToolSource::Builtin => " [Builtin]".to_string(),
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
    /// System builtin commands (/search, /mcp, /skill, /video, /chat)
    Builtin,
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
            ToolSource::Builtin => ToolSourceType::Builtin,
            ToolSource::Mcp { .. } => ToolSourceType::Mcp,
            ToolSource::Skill { .. } => ToolSourceType::Skill,
            ToolSource::Custom { .. } => ToolSourceType::Custom,
        }
    }
}

impl ToolSourceType {
    /// Get default SF Symbol icon for this source type
    ///
    /// Used for UI display in command completion and settings.
    pub fn default_icon(&self) -> &'static str {
        match self {
            ToolSourceType::Native | ToolSourceType::Builtin => "command.circle.fill",
            ToolSourceType::Mcp => "bolt.fill",
            ToolSourceType::Skill => "lightbulb.fill",
            ToolSourceType::Custom => "command",
        }
    }

    /// Get badge label for this source type
    pub fn badge_label(&self) -> &'static str {
        match self {
            ToolSourceType::Native | ToolSourceType::Builtin => "System",
            ToolSourceType::Mcp => "MCP",
            ToolSourceType::Skill => "Skill",
            ToolSourceType::Custom => "Custom",
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

    // UI Metadata
    /// SF Symbol icon name
    pub icon: Option<String>,
    /// Usage example
    pub usage: Option<String>,
    /// Localization key for i18n
    pub localization_key: Option<String>,
    /// Whether this is a system builtin command
    pub is_builtin: bool,
    /// Display sort order
    pub sort_order: i32,
    /// Whether has dynamic subtools
    pub has_subtools: bool,
}

impl From<&UnifiedTool> for UnifiedToolInfo {
    fn from(tool: &UnifiedTool) -> Self {
        let (source_type, source_id) = match &tool.source {
            ToolSource::Native => (ToolSourceType::Native, None),
            ToolSource::Builtin => (ToolSourceType::Builtin, None),
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
            // UI metadata
            icon: tool.icon.clone(),
            usage: tool.usage.clone(),
            localization_key: tool.localization_key.clone(),
            is_builtin: tool.is_builtin,
            sort_order: tool.sort_order,
            has_subtools: tool.has_subtools,
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
        assert_eq!(ToolSource::Builtin.label(), "Builtin");
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
    fn test_builtin_tool_constructor() {
        let tool = UnifiedTool::builtin("search")
            .with_display_name("Web Search")
            .with_description("Search the web")
            .with_icon("magnifyingglass")
            .with_usage("/search <query>")
            .with_localization_key("tool.search")
            .with_sort_order(1);

        assert_eq!(tool.id, "builtin:search");
        assert_eq!(tool.name, "search");
        assert_eq!(tool.display_name, "Web Search");
        assert_eq!(tool.description, "Search the web");
        assert_eq!(tool.icon, Some("magnifyingglass".to_string()));
        assert_eq!(tool.usage, Some("/search <query>".to_string()));
        assert_eq!(tool.localization_key, Some("tool.search".to_string()));
        assert_eq!(tool.sort_order, 1);
        assert!(tool.is_builtin);
        assert!(matches!(tool.source, ToolSource::Builtin));
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

    // =========================================================================
    // Conflict Resolution Tests
    // =========================================================================

    #[test]
    fn test_tool_priority_ordering() {
        // Verify priority ordering: Builtin > Native > Custom > Mcp > Skill
        assert!(ToolPriority::Builtin > ToolPriority::Native);
        assert!(ToolPriority::Native > ToolPriority::Custom);
        assert!(ToolPriority::Custom > ToolPriority::Mcp);
        assert!(ToolPriority::Mcp > ToolPriority::Skill);
    }

    #[test]
    fn test_tool_source_priority() {
        assert_eq!(ToolSource::Builtin.priority(), ToolPriority::Builtin);
        assert_eq!(ToolSource::Native.priority(), ToolPriority::Native);
        assert_eq!(
            ToolSource::Custom { rule_index: 0 }.priority(),
            ToolPriority::Custom
        );
        assert_eq!(
            ToolSource::Mcp {
                server: "test".into()
            }
            .priority(),
            ToolPriority::Mcp
        );
        assert_eq!(
            ToolSource::Skill { id: "test".into() }.priority(),
            ToolPriority::Skill
        );
    }

    #[test]
    fn test_tool_source_suffix() {
        assert_eq!(ToolSource::Builtin.suffix(), "system");
        assert_eq!(ToolSource::Native.suffix(), "native");
        assert_eq!(ToolSource::Custom { rule_index: 0 }.suffix(), "custom");
        assert_eq!(
            ToolSource::Mcp {
                server: "test".into()
            }
            .suffix(),
            "mcp"
        );
        assert_eq!(ToolSource::Skill { id: "test".into() }.suffix(), "skill");
    }

    #[test]
    fn test_tool_source_type_checks() {
        assert!(ToolSource::Builtin.is_builtin());
        assert!(!ToolSource::Native.is_builtin());

        assert!(ToolSource::Mcp {
            server: "test".into()
        }
        .is_mcp());
        assert!(!ToolSource::Builtin.is_mcp());

        assert!(ToolSource::Skill { id: "test".into() }.is_skill());
        assert!(!ToolSource::Builtin.is_skill());
    }

    #[test]
    fn test_unified_tool_with_original_name() {
        let tool = UnifiedTool::new(
            "mcp:server:search-mcp",
            "search-mcp",
            "Search via MCP",
            ToolSource::Mcp {
                server: "server".into(),
            },
        )
        .with_original_name("search");

        assert_eq!(tool.name, "search-mcp");
        assert_eq!(tool.original_name, Some("search".to_string()));
        assert!(tool.was_renamed);
    }

    #[test]
    fn test_conflict_resolution_variants() {
        let rename_existing = ConflictResolution::RenameExisting {
            original_name: "search".to_string(),
            new_name: "search-mcp".to_string(),
        };

        let rename_new = ConflictResolution::RenameNew {
            original_name: "search".to_string(),
            new_name: "search-skill".to_string(),
        };

        let no_conflict = ConflictResolution::NoConflict;

        // Verify they are distinct
        assert_ne!(rename_existing, rename_new);
        assert_ne!(rename_existing, no_conflict);
        assert_ne!(rename_new, no_conflict);
    }
}
