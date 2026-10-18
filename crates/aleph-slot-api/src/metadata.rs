//! Tool metadata value types — pure provider/panel-facing shapes.
//!
//! Assembled from the pre-migration Core modules
//! `src/tool_metadata/types/{category,definition,safety,conflict,tool_info}.rs`
//! and `src/tool_metadata/types/unified/{mod,builders,conversions,tests}.rs`
//! as part of the P0 Task 2 tool-contract closure. Every declaration is a
//! verbatim move from the original HEAD text; the only deliberate changes
//! are:
//!
//! 1. `UnifiedTool::{is_active, requires_confirmation, safety_level}` are
//!    now private (they were `pub(crate)` in Core, which the move to a
//!    separate crate would silently widen) with read-only getters and a
//!    declaration-only `set_active` — the approved recovery ruling.
//! 2. `extract_source` moved verbatim from
//!    `src/gateway/handlers/tools_visibility.rs` (old path re-exports it).
//! 3. `project_metadata_source_to_descriptor` moved verbatim from the
//!    six-arm mapping in `src/tools/handlers/builtin.rs` (old caller now
//!    calls this single definition).
//! 4. The contradictory pre-existing `populate_safety_profile(true, true)`
//!    test assertion was corrected to the algorithm's actual behavior
//!    (confirmation set, safety left at `ReadOnly`) — the user-approved
//!    correction; a fresh/preset truth-table test pins the full matrix.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// Tool category for UI grouping and filtering
///
/// Tools are classified into 5 categories based on their source:
/// - **Builtin**: Built-in rig-core tools (search, `web_fetch`, `file_ops`)
/// - **Skills**: User-configured skills (instruction injection)
/// - **Mcp**: MCP server tools (dynamically loaded)
/// - **Custom**: User-defined custom tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum ToolCategory {
    /// Built-in rig-core tools
    Builtin,
    /// User-configured skills (via UI settings)
    Skills,
    /// MCP server tools (via UI settings)
    Mcp,
    /// User-defined custom tools (via UI settings)
    Custom,
}

impl ToolCategory {
    /// Get display name for UI
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::Builtin => "Builtin",
            Self::Skills => "Skills",
            Self::Mcp => "MCP",
            Self::Custom => "Custom",
        }
    }

    /// Get SF Symbol icon name
    #[must_use]
    pub const fn icon(&self) -> &'static str {
        match self {
            Self::Builtin => "command.square.fill",
            Self::Skills => "sparkles",
            Self::Mcp => "server.rack",
            Self::Custom => "slider.horizontal.3",
        }
    }
}

impl fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Tool definition for LLM function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub requires_confirmation: bool,
    pub category: ToolCategory,
    #[serde(default)]
    pub strict: bool,
}

impl ToolDefinition {
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
            strict: false,
        }
    }

    #[must_use]
    pub const fn with_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    #[must_use]
    pub const fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }
}

#[cfg(test)]
mod definition_tests {
    use super::*;

    #[test]
    fn definition_builders_round_trip() {
        let def = ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({"type": "object"}),
            ToolCategory::Builtin,
        )
        .with_confirmation(true)
        .with_strict(true);
        assert_eq!(def.name, "search");
        assert!(def.requires_confirmation);
        assert!(def.strict);
    }
}

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

// =============================================================================
// Tool Priority
// =============================================================================

/// Tool priority for conflict resolution
///
/// When multiple tools have the same name, the higher priority tool wins
/// and the lower priority tool is renamed with a suffix.
///
/// Priority order (highest to lowest):
/// 1. Builtin (6) - System commands like /search, /webfetch
/// 2. Native (5) - System capabilities implementations
/// 3. Custom (4) - User-defined rules from config.toml
/// 4. Mcp (3) - External MCP server tools
/// 5. Plugin (2) - Plugin tools from manifests
/// 6. Skill (1) - Claude Agent skills
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ToolPriority {
    /// Lowest priority - Claude Agent skills
    Skill = 1,
    /// Plugin tools from manifests
    Plugin = 2,
    /// External MCP server tools
    Mcp = 3,
    /// User-defined custom rules
    Custom = 4,
    /// System native capabilities
    Native = 5,
    /// Highest priority - System builtin commands
    Builtin = 6,
}

