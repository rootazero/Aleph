//! Compaction orchestration — cascade degradation for the agent loop.
//!
//! This module provides the traits and types that drive context-window
//! compaction at multiple pressure levels:
//!
//! - [`types::PressureLevel`] — discrete pressure classification
//! - [`types::CompactionStrategy`] — pluggable compaction implementations
//! - [`types::PostCompactCleanup`] — hooks that run after each compaction pass
//! - [`types::CompactionContext`] / [`types::CompactionResult`] — data carriers

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
pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker};
pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
pub use types::{
    CompactionContext, CompactionResult, CompactionStrategy, PostCompactCleanup, PressureLevel,
    TokenEstimate,
};
