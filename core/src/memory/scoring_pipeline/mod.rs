//! Scoring pipeline for memory retrieval.
//!
//! The pipeline applies a configurable sequence of [`ScoringStage`]s to a
//! list of [`ScoredFact`] candidates, progressively re-scoring, filtering,
//! and re-ordering them before they are returned to the caller.
//!
//! # Example
//!
//! ```rust,ignore
//! let pipeline = ScoringPipeline::from_config(&ScoringPipelineConfig::default());
//! let results = pipeline.run(candidates, &ctx);
//! ```

pub mod config;
pub mod context;
pub mod stages;

pub use config::ScoringPipelineConfig;
pub use context::ScoringContext;
pub use stages::ScoringStage;

use crate::memory::store::types::ScoredFact;
use stages::cosine_rerank::CosineRerankStage;
use stages::hard_min_score::HardMinScoreStage;
use stages::importance_weight::ImportanceWeightStage;
use stages::length_normalization::LengthNormalizationStage;
use stages::mmr_diversity::MmrDiversityStage;
use stages::recency_boost::RecencyBoostStage;
use stages::time_decay::TimeDecayStage;
use tracing::debug;

/// A configurable pipeline of scoring stages.
///
/// Stages are applied in insertion order. Use [`ScoringPipeline::from_config`]
/// to get the default seven-stage pipeline, or build a custom one with
/// [`ScoringPipeline::add_stage`].
pub struct ScoringPipeline {
    stages: Vec<Box<dyn ScoringStage>>,
}

impl ScoringPipeline {
    /// Create an empty pipeline with no stages.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Build the default seven-stage pipeline from configuration.
    ///
    /// Stage order:
    /// 1. Cosine-rerank blending
    /// 2. Recency boost
    /// 3. Importance weighting
    /// 4. Length normalization
    /// 5. Time decay
    /// 6. Hard minimum score gate
    /// 7. MMR diversity
    pub fn from_config(_config: &ScoringPipelineConfig) -> Self {
        let stages: Vec<Box<dyn ScoringStage>> = vec![
            Box::new(CosineRerankStage),
            Box::new(RecencyBoostStage),
            Box::new(ImportanceWeightStage),
            Box::new(LengthNormalizationStage),
            Box::new(TimeDecayStage),
            Box::new(HardMinScoreStage),
            Box::new(MmrDiversityStage),
        ];
        Self { stages }
    }

    /// Add a custom stage to the end of the pipeline.
    pub fn add_stage(&mut self, stage: Box<dyn ScoringStage>) {
        self.stages.push(stage);
    }

    /// Return the number of stages in the pipeline.
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Run all stages sequentially, returning the final candidate list.
    pub fn run(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let mut current = candidates;

        for stage in &self.stages {
            let before = current.len();
            current = stage.apply(current, ctx);
            debug!(
                stage = stage.name(),
                before = before,
                after = current.len(),
                "scoring stage applied"
            );
        }

        current
    }
}

impl Default for ScoringPipeline {
    fn default() -> Self {
        Self::from_config(&ScoringPipelineConfig::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::FactType;
    use crate::memory::context::MemoryFact;
    use crate::memory::store::types::ScoredFact;

    /// Helper: create a `ScoredFact` from a string and score.
    fn scored(content: &str, score: f32) -> ScoredFact {
        ScoredFact {
            fact: MemoryFact::new(
                content.to_string(),
                FactType::Other,
                vec![],
            ),
            score,
        }
    }

    /// Helper: create a default `ScoringContext`.
    fn default_ctx() -> ScoringContext {
        ScoringContext {
            query: "test query".to_string(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig::default(),
        }
    }

    #[test]
    fn empty_pipeline_passes_through() {
        let pipeline = ScoringPipeline::new();
        assert_eq!(pipeline.stage_count(), 0);

        let candidates = vec![scored("fact A", 0.9), scored("fact B", 0.7)];
        let ctx = default_ctx();
        let result = pipeline.run(candidates, &ctx);

        assert_eq!(result.len(), 2);
        assert!((result[0].score - 0.9).abs() < f32::EPSILON);
        assert!((result[1].score - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn default_pipeline_creates_seven_stages() {
        let pipeline = ScoringPipeline::default();
        assert_eq!(pipeline.stage_count(), 7);
    }

    #[test]
    fn from_config_creates_seven_stages() {
        let cfg = ScoringPipelineConfig::default();
        let pipeline = ScoringPipeline::from_config(&cfg);
        assert_eq!(pipeline.stage_count(), 7);
    }

    #[test]
    fn default_pipeline_passthrough_with_stubs() {
        // Since all stages are stubs (passthrough), the output should equal input.
        let pipeline = ScoringPipeline::default();
        let candidates = vec![
            scored("alpha", 0.95),
            scored("beta", 0.80),
            scored("gamma", 0.60),
        ];
        let ctx = default_ctx();
        let result = pipeline.run(candidates, &ctx);

        assert_eq!(result.len(), 3);
        assert!((result[0].score - 0.95).abs() < f32::EPSILON);
        assert!((result[1].score - 0.80).abs() < f32::EPSILON);
        assert!((result[2].score - 0.60).abs() < f32::EPSILON);
    }

    #[test]
    fn add_stage_increases_count() {
        let mut pipeline = ScoringPipeline::new();
        assert_eq!(pipeline.stage_count(), 0);

        pipeline.add_stage(Box::new(CosineRerankStage));
        assert_eq!(pipeline.stage_count(), 1);

        pipeline.add_stage(Box::new(HardMinScoreStage));
        assert_eq!(pipeline.stage_count(), 2);
    }

    #[test]
    fn empty_candidates_returns_empty() {
        let pipeline = ScoringPipeline::default();
        let ctx = default_ctx();
        let result = pipeline.run(vec![], &ctx);
        assert!(result.is_empty());
    }
}
