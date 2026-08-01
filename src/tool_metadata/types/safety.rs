//! Tool Safety Level
//!
//! Safety level classification for tool confirmation and rollback behavior.

use serde::{Deserialize, Serialize};

/// Tool safety level for confirmation and rollback behavior
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
