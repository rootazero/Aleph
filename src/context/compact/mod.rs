//! Cross-turn context compaction and the live-conversation compaction framework.
//!
//! This module houses the full compaction surface: the LLM-based `ContextCompactor`
//! (relocated from `src/harness/` in P0) plus the framework types and components
//! (PressureLevel, CompactionStrategy trait, Orchestrator, MicroCompactor, etc.)
//! relocated from `src/memory/compaction/` in P1.
//!
//! `session_summary_source` (cross-session artifact consumer) remains under
//! `crate::memory::session_compactor::summary_source` — it is not part of the
//! live-compaction framework.

pub mod compactor;
pub mod constraint_injector;
pub mod file_content_tracker;
pub mod micro_compactor;
pub mod orchestrator;
pub mod summary_utils;
pub mod tool_aware_chunker;
pub mod types;

pub use constraint_injector::{
    Constraint, ConstraintCategory, ConstraintInjector, ConstraintSource,
};
pub use file_content_tracker::FileContentTracker;
pub use micro_compactor::{
    classify_importance, format_compact_placeholder, Importance, MicroCompactor,
    MicroCompactorConfig, ToolOutputEntry,
};
pub use orchestrator::{CompactionOrchestrator, OrchestratorBuilder};
pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker};
pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
    TokenEstimate,
};
