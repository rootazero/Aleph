//! Tool Server Types
//!
//! Type definitions for tool update operations.

use serde::{Deserialize, Serialize};

/// Information about a tool update/replacement operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUpdateInfo {
    /// The tool name that was updated
    pub tool_name: String,
    /// Whether an existing tool was replaced (true) or newly added (false)
    pub was_replaced: bool,
    /// Description of the new tool
    pub new_description: String,
}
