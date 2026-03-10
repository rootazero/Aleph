//! Conflict Resolution System
//!
//! Handles naming conflicts in the flat namespace when multiple tools
//! have the same name from different sources.
//!
//! Contains:
//! - ToolPriority: Priority ordering for conflict resolution
//! - ConflictInfo: Information about existing conflicting tool
//! - ConflictResolution: Resolution strategy enum
//! - ToolSource: Tool origin source enum

use serde::{Deserialize, Serialize};

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
// Conflict Info
// =============================================================================

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

// =============================================================================
// Conflict Resolution
// =============================================================================

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

// =============================================================================
// Tool Source
// =============================================================================

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
    },

    /// User-defined custom command from config.toml
    /// These are [[rules]] entries with ^/ prefix patterns.
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
    pub fn label(&self) -> &'static str {
        match self {
            ToolSource::Native => "Native",
            ToolSource::Builtin => "Builtin",
            ToolSource::Mcp { .. } => "MCP",
            ToolSource::Skill { .. } => "Skill",
            ToolSource::Custom { .. } => "Custom",
            ToolSource::Plugin { .. } => "Plugin",
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
            ToolSource::Plugin { .. } => "puzzlepiece.extension",
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
            ToolSource::Plugin { .. } => ToolPriority::Plugin,
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
            ToolSource::Plugin { .. } => "plugin",
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

    /// Check if this source is a plugin tool
    pub fn is_plugin(&self) -> bool {
        matches!(self, ToolSource::Plugin { .. })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
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
        assert_eq!(ToolSource::Skill { id: "test".into() }.label(), "Skill");
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
