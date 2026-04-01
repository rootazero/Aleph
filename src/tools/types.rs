//! Tool Server Types
//!
//! Type definitions for tool repair and update operations.

/// Information about a tool name repair that was performed
#[derive(Debug, Clone)]
pub struct ToolRepairInfo {
    /// The original tool name that was requested
    pub original_name: String,
    /// The repaired tool name that was actually used
    pub repaired_name: String,
    /// The type of repair that was performed
    pub repair_type: ToolRepairType,
}

/// Types of tool name repairs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRepairType {
    /// Converted to lowercase (e.g., "Search" -> "search")
    CaseInsensitive,
    /// Converted to snake_case (e.g., "WebSearch" -> "web_search")
    SnakeCase,
    /// Routed to the "invalid" tool as a fallback
    InvalidFallback,
}

impl ToolRepairInfo {
    /// Check if this was a successful repair (not a fallback to invalid)
    pub fn was_successful(&self) -> bool {
        !matches!(self.repair_type, ToolRepairType::InvalidFallback)
    }
}

/// Information about a tool update/replacement operation
#[derive(Debug, Clone)]
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
    pub fn is_new(&self) -> bool {
        !self.was_replaced
    }

    /// Check if this was a replacement of an existing tool
    pub fn is_replacement(&self) -> bool {
        self.was_replaced
    }
}
