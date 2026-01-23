//! Unified Tool Representation
//!
//! The core UnifiedTool struct that normalizes all tools (Native, MCP, Skills,
//! Custom, Builtin) for consistent handling across routing, UI display, and
//! prompt generation.

use super::category::ToolCategory;
use super::conflict::ToolSource;
use super::definition::{Capability, StructuredToolMeta, ToolDefinition, ToolDiff};
use super::index::{truncate_string, ToolIndexCategory, ToolIndexEntry};
use super::safety::ToolSafetyLevel;
use crate::config::ToolSafetyPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    /// Tool safety level for plan confirmation and rollback behavior
    ///
    /// Determines whether confirmation is required and if rollback is possible.
    #[serde(default)]
    pub safety_level: ToolSafetyLevel,

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
    /// True for system builtin commands (/search, /youtube, /webfetch)
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

    // =========================================================================
    // Structured Tool Description Fields (for LLM tool selection)
    // =========================================================================
    /// Structured metadata for enhanced tool descriptions
    ///
    /// Contains precise capability enumeration, differentiation from similar tools,
    /// and usage guidance to help LLM make accurate tool selection decisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_meta: Option<StructuredToolMeta>,
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
            safety_level: ToolSafetyLevel::default(),
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
            // Structured description defaults
            structured_meta: None,
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

    // =========================================================================
    // Basic Builder Methods
    // =========================================================================

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

    /// Builder method: set safety level
    pub fn with_safety_level(mut self, level: ToolSafetyLevel) -> Self {
        self.safety_level = level;
        // Sync requires_confirmation with safety level
        self.requires_confirmation = level.requires_confirmation();
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

    // =========================================================================
    // UI Metadata Builder Methods
    // =========================================================================

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
    // Structured Tool Description Builder Methods (for LLM tool selection)
    // =========================================================================

    /// Builder method: add a capability
    ///
    /// Capabilities describe what this tool can do precisely.
    /// Multiple capabilities can be added for tools with diverse functions.
    pub fn with_capability(mut self, capability: Capability) -> Self {
        let meta = self
            .structured_meta
            .get_or_insert_with(StructuredToolMeta::default);
        meta.capabilities.push(capability);
        self
    }

    /// Builder method: add a not-suitable-for scenario
    ///
    /// Explicitly states when NOT to use this tool, helping prevent misuse.
    pub fn with_not_suitable_for(mut self, scenario: impl Into<String>) -> Self {
        let meta = self
            .structured_meta
            .get_or_insert_with(StructuredToolMeta::default);
        meta.not_suitable_for.push(scenario.into());
        self
    }

    /// Builder method: add a differentiation from another tool
    ///
    /// Helps LLM distinguish this tool from similar tools.
    pub fn with_differentiation(mut self, diff: ToolDiff) -> Self {
        let meta = self
            .structured_meta
            .get_or_insert_with(StructuredToolMeta::default);
        meta.differentiation.push(diff);
        self
    }

    /// Builder method: add a use-when scenario
    ///
    /// Describes typical use cases (positive examples) for this tool.
    pub fn with_use_when(mut self, scenario: impl Into<String>) -> Self {
        let meta = self
            .structured_meta
            .get_or_insert_with(StructuredToolMeta::default);
        meta.use_when.push(scenario.into());
        self
    }

    // =========================================================================
    // Tool Index Methods (Smart Tool Discovery)
    // =========================================================================

    /// Convert to lightweight index entry for smart discovery
    ///
    /// Creates a minimal representation suitable for LLM prompt injection.
    /// The summary is truncated to 50 characters for token efficiency.
    ///
    /// # Arguments
    ///
    /// * `core_tools` - List of tool names that should be marked as core
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let tool = UnifiedTool::new(...);
    /// let entry = tool.to_index_entry(&["search", "file_ops"]);
    /// ```
    pub fn to_index_entry(&self, core_tools: &[&str]) -> ToolIndexEntry {
        let category = ToolIndexCategory::from(&self.source);
        let summary = truncate_string(&self.description, 50);

        // Extract keywords from name and description
        let mut keywords = Vec::new();

        // Add name parts as keywords
        for part in self.name.split([':', '_', '-']) {
            if part.len() > 2 {
                keywords.push(part.to_lowercase());
            }
        }

        // Check if this is a core tool
        let is_core = core_tools.contains(&self.name.as_str());

        ToolIndexEntry {
            name: self.name.clone(),
            category: if is_core {
                ToolIndexCategory::Core
            } else {
                category
            },
            summary,
            keywords,
            is_core,
        }
    }

    // =========================================================================
    // Conversion from AgentTool Types
    // =========================================================================

    /// Create UnifiedTool from ToolDefinition (AgentTool interface)
    ///
    /// Converts `AgentTool` definitions to `UnifiedTool` for unified
    /// registry management. The source is automatically determined from
    /// the tool's category:
    /// - `ToolCategory::Native` → `ToolSource::Native`
    /// - `ToolCategory::Builtin` → `ToolSource::Builtin`
    /// - `ToolCategory::Mcp` → `ToolSource::Mcp { server: service_name }`
    /// - `ToolCategory::Skills` → `ToolSource::Skill { id: tool_name }`
    /// - `ToolCategory::Custom` → `ToolSource::Custom { rule_index: 0 }`
    ///
    /// # Arguments
    ///
    /// * `def` - The tool definition from an AgentTool implementation
    /// * `service_name` - Optional service grouping name. For MCP tools, this
    ///   should be the actual MCP server name (e.g., "github", "filesystem").
    ///   For native tools, this is a grouping name (e.g., "filesystem", "git").
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Native tool
    /// let tool = FileReadTool::new(ctx);
    /// let unified = UnifiedTool::from_tool_definition(tool.definition(), Some("filesystem"));
    ///
    /// // MCP tool
    /// let mcp_bridge = McpToolBridge::new(tool_def, client, "github".to_string());
    /// let unified = UnifiedTool::from_tool_definition(mcp_bridge.definition(), Some("github"));
    /// ```
    #[allow(deprecated)] // ToolCategory::Native is deprecated but still needed for compatibility
    pub fn from_tool_definition(def: ToolDefinition, service_name: Option<&str>) -> Self {
        // Determine ToolSource from ToolCategory
        let (source, id) = match def.category {
            ToolCategory::Builtin => {
                let id = format!("builtin:{}", def.name);
                (ToolSource::Builtin, id)
            }
            ToolCategory::Native => {
                let id = match service_name {
                    Some(svc) => format!("native:{}:{}", svc, def.name),
                    None => format!("native:{}", def.name),
                };
                (ToolSource::Native, id)
            }
            ToolCategory::Mcp => {
                let server = service_name.unwrap_or("unknown").to_string();
                let id = format!("mcp:{}:{}", server, def.name);
                (ToolSource::Mcp { server }, id)
            }
            ToolCategory::Skills => {
                let skill_id = service_name.unwrap_or(&def.name).to_string();
                let id = format!("skill:{}", skill_id);
                (
                    ToolSource::Skill {
                        id: skill_id.clone(),
                    },
                    id,
                )
            }
            ToolCategory::Custom => {
                let id = format!("custom:{}", def.name);
                (ToolSource::Custom { rule_index: 0 }, id)
            }
        };

        let icon = Self::icon_for_category(def.category);
        // Use default policy for safety level inference (policy can be injected via Config)
        let safety_level = Self::infer_safety_level(&def.name, def.category, None);

        // Determine intent type based on source
        let intent_type = match &source {
            ToolSource::Builtin => format!("builtin:{}", def.name),
            ToolSource::Native => format!("native:{}", def.name),
            ToolSource::Mcp { server } => format!("mcp:{}:{}", server, def.name),
            ToolSource::Skill { id } => format!("skill:{}", id),
            ToolSource::Custom { .. } => format!("custom:{}", def.name),
        };

        let mut tool = Self::new(&id, &def.name, &def.description, source)
            .with_display_name(&def.name)
            .with_parameters_schema(def.parameters.clone())
            .with_requires_confirmation(def.requires_confirmation)
            .with_safety_level(safety_level)
            .with_icon(icon)
            .with_usage(format!("/{} [args]", def.name))
            // Generate routing regex for flat namespace
            .with_routing_regex(format!(r"^/{}\s*", regex::escape(&def.name)))
            .with_routing_intent_type(intent_type)
            .with_routing_strip_prefix(true);

        if let Some(svc) = service_name {
            tool = tool.with_service_name(svc);
        }

        tool
    }

    /// Get icon for a tool category
    fn icon_for_category(category: ToolCategory) -> &'static str {
        // Delegate to ToolCategory's built-in icon method
        category.icon()
    }

    /// Infer safety level from tool name and category
    ///
    /// Uses heuristics based on common tool naming patterns:
    /// - Read-only: search, query, get, read, list, show, view
    /// - Reversible: create, copy, move, rename, update, set
    /// - Irreversible Low Risk: send, notify, post, publish
    /// - Irreversible High Risk: delete, remove, drop, execute, run, shell
    ///
    /// If a `ToolSafetyPolicy` is provided, uses configurable keywords from policy.
    /// Otherwise, uses hardcoded defaults for backward compatibility.
    #[allow(deprecated)] // ToolCategory::Native is deprecated but still needed for compatibility
    pub fn infer_safety_level(
        name: &str,
        category: ToolCategory,
        policy: Option<&ToolSafetyPolicy>,
    ) -> ToolSafetyLevel {
        // Use provided policy or default
        let default_policy = ToolSafetyPolicy::default();
        let policy = policy.unwrap_or(&default_policy);

        // Check keyword-based classification using policy
        if policy.is_high_risk(name) {
            return ToolSafetyLevel::IrreversibleHighRisk;
        }

        if policy.is_low_risk(name) {
            return ToolSafetyLevel::IrreversibleLowRisk;
        }

        if policy.is_reversible(name) {
            return ToolSafetyLevel::Reversible;
        }

        if policy.is_readonly(name) {
            return ToolSafetyLevel::ReadOnly;
        }

        // Fall back to category-based inference using policy fallbacks
        let fallback_str = match category {
            ToolCategory::Builtin => &policy.builtin_fallback,
            ToolCategory::Native => &policy.native_fallback,
            ToolCategory::Skills => &policy.skill_fallback,
            ToolCategory::Mcp => &policy.mcp_fallback,
            ToolCategory::Custom => &policy.custom_fallback,
        };

        // Convert fallback string to ToolSafetyLevel
        Self::parse_safety_level_str(policy.parse_safety_level(fallback_str))
    }

    /// Parse safety level string to enum
    fn parse_safety_level_str(level: &str) -> ToolSafetyLevel {
        match level {
            "readonly" => ToolSafetyLevel::ReadOnly,
            "reversible" => ToolSafetyLevel::Reversible,
            "irreversible_low_risk" => ToolSafetyLevel::IrreversibleLowRisk,
            "irreversible_high_risk" => ToolSafetyLevel::IrreversibleHighRisk,
            _ => ToolSafetyLevel::IrreversibleLowRisk, // Default fallback
        }
    }

    /// Format tool for LLM prompt inclusion
    ///
    /// Returns a markdown-formatted line for system prompt injection.
    /// Builtin and Native tools are marked as "Preferred" to guide L3 routing priority.
    pub fn to_prompt_line(&self) -> String {
        let source_badge = match &self.source {
            ToolSource::Native => " [Native - Preferred]".to_string(),
            ToolSource::Builtin => " [Builtin - Preferred]".to_string(),
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
                        .map(|obj| obj.keys().cloned().collect())
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        let tool = UnifiedTool::new("native:search", "search", "Search the web", ToolSource::Native)
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
    fn test_unified_tool_with_safety_level() {
        let tool = UnifiedTool::new(
            "native:delete_file",
            "delete_file",
            "Delete a file",
            ToolSource::Native,
        )
        .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);

        assert_eq!(tool.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
        assert!(tool.requires_confirmation); // Auto-synced from safety level
    }

    #[test]
    fn test_unified_tool_with_structured_meta() {
        let tool = UnifiedTool::new(
            "builtin:search_files",
            "search_files",
            "Search for files by name pattern",
            ToolSource::Builtin,
        )
        .with_capability(Capability::new(
            "search",
            "file names",
            "project",
            "file paths",
        ))
        .with_not_suitable_for("searching file content")
        .with_differentiation(ToolDiff::new(
            "search_content",
            "matches names",
            "matches content",
            "know file name",
            "know content",
        ))
        .with_use_when("user mentions specific file name");

        assert!(tool.structured_meta.is_some());
        let meta = tool.structured_meta.unwrap();
        assert_eq!(meta.capabilities.len(), 1);
        assert_eq!(meta.not_suitable_for.len(), 1);
        assert_eq!(meta.differentiation.len(), 1);
        assert_eq!(meta.use_when.len(), 1);
    }

    #[test]
    #[allow(deprecated)] // Testing legacy ToolCategory::Native behavior
    fn test_infer_safety_level_high_risk() {
        assert_eq!(
            UnifiedTool::infer_safety_level("delete_file", ToolCategory::Native, None),
            ToolSafetyLevel::IrreversibleHighRisk
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("shell_execute", ToolCategory::Native, None),
            ToolSafetyLevel::IrreversibleHighRisk
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("run_bash_command", ToolCategory::Native, None),
            ToolSafetyLevel::IrreversibleHighRisk
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("remove_directory", ToolCategory::Native, None),
            ToolSafetyLevel::IrreversibleHighRisk
        );
    }

    #[test]
    #[allow(deprecated)] // Testing legacy ToolCategory::Native behavior
    fn test_infer_safety_level_low_risk() {
        assert_eq!(
            UnifiedTool::infer_safety_level("send_notification", ToolCategory::Builtin, None),
            ToolSafetyLevel::IrreversibleLowRisk
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("post_message", ToolCategory::Mcp, None),
            ToolSafetyLevel::IrreversibleLowRisk
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("git_push", ToolCategory::Native, None),
            ToolSafetyLevel::IrreversibleLowRisk
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("commit_changes", ToolCategory::Native, None),
            ToolSafetyLevel::IrreversibleLowRisk
        );
    }

    #[test]
    #[allow(deprecated)] // Testing legacy ToolCategory::Native behavior
    fn test_infer_safety_level_reversible() {
        assert_eq!(
            UnifiedTool::infer_safety_level("create_file", ToolCategory::Native, None),
            ToolSafetyLevel::Reversible
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("copy_file", ToolCategory::Native, None),
            ToolSafetyLevel::Reversible
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("write_text", ToolCategory::Native, None),
            ToolSafetyLevel::Reversible
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("update_config", ToolCategory::Builtin, None),
            ToolSafetyLevel::Reversible
        );
    }

    #[test]
    #[allow(deprecated)] // Testing legacy ToolCategory::Native behavior
    fn test_infer_safety_level_readonly() {
        assert_eq!(
            UnifiedTool::infer_safety_level("search_web", ToolCategory::Native, None),
            ToolSafetyLevel::ReadOnly
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("read_file", ToolCategory::Native, None),
            ToolSafetyLevel::ReadOnly
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("list_files", ToolCategory::Native, None),
            ToolSafetyLevel::ReadOnly
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("translate_text", ToolCategory::Native, None),
            ToolSafetyLevel::ReadOnly
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("summarize_document", ToolCategory::Native, None),
            ToolSafetyLevel::ReadOnly
        );
    }

    #[test]
    #[allow(deprecated)] // Testing legacy ToolCategory::Native behavior
    fn test_infer_safety_level_category_fallback() {
        // Unknown tool names should fall back to category-based inference
        assert_eq!(
            UnifiedTool::infer_safety_level("xyz_unknown", ToolCategory::Builtin, None),
            ToolSafetyLevel::ReadOnly
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("xyz_unknown", ToolCategory::Native, None),
            ToolSafetyLevel::Reversible
        );
        assert_eq!(
            UnifiedTool::infer_safety_level("xyz_unknown", ToolCategory::Mcp, None),
            ToolSafetyLevel::IrreversibleLowRisk
        );
    }

    #[test]
    fn test_unified_tool_to_index_entry() {
        let tool = UnifiedTool::new(
            "mcp:github:pr_list",
            "github:pr_list",
            "List all pull requests from a GitHub repository",
            ToolSource::Mcp {
                server: "github".into(),
            },
        );

        let entry = tool.to_index_entry(&["search", "file_ops"]);
        assert_eq!(entry.name, "github:pr_list");
        assert_eq!(entry.category, ToolIndexCategory::Mcp);
        assert!(!entry.is_core);
        // Summary is truncated
        assert!(entry.summary.len() <= 50);
    }

    #[test]
    fn test_unified_tool_to_index_entry_core() {
        let tool = UnifiedTool::new(
            "builtin:search",
            "search",
            "Search the web",
            ToolSource::Builtin,
        );

        let entry = tool.to_index_entry(&["search", "file_ops"]);
        assert_eq!(entry.name, "search");
        assert_eq!(entry.category, ToolIndexCategory::Core);
        assert!(entry.is_core);
    }
}
