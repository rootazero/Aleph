//! Maximal Marginal Relevance (MMR) diversity stage.
//!
//! Greedily selects diverse candidates using cosine similarity.
//! Candidates that are too similar to any already-selected candidate
//! are deferred to the tail of the result list (not dropped).

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Compute cosine similarity between two vectors, clamped to `[0, 1]`.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
}

/// Removes near-duplicate candidates via MMR diversity filtering.
pub struct MmrDiversityStage;

impl ScoringStage for MmrDiversityStage {
    fn name(&self) -> &str {
        "mmr_diversity"
    }

    fn apply(&self, candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let threshold = ctx.config.mmr_similarity_threshold;

        let mut selected: Vec<ScoredFact> = Vec::new();
        let mut deferred: Vec<ScoredFact> = Vec::new();

        for candidate in candidates {
            let candidate_emb = match candidate.fact.embedding.as_ref() {
                Some(emb) => emb,
                None => {
                    // No embedding — cannot compare, treat as diverse
                    selected.push(candidate);
                    continue;
                }
            };

            let too_similar = selected.iter().any(|s| {
                if let Some(ref sel_emb) = s.fact.embedding {
                    cosine_similarity(candidate_emb, sel_emb) > threshold
                } else {
                    false
                }
            });

            if too_similar {
                deferred.push(candidate);
            } else {
                selected.push(candidate);
            }
        }

        selected.extend(deferred);
        selected
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};
    use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
    use crate::memory::scoring_pipeline::context::ScoringContext;

    fn scored_with_emb(content: &str, score: f32, emb: Option<Vec<f32>>) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.embedding = emb;
        ScoredFact { fact, score }
    }

    fn ctx_with_threshold(threshold: f32) -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp: 1700000000,
            config: ScoringPipelineConfig {
                mmr_similarity_threshold: threshold,
                ..Default::default()
            },
        }
    }

    #[test]
    fn identical_embeddings_demoted() {
        let stage = MmrDiversityStage;
        let emb = vec![1.0, 0.0, 0.0];
        let candidates = vec![
            scored_with_emb("A", 0.9, Some(emb.clone())),
            scored_with_emb("B", 0.8, Some(emb.clone())),
            scored_with_emb("C", 0.7, Some(emb.clone())),
        ];
        let ctx = ctx_with_threshold(0.85);
        let result = stage.apply(candidates, &ctx);
        // A is selected first, B and C are deferred (identical to A)
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].fact.content, "A");
        // B and C should be at the end (deferred)
        let deferred_contents: Vec<&str> = result[1..].iter().map(|r| r.fact.content.as_str()).collect();
        assert!(deferred_contents.contains(&"B"));
        assert!(deferred_contents.contains(&"C"));
    }

    #[test]
    fn diverse_embeddings_unchanged() {
        let stage = MmrDiversityStage;
        let candidates = vec![
            scored_with_emb("X", 0.9, Some(vec![1.0, 0.0, 0.0])),
            scored_with_emb("Y", 0.8, Some(vec![0.0, 1.0, 0.0])),
            scored_with_emb("Z", 0.7, Some(vec![0.0, 0.0, 1.0])),
        ];
        let ctx = ctx_with_threshold(0.85);
        let result = stage.apply(candidates, &ctx);
        // All orthogonal — all selected in order
        assert_eq!(result[0].fact.content, "X");
        assert_eq!(result[1].fact.content, "Y");
        assert_eq!(result[2].fact.content, "Z");
    }

    #[test]
    fn no_embeddings_unchanged() {
        let stage = MmrDiversityStage;
        let candidates = vec![
            scored_with_emb("A", 0.9, None),
            scored_with_emb("B", 0.8, None),
        ];
        let ctx = ctx_with_threshold(0.85);
        let result = stage.apply(candidates, &ctx);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].fact.content, "A");
        assert_eq!(result[1].fact.content, "B");
    }

    #[test]
    fn mixed_embeddings_and_none() {
        let stage = MmrDiversityStage;
        let emb = vec![1.0, 0.0];
        let candidates = vec![
            scored_with_emb("A", 0.9, Some(emb.clone())),
            scored_with_emb("B", 0.8, None),
            scored_with_emb("C", 0.7, Some(emb.clone())),
        ];
        let ctx = ctx_with_threshold(0.85);
        let result = stage.apply(candidates, &ctx);
        // A selected, B selected (no embedding), C deferred (similar to A)
        assert_eq!(result[0].fact.content, "A");
        assert_eq!(result[1].fact.content, "B");
        assert_eq!(result[2].fact.content, "C");
    }
}
