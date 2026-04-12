//! Dream pipeline stages: trait definition and stage implementations.

pub mod daily_digest;
pub mod types;

pub use daily_digest::DailyDigestStage;

// Old stages — temporarily disabled during migration to notes layer.
// Task 8 will replace these with new note-based implementations.
// #[cfg(never)]
// pub mod consolidate;
// #[cfg(never)]
// pub mod decay;
// #[cfg(never)]
// pub mod drift;
// #[cfg(never)]
// pub mod summarize;
// #[cfg(never)]
// pub mod synthesis;
// #[cfg(never)]
// pub mod tunnel;
// #[cfg(never)]
// pub mod wiki_ingest;
// #[cfg(never)]
// pub mod wiki_lint;

use async_trait::async_trait;

use super::DreamContext;
use crate::error::AlephError;

/// A single stage in the dream pipeline.
///
/// Each stage receives a `DreamContext`, performs its work, and returns
/// the (potentially modified) context for the next stage.
#[async_trait]
pub trait DreamStage: Send + Sync {
    /// Human-readable name of this stage (used for logging and reports).
    fn name(&self) -> &'static str;

    /// Whether this stage should run given the current context.
    /// Returning `false` skips this stage without error.
    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    /// Execute the stage, consuming and returning the context.
    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError>;
}

// Re-export types still needed by other modules
pub use types::{MemoryCluster, MetadataGroupKey};
