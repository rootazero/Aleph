//! Dream pipeline stages: trait definition and stage implementations.

pub mod daily_digest;
pub mod index_refresher;
pub mod note_consolidate;
pub mod note_decay;
pub mod note_drift;
pub mod note_lint;
pub mod note_synthesis;
pub mod types;

pub use daily_digest::DailyDigestStage;
pub use index_refresher::IndexRefresherStage;
pub use note_consolidate::NoteConsolidateStage;
pub use note_decay::NoteDecayStage;
pub use note_drift::NoteDriftStage;
pub use note_lint::NoteLintStage;
pub use note_synthesis::NoteSynthesisStage;

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
