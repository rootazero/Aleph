//! Tool Server Types
//!
//! Type definitions for tool repair and update operations.

use serde::{Deserialize, Serialize};

/// Information about a tool name repair that was performed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRepairInfo {
    /// The original tool name that was requested
    pub original_name: String,
    /// The repaired tool name that was actually used
    pub repaired_name: String,
    /// The type of repair that was performed
    pub repair_type: ToolRepairType,
}

/// Types of tool name repairs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRepairType {
    /// Converted to lowercase (e.g., "Search" -> "search")
    CaseInsensitive,
    /// Converted to `snake_case` (e.g., "`WebSearch`" -> "`web_search`")
    SnakeCase,
}

/// Information about a tool update/replacement operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUpdateInfo {
    /// The tool name that was updated
    pub tool_name: String,
    /// Whether an existing tool was replaced (true) or newly added (false)
    pub was_replaced: bool,
    /// Description of the old tool (if replaced)
    pub old_description: Option<String>,
    /// Description of the new tool
    pub new_description: String,
}

impl ToolUpdateInfo {
    /// Check if this was a new addition (not a replacement)
    #[must_use]
    pub const fn is_new(&self) -> bool {
        !self.was_replaced
    }

    /// Check if this was a replacement of an existing tool
    #[must_use]
    pub const fn is_replacement(&self) -> bool {
        self.was_replaced
    }
}
