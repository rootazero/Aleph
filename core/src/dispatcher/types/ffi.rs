//! FFI Types (UniFFI Interop)
//!
//! Simplified types for Swift/Kotlin interop via UniFFI.
//! UniFFI doesn't support enums with associated data, so we use simple
//! enum types with separate ID fields.
//!
//! Contains:
//! - ToolSourceType: Simplified source enum
//! - UnifiedToolInfo: Simplified tool representation

use super::conflict::ToolSource;
use super::unified::UnifiedTool;

// =============================================================================
// Tool Source Type (FFI)
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

// =============================================================================
// Unified Tool Info (FFI)
// =============================================================================

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
        assert_eq!(ToolSourceType::Builtin.default_icon(), "command.circle.fill");
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

    #[test]
    fn test_unified_tool_info_from_unified_tool() {
        let tool = UnifiedTool::new(
            "native:search",
            "search",
            "Search the web",
            ToolSource::Native,
        )
        .with_icon("magnifyingglass")
        .with_usage("/search <query>");

        let info = UnifiedToolInfo::from(&tool);

        assert_eq!(info.id, "native:search");
        assert_eq!(info.name, "search");
        assert_eq!(info.source_type, ToolSourceType::Native);
        assert!(info.source_id.is_none());
        assert_eq!(info.icon, Some("magnifyingglass".to_string()));
        assert_eq!(info.usage, Some("/search <query>".to_string()));
    }

    #[test]
    fn test_unified_tool_info_mcp_source() {
        let tool = UnifiedTool::new(
            "mcp:github:pr_list",
            "pr_list",
            "List PRs",
            ToolSource::Mcp {
                server: "github".to_string(),
            },
        );

        let info = UnifiedToolInfo::from(&tool);

        assert_eq!(info.source_type, ToolSourceType::Mcp);
        assert_eq!(info.source_id, Some("github".to_string()));
    }
}
