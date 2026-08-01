//! Tool Info Types
//!
//! Simplified types for Gateway JSON-RPC serialization.
//! `ToolSourceType`: Simplified source enum for JSON serialization.

use super::conflict::ToolSource;

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
mod tests {
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
            ToolSourceType::from(&ToolSource::Skill { id: "test".into() }),
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
