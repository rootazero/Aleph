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
    fn default_pipeline_processes_high_score_candidates() {
        // Stages are real now — high-scoring, recent, short facts survive
        // the full pipeline (no query_embedding so cosine rerank is a no-op).
        let pipeline = ScoringPipeline::default();

        let mut fact_a = MemoryFact::new("alpha".to_string(), FactType::Other, vec![]);
        fact_a.created_at = 1700000000; // same as ctx timestamp
        fact_a.confidence = 1.0;
        let mut fact_b = MemoryFact::new("beta".to_string(), FactType::Other, vec![]);
        fact_b.created_at = 1700000000;
        fact_b.confidence = 1.0;

        let candidates = vec![
            ScoredFact { fact: fact_a, score: 0.95 },
            ScoredFact { fact: fact_b, score: 0.80 },
        ];
        let ctx = default_ctx();
        let result = pipeline.run(candidates, &ctx);

        // Both should survive hard_min_score (default 0.35)
        assert_eq!(result.len(), 2);
        // Scores will be adjusted by importance weight and time decay
        // but both should remain well above threshold
        assert!(result[0].score > 0.5);
        assert!(result[1].score > 0.5);
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

    #[test]
    fn test_full_pipeline_end_to_end() {
        // Create 3 candidates:
        //   1. recent + important (high confidence, just created)
        //   2. old + verbose (long content, created 365 days ago)
        //   3. low-score (should be filtered by hard_min_score)

        let now = 1700000000_i64;

        // Candidate 1: recent + important
        let mut fact1 = MemoryFact::new(
            "User prefers Rust.".to_string(),
            FactType::Preference,
            vec![],
        );
        fact1.created_at = now; // brand new
        fact1.confidence = 1.0; // max confidence

        // Candidate 2: old + verbose
        let mut fact2 = MemoryFact::new(
            "x".repeat(2000), // very long content
            FactType::Other,
            vec![],
        );
        fact2.created_at = now - 365 * 86400; // 1 year old
        fact2.confidence = 0.5;

        // Candidate 3: low starting score — will be filtered
        let mut fact3 = MemoryFact::new(
            "marginal fact".to_string(),
            FactType::Other,
            vec![],
        );
        fact3.created_at = now - 180 * 86400;
        fact3.confidence = 0.3;

        let candidates = vec![
            ScoredFact { fact: fact1, score: 0.90 },
            ScoredFact { fact: fact2, score: 0.70 },
            ScoredFact { fact: fact3, score: 0.30 }, // below default hard_min_score of 0.35
        ];

        let ctx = ScoringContext {
            query: "What language does the user prefer?".to_string(),
            query_embedding: None,
            timestamp: now,
            config: ScoringPipelineConfig::default(),
        };

        let pipeline = ScoringPipeline::from_config(&ctx.config);
        let result = pipeline.run(candidates, &ctx);

        // low-score candidate should be filtered out
        // (starts at 0.30, then gets multiplied by importance weight ≈ 0.79,
        //  then time decay ≈ 0.68 → ~0.16 < 0.35 threshold)
        assert!(
            result.iter().all(|r| r.fact.content != "marginal fact"),
            "low-score candidate should have been filtered out"
        );

        // recent+important candidate should rank first
        assert_eq!(
            result[0].fact.content, "User prefers Rust.",
            "recent + important fact should rank first"
        );

        // We should have at most 2 survivors
        assert!(result.len() <= 2, "expected at most 2 results, got {}", result.len());

        // First result should have a meaningfully higher score than second
        if result.len() == 2 {
            assert!(
                result[0].score > result[1].score,
                "first result ({}) should outscore second ({})",
                result[0].score,
                result[1].score
            );
        }
    }
}
