//! Dispatcher Type Definitions
//!
//! Core data structures for the Dispatcher Layer.
//!
//! This module contains all tool-related type definitions:
//! - Tool metadata: ToolCategory, ToolDefinition, ToolResult
//! - Safety levels: ToolSafetyLevel
//! - Source tracking: ToolSource, ToolPriority
//! - Unified representation: UnifiedTool

use crate::config::ToolSafetyPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

// =============================================================================
// Tool Category (moved from tools/traits.rs)
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
// Tool Definition (moved from tools/traits.rs)
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
// Tool Result (moved from tools/traits.rs)
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

// =============================================================================
// Tool Safety Level (moved from routing module)
// =============================================================================

/// Tool safety level for confirmation and rollback behavior
///
/// Determines whether user confirmation is required before execution
/// and whether the operation can be rolled back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ToolSafetyLevel {
    /// Read-only operations that don't modify anything
    /// No confirmation required, instant execution
    #[default]
    ReadOnly,

    /// Operations that can be undone/reversed
    /// May require confirmation based on config
    Reversible,

    /// Operations that cannot be undone but have low impact
    /// (e.g., sending a message, posting a comment)
    /// Usually requires confirmation
    IrreversibleLowRisk,

    /// Operations that cannot be undone and have high impact
    /// (e.g., deleting files, dropping tables)
    /// Always requires confirmation
    IrreversibleHighRisk,
}

impl ToolSafetyLevel {
    /// Check if this safety level requires user confirmation
    pub fn requires_confirmation(&self) -> bool {
        matches!(
            self,
            ToolSafetyLevel::IrreversibleLowRisk | ToolSafetyLevel::IrreversibleHighRisk
        )
    }

    /// Get a human-readable label for this safety level
    pub fn label(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "Read Only",
            ToolSafetyLevel::Reversible => "Reversible",
            ToolSafetyLevel::IrreversibleLowRisk => "Low Risk",
            ToolSafetyLevel::IrreversibleHighRisk => "High Risk",
        }
    }

    /// Get a badge color hint for UI (SF Symbol color name)
    pub fn color_hint(&self) -> &'static str {
        match self {
            ToolSafetyLevel::ReadOnly => "green",
            ToolSafetyLevel::Reversible => "blue",
            ToolSafetyLevel::IrreversibleLowRisk => "yellow",
            ToolSafetyLevel::IrreversibleHighRisk => "red",
        }
    }
}

// =============================================================================
// Conflict Resolution System (Flat Namespace)
// =============================================================================

/// Tool priority for conflict resolution
///
/// When multiple tools have the same name, the higher priority tool wins
/// and the lower priority tool is renamed with a suffix.
///
/// Priority order (highest to lowest):
/// 1. Builtin (5) - System commands like /search, /youtube, /webfetch
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

    /// System builtin commands (/search, /youtube, /webfetch)
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
    /// Built-in native capabilities (Search, YouTube)
    Native,
    /// System builtin commands (/search, /youtube, /webfetch)
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
    /// Safety level label (ReadOnly, Reversible, Low Risk, High Risk)
    pub safety_level: String,
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
            safety_level: tool.safety_level.label().to_string(),
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
        assert_eq!(ToolSource::Skill { id: "test".into() }.label(), "Skill");
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

    // =========================================================================
    // Safety Level Inference Tests
    // =========================================================================

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

    // =========================================================================
    // Task 6: Capability and ToolDiff Tests
    // =========================================================================

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
        assert_eq!(prompt, "search file names within project directory → list of paths");
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

    // =========================================================================
    // Task 7: StructuredToolMeta Tests
    // =========================================================================

    #[test]
    fn test_unified_tool_with_structured_meta() {
        let tool = UnifiedTool::new(
            "builtin:search_files",
            "search_files",
            "Search for files by name pattern",
            ToolSource::Builtin,
        )
        .with_capability(Capability::new("search", "file names", "project", "file paths"))
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
            capabilities: vec![Capability::new("search", "file names", "project", "file paths")],
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