// =============================================================================
// Tool Source
// =============================================================================

/// Tool source origin
///
/// Identifies where a tool comes from, enabling proper routing
/// and UI grouping (badges, icons).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(tag = "type", content = "payload")]
pub enum ToolSource {
    /// Built-in native capabilities (Search, Video)
    /// These are always available without any configuration.
    Native,

    /// System builtin commands (/search, /webfetch)
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
    /// Instruction-injection skills from ~/.aleph/skills/
    Skill {
        /// Skill directory ID (e.g., "refine-text")
        id: String,
        /// Owning plugin, when a plugin registered this entry. See
        /// `skill::SkillInfo::plugin_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin_id: Option<String>,
    },

    /// User-defined custom command from config.toml
    /// These are [\[rules\]] entries with ^/ prefix patterns.
    Custom {
        /// Index in the rules array for reference
        rule_index: usize,
    },

    /// Plugin tool from ~/.aleph/plugins/
    /// These are tools declared in plugin manifests (aleph.plugin.toml).
    Plugin {
        /// Plugin identifier (e.g., "diagnostics")
        plugin_id: String,
    },
}

impl ToolSource {
    /// Get a short type label for UI display
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Native => "Native",
            Self::Builtin => "Builtin",
            Self::Mcp { .. } => "MCP",
            Self::Skill { .. } => "Skill",
            Self::Custom { .. } => "Custom",
            Self::Plugin { .. } => "Plugin",
        }
    }

    /// Get the priority level for conflict resolution
    ///
    /// Higher priority tools win name conflicts and lower priority tools
    /// are renamed with a suffix.
    #[must_use]
    pub const fn priority(&self) -> ToolPriority {
        match self {
            Self::Builtin => ToolPriority::Builtin,
            Self::Native => ToolPriority::Native,
            Self::Custom { .. } => ToolPriority::Custom,
            Self::Mcp { .. } => ToolPriority::Mcp,
            Self::Plugin { .. } => ToolPriority::Plugin,
            Self::Skill { .. } => ToolPriority::Skill,
        }
    }

    /// Get the suffix used when renaming a conflicting tool
    ///
    /// When a tool loses a name conflict, it's renamed to `{name}-{suffix}`.
    /// For example, an MCP tool named "search" becomes "search-mcp".
    #[must_use]
    pub const fn suffix(&self) -> &'static str {
        match self {
            Self::Builtin => "system",
            Self::Native => "native",
            Self::Custom { .. } => "custom",
            Self::Mcp { .. } => "mcp",
            Self::Plugin { .. } => "plugin",
            Self::Skill { .. } => "skill",
        }
    }

    /// Check if this source is a builtin command
    #[must_use]
    pub const fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin)
    }

    /// Check if this source is an MCP tool
    #[must_use]
    pub const fn is_mcp(&self) -> bool {
        matches!(self, Self::Mcp { .. })
    }

    /// Check if this source is a skill
    #[must_use]
    pub const fn is_skill(&self) -> bool {
        matches!(self, Self::Skill { .. })
    }

    /// Display priority for UI sorting (lower = first).
    ///
    /// Order: Builtin (0) > Native (1) > Custom (2) > Mcp (3) > Plugin (4) > Skill (5).
    #[must_use]
    pub const fn display_priority(&self) -> u8 {
        match self {
            Self::Builtin => 0,
            Self::Native => 1,
            Self::Custom { .. } => 2,
            Self::Mcp { .. } => 3,
            Self::Plugin { .. } => 4,
            Self::Skill { .. } => 5,
        }
    }

    /// Format a tool ID for this source type.
    ///
    /// E.g. `ToolSource::Mcp { server: "fs" }.format_tool_id("read_file")` → `"mcp:fs:read_file"`.
    #[must_use]
    pub fn format_tool_id(&self, name: &str) -> String {
        match self {
            Self::Native => format!("native:{name}"),
            Self::Builtin => format!("builtin:{name}"),
            Self::Mcp { server } => format!("mcp:{server}:{name}"),
            Self::Skill { id, .. } => {
                // Skills are identified by their internal id only — the tool
                // name is a display label and not part of the identity.
                format!("skill:{id}")
            }
            Self::Custom { rule_index } => format!("custom:{rule_index}:{name}"),
            Self::Plugin { plugin_id } => format!("plugin:{plugin_id}:{name}"),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod conflict_tests {
    use super::*;

    #[test]
    fn test_tool_priority_ordering() {
        // Verify priority ordering: Builtin > Native > Custom > Mcp > Skill
        assert!(ToolPriority::Builtin > ToolPriority::Native);
        assert!(ToolPriority::Native > ToolPriority::Custom);
        assert!(ToolPriority::Custom > ToolPriority::Mcp);
        assert!(ToolPriority::Mcp > ToolPriority::Skill);
    }

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
            ToolSource::Skill {
                id: "test".into(),
                plugin_id: None
            }
            .label(),
            "Skill"
        );
        assert_eq!(ToolSource::Custom { rule_index: 0 }.label(), "Custom");
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
            ToolSource::Skill {
                id: "test".into(),
                plugin_id: None
            }
            .priority(),
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
        assert_eq!(
            ToolSource::Skill {
                id: "test".into(),
                plugin_id: None
            }
            .suffix(),
            "skill"
        );
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

        assert!(ToolSource::Skill {
            id: "test".into(),
            plugin_id: None
        }
        .is_skill());
        assert!(!ToolSource::Builtin.is_skill());
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

// =============================================================================
// Tool Source Type
// =============================================================================

/// Tool source type (simplified enum for JSON serialization)
///
/// Uses a simple enum with a separate `source_id` field for easy JSON encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSourceType {
    /// Built-in native capabilities (Search, `WebFetch`)
    Native,
    /// System builtin commands (/search, /webfetch)
    Builtin,
    /// MCP server tool
    Mcp,
    /// Claude Agent Skill
    Skill,
    /// User-defined custom command
    Custom,
    /// Plugin tool from manifest
    Plugin,
}

impl From<&ToolSource> for ToolSourceType {
    fn from(source: &ToolSource) -> Self {
        match source {
            ToolSource::Native => Self::Native,
            ToolSource::Builtin => Self::Builtin,
            ToolSource::Mcp { .. } => Self::Mcp,
            ToolSource::Skill { .. } => Self::Skill,
            ToolSource::Custom { .. } => Self::Custom,
            ToolSource::Plugin { .. } => Self::Plugin,
        }
    }
}

impl ToolSourceType {
    /// Get default SF Symbol icon for this source type
    ///
    /// Used for UI display in command completion and settings.
    #[must_use]
    pub const fn default_icon(&self) -> &'static str {
        match self {
            Self::Native | Self::Builtin => "command.circle.fill",
            Self::Mcp => "bolt.fill",
            Self::Skill => "lightbulb.fill",
            Self::Custom => "command",
            Self::Plugin => "puzzlepiece.extension",
        }
    }

    /// Get badge label for this source type
    #[must_use]
    pub const fn badge_label(&self) -> &'static str {
        match self {
            Self::Native | Self::Builtin => "System",
            Self::Mcp => "MCP",
            Self::Skill => "Skill",
            Self::Custom => "Custom",
            Self::Plugin => "Plugin",
        }
    }
}

