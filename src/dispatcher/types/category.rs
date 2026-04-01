//! Tool Category
//!
//! Tool classification for UI grouping and filtering.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Tool category for UI grouping and filtering
///
/// Tools are classified into 5 categories based on their source:
/// - **Builtin**: Built-in rig-core tools (search, web_fetch, file_ops)
/// - **Skills**: User-configured skills (instruction injection)
/// - **Mcp**: MCP server tools (dynamically loaded)
/// - **Custom**: User-defined custom tools
/// - **GeneratedSkill**: Auto-generated from skill evolution (Skill Compiler)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Auto-generated tools from skill evolution (Skill Compiler)
    GeneratedSkill,
}

impl ToolCategory {
    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            ToolCategory::Builtin => "Builtin",
            ToolCategory::Skills => "Skills",
            ToolCategory::Mcp => "MCP",
            ToolCategory::Custom => "Custom",
            ToolCategory::GeneratedSkill => "Generated",
        }
    }

    /// Get SF Symbol icon name
    pub fn icon(&self) -> &'static str {
        match self {
            ToolCategory::Builtin => "command.square.fill",
            ToolCategory::Skills => "sparkles",
            ToolCategory::Mcp => "server.rack",
            ToolCategory::Custom => "slider.horizontal.3",
            ToolCategory::GeneratedSkill => "gearshape.2.fill",
        }
    }
}

impl fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}
