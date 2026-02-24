//! Scoring pipeline for memory retrieval.
//!
//! The pipeline applies a sequence of scoring stages to a list of
//! `ScoredFact` candidates. Each stage may adjust scores, filter
//! candidates, or reorder them.
//!
//! ## Default pipeline order
//!
//! 1. **CosineRerank** — blend original score with query similarity
//! 2. **RecencyBoost** — additive boost for recent facts
//! 3. **ImportanceWeight** — scale by fact confidence
//! 4. **LengthNormalization** — penalize verbose content
//! 5. **TimeDecay** — multiplicative decay for old facts
//! 6. **HardMinScore** — discard low-scoring candidates
//! 7. **MmrDiversity** — demote near-duplicate results

pub mod config;
pub mod context;
pub mod stages;

use config::ScoringPipelineConfig;
use context::ScoringContext;
use stages::cosine_rerank::CosineRerank;
use stages::hard_min_score::HardMinScore;
use stages::importance_weight::ImportanceWeight;
use stages::length_normalization::LengthNormalization;
use stages::mmr_diversity::MmrDiversity;
use stages::recency_boost::RecencyBoost;
use stages::time_decay::TimeDecay;
use stages::ScoringStage;

use crate::memory::store::types::ScoredFact;

/// A configurable pipeline of scoring stages.
pub struct ScoringPipeline {
    stages: Vec<Box<dyn ScoringStage>>,
    config: ScoringPipelineConfig,
}

impl ScoringPipeline {
    /// Create a pipeline with the default 7-stage sequence.
    pub fn new(config: ScoringPipelineConfig) -> Self {
        let stages: Vec<Box<dyn ScoringStage>> = vec![
            Box::new(CosineRerank),
            Box::new(RecencyBoost),
            Box::new(ImportanceWeight),
            Box::new(LengthNormalization),
            Box::new(TimeDecay),
            Box::new(HardMinScore),
            Box::new(MmrDiversity),
        ];
        Self { stages, config }
    }

    /// Create a pipeline with a custom set of stages.
    pub fn with_stages(
        stages: Vec<Box<dyn ScoringStage>>,
        config: ScoringPipelineConfig,
    ) -> Self {
        Self { stages, config }
    }

    /// Run all stages in order, mutating candidates in place.
    pub fn run(&self, candidates: &mut Vec<ScoredFact>, ctx: &ScoringContext) {
        for stage in &self.stages {
            stage.apply(candidates, ctx, &self.config);
        }
    }

    /// Return the names of all stages in order.
    pub fn stage_names(&self) -> Vec<&str> {
        self.stages.iter().map(|s| s.name()).collect()
    }
}

impl Default for ScoringPipeline {
    fn default() -> Self {
        Self::new(ScoringPipelineConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    /// Helper to create a `ScoredFact` with control over key fields.
    fn make_fact(
        content: &str,
        score: f32,
        confidence: f32,
        created_at: i64,
        embedding: Option<Vec<f32>>,
    ) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.confidence = confidence;
        fact.created_at = created_at;
        fact.embedding = embedding;
        ScoredFact { fact, score }
    }

    #[test]
    fn test_default_pipeline_has_7_stages() {
        let pipeline = ScoringPipeline::default();
        let names = pipeline.stage_names();
        assert_eq!(names.len(), 7);
        assert_eq!(names[0], "cosine_rerank");
        assert_eq!(names[1], "recency_boost");
        assert_eq!(names[2], "importance_weight");
        assert_eq!(names[3], "length_normalization");
        assert_eq!(names[4], "time_decay");
        assert_eq!(names[5], "hard_min_score");
        assert_eq!(names[6], "mmr_diversity");
    }

    #[test]
    fn test_empty_candidates_no_panic() {
        let pipeline = ScoringPipeline::default();
        let ctx = ScoringContext::new(Some(vec![1.0, 0.0, 0.0]), 1_700_000_000);
        let mut candidates = vec![];
        pipeline.run(&mut candidates, &ctx);
        assert!(candidates.is_empty());
    }

    #[test]
    fn test_full_pipeline_end_to_end() {
        // Current time
        let now = 1_700_000_000i64;
        let query_emb = vec![1.0, 0.0, 0.0];

        // Candidate 1: recent + important + short + matching embedding
        let recent_important = make_fact(
            "Recent important fact",     // short content
            0.8,                         // good initial score
            1.0,                         // high confidence
            now - 86400,                 // 1 day ago
            Some(vec![0.9, 0.1, 0.0]),   // similar to query
        );

        // Candidate 2: old + unimportant + verbose content + different embedding
        let verbose_content = "x".repeat(2000); // very long
        let old_verbose = make_fact(
            &verbose_content,
            0.7,                         // decent initial score
            0.2,                         // low confidence
            now - 365 * 86400,           // 1 year ago
            Some(vec![0.0, 0.0, 1.0]),   // orthogonal to query
        );

        // Candidate 3: low score — should be filtered by HardMinScore
        let low_score = make_fact(
            "Low score fact",
            0.1,                         // very low score
            0.5,                         // medium confidence
            now - 30 * 86400,            // 30 days ago
            Some(vec![0.5, 0.5, 0.0]),   // somewhat similar
        );

        let mut candidates = vec![
            old_verbose,
            recent_important,
            low_score,
        ];

        let config = ScoringPipelineConfig::default();
        let pipeline = ScoringPipeline::new(config);
        let ctx = ScoringContext::new(Some(query_emb), now);

        pipeline.run(&mut candidates, &ctx);

        // The recent+important fact should rank first
        assert_eq!(
            candidates[0].fact.content, "Recent important fact",
            "Recent+important fact should rank first"
        );

        // The low-score fact should be filtered out by HardMinScore
        // (its score after cosine rerank with blend=0.3: (0.7*0.1 + 0.3*~0.7) ≈ 0.28,
        //  then further reduced by importance, time_decay, length — definitely < 0.35)
        assert!(
            !candidates.iter().any(|c| c.fact.content == "Low score fact"),
            "Low-score fact should be filtered out"
        );

        // All remaining candidates should have scores >= hard_min_score
        for c in &candidates {
            assert!(
                c.score >= 0.0,
                "All remaining scores should be non-negative"
            );
        }
    }

    #[test]
    fn test_custom_pipeline_single_stage() {
        let stages: Vec<Box<dyn ScoringStage>> = vec![Box::new(HardMinScore)];
        let config = ScoringPipelineConfig {
            hard_min_score: 0.5,
            ..Default::default()
        };
        let pipeline = ScoringPipeline::with_stages(stages, config);
        let ctx = ScoringContext::new(None, 1000);

        let mut candidates = vec![
            make_fact("high", 0.9, 1.0, 1000, None),
            make_fact("low", 0.3, 1.0, 1000, None),
        ];

        pipeline.run(&mut candidates, &ctx);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].fact.content, "high");
    }
}