// =============================================================================

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tool_info_tests {
    use super::*;

    #[test]
    fn test_tool_source_type_from_source() {
        assert_eq!(
            ToolSourceType::from(&ToolSource::Native),
            ToolSourceType::Native
        );
        assert_eq!(
            ToolSourceType::from(&ToolSource::Builtin),
            ToolSourceType::Builtin
        );
        assert_eq!(
            ToolSourceType::from(&ToolSource::Mcp {
                server: "test".into()
            }),
            ToolSourceType::Mcp
        );
        assert_eq!(
            ToolSourceType::from(&ToolSource::Skill {
                id: "test".into(),
                plugin_id: None
            }),
            ToolSourceType::Skill
        );
        assert_eq!(
            ToolSourceType::from(&ToolSource::Custom { rule_index: 0 }),
            ToolSourceType::Custom
        );
    }

    #[test]
    fn test_tool_source_type_default_icon() {
        assert_eq!(ToolSourceType::Native.default_icon(), "command.circle.fill");
        assert_eq!(
            ToolSourceType::Builtin.default_icon(),
            "command.circle.fill"
        );
        assert_eq!(ToolSourceType::Mcp.default_icon(), "bolt.fill");
        assert_eq!(ToolSourceType::Skill.default_icon(), "lightbulb.fill");
        assert_eq!(ToolSourceType::Custom.default_icon(), "command");
    }

    #[test]
    fn test_tool_source_type_badge_label() {
        assert_eq!(ToolSourceType::Native.badge_label(), "System");
        assert_eq!(ToolSourceType::Builtin.badge_label(), "System");
        assert_eq!(ToolSourceType::Mcp.badge_label(), "MCP");
        assert_eq!(ToolSourceType::Skill.badge_label(), "Skill");
        assert_eq!(ToolSourceType::Custom.badge_label(), "Custom");
    }
}

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

    /// Per-result token budget the tool declares
    /// (`crate::tools::AlephTool::MAX_RESULT_TOKENS`), stamped at
    /// registration and carried by `RegistryToolAdapter` to the Layer-2
    /// resolver. `None` = the global default. Runtime-only: it is not part of
    /// the tool's wire description, so it is never serialized.
    #[serde(skip)]
    pub max_result_tokens: Option<usize>,

    /// Whether this tool is currently active/enabled
    /// Disabled tools are excluded from routing and prompt generation.
    /// Mutation is private so the registry's lock-protected writers are
    /// the only path that flips this bit — external callers must go through
    /// `ToolCatalog::set_active` to keep the read-then-write contract.
    /// (Was `pub(crate)` in Core; the P0 Task 2 move to a separate crate
    /// would have silently severed every Core reader, so the field is now
    /// private with a getter — the approved recovery ruling.)
    is_active: bool,

    /// Whether this tool requires user confirmation before execution
    /// Tools with destructive operations should set this to true.
    requires_confirmation: bool,

    /// Tool safety level for plan confirmation and rollback behavior
    ///
    /// Determines whether confirmation is required and if rollback is possible.
    #[serde(default)]
    safety_level: ToolSafetyLevel,

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
    /// e.g., "/search `&lt;query&gt;`", "/mcp `&lt;tool&gt;` \[params\]"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,

    /// Parameter hint for UI display (e.g., "\[topic\]", "`&lt;name&gt;`")
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

    /// System prompt declared by a custom `[[rules]]` entry.
    ///
    /// Written only by `register_custom_commands`; skills deliberately leave
    /// it unset. Its single reader is `CommandContext::Custom.system_prompt`,
    /// which reaches the slash-command envelope's `system_prompt` key — and
    /// the envelope's `"custom"` arm falls through to the agent loop without
    /// reading it, so nothing injects this string into a prompt today. The
    /// same text does reach the model as the tool's `description`
    /// (`registration.rs` truncates it to 100 chars). Do not describe this as
    /// an injected prompt until a consumer actually injects it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_system_prompt: Option<String>,

    /// The row's validated `allowed-tools:`, as Aleph tool names.
    ///
    /// Only `ToolRegistrar::register_skills` populates this today, for skill
    /// and plugin-command rows (`registration::AllowedToolsMeaning`). For a
    /// plugin command
    /// (`commands/*.md`, already mapped to Aleph names at parse time) it is
    /// what the run's tool surface is narrowed to: `None` = the command
    /// declares nothing, so the run keeps the agent's full tool surface;
    /// `Some(vec![])` = an explicit deny-all. For a skill it is what a typed
    /// `/<skill>` may pre-grant — Claude Code names mapped, what grants
    /// nothing dropped — and narrows nothing, except on the stale-row arm: a
    /// skill row whose manifest is gone by the turn restricts like a command's
    /// (`slash_skill_pregrant::split`). The `Option` is not decoration:
    /// it is the only thing that keeps "deny everything" distinguishable from
    /// "said nothing" by the time the value reaches `ScopedToolService`, which
    /// reads an empty allow-set as allow-all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_capabilities: Option<Vec<String>>,

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
            max_result_tokens: None,
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
            routing_capabilities: None,
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

    /// Read-only accessor for the private `is_active` field (P0 Task 2
    /// recovery ruling: the three sensitive fields are private; Core
    /// readers that used to be same-crate now go through these getters).
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.is_active
    }

    /// Read-only accessor for the private `requires_confirmation` field.
    #[must_use]
    pub const fn requires_confirmation(&self) -> bool {
        self.requires_confirmation
    }

    /// Read-only accessor for the private `safety_level` field.
    #[must_use]
    pub const fn safety_level(&self) -> ToolSafetyLevel {
        self.safety_level
    }

    /// Declaration-only setter for `is_active`.
    ///
    /// This flips the *declared* bit on this value only. It does NOT
    /// activate or deactivate the tool in any registry, catalog, or grant
    /// set — the registry's lock-protected writers
    /// (`ToolCatalog::set_active`) remain the only path that changes
    /// effective availability. Callers must not treat a `set_active(true)`
    /// value as an admitted tool.
    pub fn set_active(&mut self, active: bool) {
        self.is_active = active;
    }
}

