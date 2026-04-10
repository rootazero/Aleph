//! Dream pipeline stages: trait definition and stage implementations.

pub mod consolidate;
pub mod decay;
pub mod drift;
pub mod summarize;
pub mod synthesis;
pub mod tunnel;
pub mod types;
pub mod wiki_ingest;
pub mod wiki_lint;

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

// Re-export all stages
pub use consolidate::ConsolidateStage;
pub use decay::{DecayStage, MemoryDecayReport};
pub use drift::{DriftAction, DriftCandidate, DriftDetectStage};
pub use summarize::SummarizeStage;
pub use synthesis::{DeepSynthesisStage, PatternInsight};
pub use tunnel::TunnelDiscoveryStage;
pub use types::{MemoryCluster, MetadataGroupKey};
