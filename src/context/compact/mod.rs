//! Cross-turn context compaction and the live-conversation compaction framework.
//!
//! Houses the LLM-based `ContextCompactor` plus the framework types and components
//! (PressureLevel, CompactionStrategy trait, etc.).
//!
//! `session_summary_source` (cross-session artifact consumer) remains under
//! `crate::memory::session_compactor::summary_source` — it is not part of the
//! live-compaction framework.

pub mod compactor;
pub mod constraint_injector;
pub mod file_content_tracker;
pub mod summary_utils;
pub mod tool_aware_chunker;
pub mod types;

pub use constraint_injector::{
    Constraint, ConstraintCategory, ConstraintInjector, ConstraintSource,
};
pub use file_content_tracker::FileContentTracker;
pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker};
pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
    TokenEstimate,
};