impl UnifiedTool {
    // =========================================================================
    // Basic Builder Methods
    // =========================================================================

    /// Builder method: set display name
    pub fn with_display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = display_name.into();
        self
    }

    /// Builder method: set alternative invocation names (aliases).
    ///
    /// Replaces any existing aliases. Empty/whitespace-only entries are dropped
    /// and duplicates of the canonical name are ignored so resolution stays
    /// unambiguous.
    pub fn with_aliases<I, S>(mut self, aliases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let canonical = self.name.to_lowercase();
        self.aliases = aliases
            .into_iter()
            .map(|a| a.into())
            .filter(|a| {
                let t = a.trim();
                !t.is_empty() && t.to_lowercase() != canonical
            })
            .collect();
        self
    }

    /// Builder method: set parameters schema
    #[must_use]
    pub fn with_parameters_schema(mut self, schema: Value) -> Self {
        self.parameters_schema = Some(schema);
        self
    }

    /// Builder method: set requires confirmation
    #[must_use]
    pub const fn with_requires_confirmation(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Builder method: set safety level
    #[must_use]
    pub const fn with_safety_level(mut self, level: ToolSafetyLevel) -> Self {
        self.safety_level = level;
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

    /// Builder method: set parameter hint for UI display
    #[must_use]
    pub fn with_param_hint(mut self, hint: &str) -> Self {
        self.param_hint = Some(hint.to_string());
        self
    }

    /// Builder method: set localization key
    pub fn with_localization_key(mut self, key: impl Into<String>) -> Self {
        self.localization_key = Some(key.into());
        self
    }

    /// Builder method: set sort order
    #[must_use]
    pub const fn with_sort_order(mut self, order: i32) -> Self {
        self.sort_order = order;
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

    /// Builder method: set the command's declared tool-surface narrowing.
    ///
    /// Takes the whole `Option` so a caller can express "the author declared
    /// an empty list" (`Some(vec![])`, deny-all) as distinct from "the author
    /// declared nothing" (`None`, allow-all). A `Vec`-taking signature forces
    /// the caller to skip the call for the empty case, which is exactly how
    /// the distinction gets lost.
    #[must_use]
    pub fn with_routing_capabilities(mut self, caps: Option<Vec<String>>) -> Self {
        self.routing_capabilities = caps;
        self
    }

    /// Builder method: set routing intent type
    pub fn with_routing_intent_type(mut self, intent: impl Into<String>) -> Self {
        self.routing_intent_type = Some(intent.into());
        self
    }

    /// Builder method: set routing strip prefix
    #[must_use]
    pub const fn with_routing_strip_prefix(mut self, strip: bool) -> Self {
        self.routing_strip_prefix = strip;
        self
    }

    // =========================================================================
    // Visibility Builder Methods
    // =========================================================================

    /// Builder method: set visible channels
    #[must_use]
    pub fn with_visible_channels(mut self, channels: Vec<ChannelType>) -> Self {
        self.visible_channels = channels;
        self
    }

    /// Populate `requires_confirmation` and `safety_level` from a tool's
    /// handler-side flags. Every registration path (MCP, builtin, skills,
    /// plugins, custom commands) MUST call this on a `UnifiedTool` before
    /// handing it to `register_with_conflict_resolution`; the catalog-side
    /// flags drive `infer_visible_channels`, which gates iMessage / Telegram
    /// / Discord exclusion on them. Skipping this helper makes the entry
    /// default to `requires_confirmation=false, safety_level=ReadOnly`, and
    /// `infer_visible_channels` falls through to `Vec::new()` for the entry,
    /// exposing the tool to every channel — including iMessage.
    ///
    /// Policy (matches the contract the MCP path established in round-8):
    /// - `requires_confirmation=true` → `with_requires_confirmation(true)`
    /// - `!read_only && requires_confirmation` → `IrreversibleHighRisk`
    ///   (panel/cli-only gating kicks in for destructive tools)
    /// - `!read_only && !requires_confirmation` → `Reversible`
    ///   (catalog reflects that the tool mutates state, without the
    ///   panel/cli-only gate on its own)
    /// - `read_only` → safety level left at the default `ReadOnly`
    ///
    /// (occams-r9 tool_metadata C-NEW-1.)
    #[must_use]
    pub fn populate_safety_profile(mut self, read_only: bool, requires_confirmation: bool) -> Self {
        if requires_confirmation {
            self = self.with_requires_confirmation(true);
        }
        if !read_only && requires_confirmation {
            self = self.with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);
        } else if !read_only {
            self = self.with_safety_level(ToolSafetyLevel::Reversible);
        }
        self
    }
}

#[cfg(test)]
mod populate_safety_profile_tests {
    use super::*;

    fn fresh(name: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{name}"),
            name,
            "test tool",
            ToolSource::Native,
        )
    }

    #[test]
    fn read_only_tool_keeps_default_safety_level() {
        let tool = fresh("ro").populate_safety_profile(true, false);
        assert!(!tool.requires_confirmation);
        assert_eq!(tool.safety_level, ToolSafetyLevel::ReadOnly);
    }

    #[test]
    fn mutating_without_confirmation_is_reversible() {
        let tool = fresh("mut").populate_safety_profile(false, false);
        assert!(!tool.requires_confirmation);
        assert_eq!(tool.safety_level, ToolSafetyLevel::Reversible);
    }

    #[test]
    fn mutating_with_confirmation_is_irreversible_high_risk() {
        let tool = fresh("destruct").populate_safety_profile(false, true);
        assert!(tool.requires_confirmation);
        assert_eq!(tool.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
    }

    /// CORRECTED (user-approved): the pre-migration version of this test
    /// was named `read_only_with_confirmation_still_irreversible_high_risk`
    /// and asserted `IrreversibleHighRisk`, contradicting the algorithm —
    /// `populate_safety_profile` only escalates `safety_level` when
    /// `!read_only`, so a `read_only=true` DTO keeps its current level.
    /// The implementation is preserved; the assertion now matches it.
    #[test]
    fn read_only_with_confirmation_keeps_default_read_only_safety() {
        // Edge case: a tool that asks for confirmation even though it
        // declares read_only. Confirmation is set, but the safety level
        // stays at the default `ReadOnly` because the algorithm never
        // escalates a read-only tool.
        let tool = fresh("weird").populate_safety_profile(true, true);
        assert!(tool.requires_confirmation);
        assert_eq!(tool.safety_level, ToolSafetyLevel::ReadOnly);
    }

    /// Full truth table over the `(read_only, requires_confirmation)` input
    /// space, for both a fresh DTO (default safety `ReadOnly`) and a preset
    /// DTO (safety pre-set to `IrreversibleHighRisk`): the algorithm only
    /// ever *sets* levels on mutating tools, it never *resets* a preset one.
    #[test]
    fn populate_safety_profile_truth_table_fresh_and_preset() {
        // Fresh DTO (safety starts at the `ReadOnly` default).
        let t = fresh("ff").populate_safety_profile(false, false);
        assert!(!t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::Reversible);
        let t = fresh("ft").populate_safety_profile(false, true);
        assert!(t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
        let t = fresh("tf").populate_safety_profile(true, false);
        assert!(!t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::ReadOnly);
        let t = fresh("tt").populate_safety_profile(true, true);
        assert!(t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::ReadOnly);

        // Preset DTO (safety pre-set before the call): read-only inputs
        // leave the preset untouched; mutating inputs overwrite it.
        let preset = || fresh("preset").with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);
        let t = preset().populate_safety_profile(true, false);
        assert!(!t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
        let t = preset().populate_safety_profile(true, true);
        assert!(t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
        let t = preset().populate_safety_profile(false, false);
        assert!(!t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::Reversible);
        let t = preset().populate_safety_profile(false, true);
        assert!(t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::IrreversibleHighRisk);

        // Sticky confirmation: the algorithm only ever *sets* the flag, it
        // never resets it — a DTO already requiring confirmation keeps
        // requiring it even when the call passes `false`.
        let t = fresh("sticky")
            .with_requires_confirmation(true)
            .populate_safety_profile(false, false);
        assert!(t.requires_confirmation);
        assert_eq!(t.safety_level, ToolSafetyLevel::Reversible);
    }
}

impl UnifiedTool {
    /// Format tool for LLM prompt inclusion
    ///
    /// Returns a markdown-formatted line for system prompt injection.
    /// Builtin and Native tools are marked as "Preferred" to guide L3 routing priority.
    #[must_use]
    pub fn to_prompt_line(&self) -> String {
        let source_badge = match &self.source {
            ToolSource::Native => " [Native - Preferred]".to_string(),
            ToolSource::Builtin => " [Builtin - Preferred]".to_string(),
            ToolSource::Mcp { server } => format!(" [MCP:{server}]"),
            ToolSource::Skill { id, .. } => format!(" [Skill:{id}]"),
            ToolSource::Custom { .. } => " [Custom]".to_string(),
            ToolSource::Plugin { plugin_id } => format!(" [Plugin:{plugin_id}]"),
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
// UnifiedTool tests (moved from Core `tool_metadata/types/unified/tests.rs`)
// =============================================================================

#[cfg(test)]
mod unified_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_builtin_tool_via_new() {
        let tool = UnifiedTool::new(
            "builtin:search",
            "search",
            "Search the web",
            ToolSource::Builtin,
        )
        .with_display_name("Web Search")
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
    fn test_unified_tool_with_safety_level() {
        let tool = UnifiedTool::new(
            "native:delete_file",
            "delete_file",
            "Delete a file",
            ToolSource::Native,
        )
        .with_safety_level(ToolSafetyLevel::IrreversibleHighRisk);

        assert_eq!(tool.safety_level, ToolSafetyLevel::IrreversibleHighRisk);
    }

    #[test]
    fn test_visible_channels_builder() {
        let tool = UnifiedTool::new("builtin:help", "help", "Show help", ToolSource::Builtin)
            .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);

        assert!(tool.visible_channels.contains(&ChannelType::Panel));
    }

    #[test]
    fn test_channel_type_equality() {
        assert_eq!(ChannelType::Panel, ChannelType::Panel);
        assert_ne!(ChannelType::Panel, ChannelType::Telegram);
    }
}

// =============================================================================
// Source extraction / descriptor projection
// =============================================================================

/// Extract `(source_key, source_display)` from a metadata [`ToolSource`].
///
/// Moved verbatim from Core `gateway/handlers/tools_visibility.rs` (P0 Task
/// 2 approved caller gate C). The old Core path re-exports this.
#[must_use]
pub fn extract_source(source: &ToolSource) -> (String, String) {
    match source {
        ToolSource::Native => ("native".into(), "Native".into()),
        ToolSource::Builtin => ("builtin".into(), "Built-in".into()),
        ToolSource::Mcp { server } => (format!("mcp:{server}"), server.clone()),
        ToolSource::Skill { id, .. } => (format!("skill:{id}"), id.clone()),
        ToolSource::Plugin { plugin_id } => (format!("plugin:{plugin_id}"), plugin_id.clone()),
        ToolSource::Custom { .. } => ("custom".into(), "Custom".into()),
    }
}

/// Project a metadata [`ToolSource`] onto the descriptor-level
/// [`crate::descriptor::ToolSource`].
///
/// Moved verbatim from Core `tools/handlers/builtin.rs` (P0 Task 2 approved
/// caller gate C six-arm mapping). Arm semantics pinned by test:
/// - `Builtin` → `Builtin`
/// - `Mcp { server }` → `Mcp { server_id: server }`
/// - `Plugin { plugin_id }` → `Extension { plugin_id }`
/// - `Skill { id, plugin_id }` → `Extension { plugin_id: plugin_id.unwrap_or(id) }`
///   (plugin-owned skills project onto their owner; standalone skills fall
///   back to their own skill id — still an `Extension`, never `Builtin`)
/// - `Native` / `Custom` → `Builtin` (no descriptor-level counterpart)
///
/// There is no unknown arm: metadata `ToolSource` is `#[non_exhaustive]` but
/// this crate defines it, so the match is exhaustive here; any *future*
/// variant fails to compile rather than silently mapping (fail-closed).
#[must_use]
pub fn project_metadata_source_to_descriptor(source: &ToolSource) -> crate::descriptor::ToolSource {
    match source {
        ToolSource::Builtin => crate::descriptor::ToolSource::Builtin,
        ToolSource::Mcp { server } => crate::descriptor::ToolSource::Mcp {
            server_id: server.clone(),
        },
        ToolSource::Plugin { plugin_id } => crate::descriptor::ToolSource::Extension {
            plugin_id: plugin_id.clone(),
        },
        ToolSource::Skill { id, plugin_id } => crate::descriptor::ToolSource::Extension {
            plugin_id: plugin_id.clone().unwrap_or_else(|| id.clone()),
        },
        ToolSource::Native | ToolSource::Custom { .. } => crate::descriptor::ToolSource::Builtin,
    }
}

#[cfg(test)]
mod source_projection_tests {
    use super::*;

    #[test]
    fn test_extract_source_all_variants() {
        assert_eq!(
            extract_source(&ToolSource::Native),
            ("native".into(), "Native".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Builtin),
            ("builtin".into(), "Built-in".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Mcp {
                server: "github".into()
            }),
            ("mcp:github".into(), "github".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Skill {
                id: "refine-text".into(),
                plugin_id: None
            }),
            ("skill:refine-text".into(), "refine-text".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Plugin {
                plugin_id: "diag".into()
            }),
            ("plugin:diag".into(), "diag".into())
        );
        assert_eq!(
            extract_source(&ToolSource::Custom { rule_index: 42 }),
            ("custom".into(), "Custom".into())
        );
    }

    #[test]
    fn project_metadata_source_to_descriptor_all_six_arms() {
        use crate::descriptor::ToolSource as D;
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Builtin),
            D::Builtin
        );
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Mcp {
                server: "fs".into()
            }),
            D::Mcp {
                server_id: "fs".into()
            }
        );
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Plugin {
                plugin_id: "diag".into()
            }),
            D::Extension {
                plugin_id: "diag".into()
            }
        );
        // Plugin-owned skill → owning plugin.
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Skill {
                id: "refine-text".into(),
                plugin_id: Some("text-plugin".into())
            }),
            D::Extension {
                plugin_id: "text-plugin".into()
            }
        );
        // Standalone skill → Extension keyed by the skill's own id (fallback).
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Skill {
                id: "refine-text".into(),
                plugin_id: None
            }),
            D::Extension {
                plugin_id: "refine-text".into()
            }
        );
        // Native and Custom collapse onto Builtin.
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Native),
            D::Builtin
        );
        assert_eq!(
            project_metadata_source_to_descriptor(&ToolSource::Custom { rule_index: 7 }),
            D::Builtin
        );
    }
}
