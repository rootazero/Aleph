//! Conflict Resolution System
//!
//! Handles naming conflicts in the flat namespace when multiple tools
//! have the same name from different sources.
//!
//! Contains:
//! - `ConflictInfo`: Information about existing conflicting tool
//! - `ConflictResolution`: Resolution strategy enum
//!
//! `ToolSource` and `ToolPriority` were moved to `aleph_slot_api::metadata`
//! as part of the P0 Task 2 tool-contract closure and are re-exported here
//! so callers using `crate::tool_metadata::types::conflict::{ToolSource,
//! ToolPriority}` continue to compile.

pub use aleph_slot_api::metadata::{ToolPriority, ToolSource};

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
#[non_exhaustive]
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
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Restored verbatim from the pre-migration Core `conflict.rs` test
    /// module (S1 repair): the migration moved this file's `ToolSource` /
    /// `ToolPriority` tests into `aleph_slot_api::metadata` but silently
    /// dropped this one. `ConflictResolution` stayed in Core, so its test
    /// belongs here beside it.
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

        // Verify they are distinct
        assert_ne!(rename_existing, rename_new);
    }
}
