//! Tool Category
//!
//! Tool classification for UI grouping and filtering.

use serde::{Deserialize, Serialize};
use std::fmt;

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
