//! Unified Tool Representation
//!
//! The core `UnifiedTool` struct that normalizes all tools (Native, MCP, Skills,
//! Custom, Builtin) for consistent handling across routing, UI display, and
//! prompt generation.

mod builders;
mod conversions;

#[cfg(test)]
mod tests;

use super::conflict::ToolSource;
use super::safety::ToolSafetyLevel;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Channel types for visibility filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ChannelType {
    Panel,
    Telegram,
    Discord,
    IMessage,
    Cli,
}

/// Unified tool representation
///
/// All tools (Native, MCP, Skills, Custom, Builtin) are normalized to this structure
/// for consistent handling across routing, UI display, and prompt generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UnifiedTool {
    /// Unique identifier
    /// Format: "{`source_type}:{name`}" (e.g., "native:search", "`mcp:github:git_status`")
    pub id: String,

    /// Command/tool name for invocation
    /// This is the name used in slash commands or LLM tool calls.
    pub name: String,

    /// Alternative invocation names (aliases) for this command.
    ///
    /// e.g. `["new"]` for `session_new`, so `/new` resolves to the same tool.
    /// Aliases are matched at slash-command resolution with **lower precedence**
    /// than the canonical `name` (a canonical-name hit always wins over an
    /// alias hit), and are also scored for "did you mean?" suggestions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,

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
    /// Mutation is `pub(crate)` so the registry's lock-protected writers are
    /// the only path that flips this bit — external callers must go through
    /// `ToolCatalog::set_active` to keep the read-then-write contract.
    pub(crate) is_active: bool,

    /// Whether this tool requires user confirmation before execution
    /// Tools with destructive operations should set this to true.
    pub(crate) requires_confirmation: bool,

    /// Tool safety level for plan confirmation and rollback behavior
    ///
    /// Determines whether confirmation is required and if rollback is possible.
    #[serde(default)]
    pub(crate) safety_level: ToolSafetyLevel,

    /// Parent service name (for MCP sub-tools)
    /// e.g., "fs" for "`fs:read_file`"
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

    /// Parameter hint for UI display (e.g., "[topic]", "<name>")
    /// Shown inline next to command name in completion dropdowns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param_hint: Option<String>,

    /// IDs of nested subtools (for namespace commands like /mcp, /skill)
    /// Empty for leaf commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtools: Vec<String>,

    /// Localization key for i18n lookup
    /// e.g., "tool.search" maps to "tool.search.hint", "tool.search.description"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub localization_key: Option<String>,

    /// Quick check for builtin status
    /// True for system builtin commands (/search, /webfetch)
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
    /// e.g., "`builtin_search`", "`general_chat`"
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
    // Channel Visibility
    // =========================================================================
    /// Channels that can see this command (empty = all channels)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visible_channels: Vec<ChannelType>,
}

impl UnifiedTool {
    /// Create a new `UnifiedTool` with required fields
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
            aliases: Vec::new(),
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
            param_hint: None,
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
            // Visibility defaults
            visible_channels: Vec::new(),
        }
    }
}
