//! Dream pipeline stages: trait definition and stage implementations.

pub mod cluster;
pub mod collect;
pub mod consolidate;
pub mod decay;
pub mod drift;
pub mod summarize;
pub mod synthesis;

use async_trait::async_trait;

use crate::error::AlephError;
use super::DreamContext;

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

// Re-export all stages
pub use cluster::{ClusterStage, MemoryCluster, MetadataGroupKey};
pub use collect::CollectStage;
pub use consolidate::ConsolidateStage;
pub use decay::{DecayStage, MemoryDecayReport};
pub use drift::{DriftAction, DriftCandidate, DriftDetectStage};
pub use summarize::SummarizeStage;
pub use synthesis::{DeepSynthesisStage, PatternInsight};
