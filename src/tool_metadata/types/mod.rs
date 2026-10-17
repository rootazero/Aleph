//! Tool Metadata Type Definitions — P0 Task 2 re-export shim.
//!
//! As of P0 Task 2, the pure value types — `ToolCategory`,
//! `ToolDefinition`, `ToolSafetyLevel`, `ToolSource` (6-variant),
//! `ToolPriority`, `ConflictInfo`, `ConflictResolution`,
//! `ToolSourceType`, `UnifiedTool`, `ChannelType`, the
//! `populate_safety_profile` algorithm, the
//! `project_metadata_source_to_descriptor` projection, the
//! `extract_source` formatter, and the `to_prompt_line` rendering —
//! live in `aleph_slot_api::metadata`. This module re-exports them
//! so every existing `crate::tool_metadata::*` path keeps
//! compiling.
//!
//! The individual submodules below are kept as empty re-export
//! shims for grep-ability and to avoid churn in callers that
//! happen to reach through the deeper path; the single source of
//! truth is `aleph_slot_api::metadata::*`.

mod category;
mod conflict;
mod definition;
mod safety;
mod tool_info;
mod unified;

// =============================================================================
// Re-exports — single source of truth is `aleph_slot_api::metadata::*`.
// `ConflictInfo` / `ConflictResolution` remain Core orchestration
// values (see `types::conflict`).
// =============================================================================

pub use aleph_slot_api::metadata::*;
// Keep the glob for future API additions; these explicit aliases also make the
// legacy submodule shims intentional and used without creating new type identities.
pub use category::ToolCategory;
pub use conflict::{ConflictInfo, ConflictResolution};
pub use definition::ToolDefinition;
pub use safety::ToolSafetyLevel;
pub use tool_info::ToolSourceType;
pub use unified::{ChannelType, UnifiedTool};

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    /// S3 repair pin: the legacy internal paths
    /// `crate::tool_metadata::types::conflict::{ToolPriority, ToolSource}`
    /// remain accessible and are *the same types* as the canonical
    /// `aleph_slot_api::metadata` definitions — an alias, not a second
    /// declaration (判据 §1).
    #[test]
    fn conflict_module_reexports_are_the_canonical_types() {
        assert_eq!(
            TypeId::of::<crate::tool_metadata::types::conflict::ToolPriority>(),
            TypeId::of::<aleph_slot_api::metadata::ToolPriority>()
        );
        assert_eq!(
            TypeId::of::<crate::tool_metadata::types::conflict::ToolSource>(),
            TypeId::of::<aleph_slot_api::metadata::ToolSource>()
        );
    }
}
