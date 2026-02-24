//! Cosine-rerank blending stage.
//!
//! Interpolates between the original vector-search score and a freshly
//! computed cosine similarity against the query embedding.

use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::ScoringStage;
use crate::memory::store::types::ScoredFact;

/// Compute cosine similarity between two vectors, clamped to `[0, 1]`.
///
/// Returns 0.0 when either vector has zero norm.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have equal length");

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
}

/// Blends vector cosine similarity with the original retrieval score.
pub struct CosineRerankStage;

impl ScoringStage for CosineRerankStage {
    fn name(&self) -> &str {
        "cosine_rerank"
    }

    fn apply(&self, mut candidates: Vec<ScoredFact>, ctx: &ScoringContext) -> Vec<ScoredFact> {
        let query_emb = match ctx.query_embedding.as_ref() {
            Some(emb) => emb,
            None => return candidates, // no query embedding — passthrough
        };

        let blend = ctx.config.rerank_blend;

        for c in &mut candidates {
            if let Some(ref fact_emb) = c.fact.embedding {
                let sim = cosine_similarity(query_emb, fact_emb);
                c.score = (1.0 - blend) * c.score + blend * sim;
            }
            // facts without embedding keep their original score
        }

        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates
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

    fn scored(content: &str, score: f32, embedding: Option<Vec<f32>>) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.embedding = embedding;
        ScoredFact { fact, score }
    }

    fn ctx_with_emb(emb: Vec<f32>, blend: f32) -> ScoringContext {
        ScoringContext {
            query: "test".to_string(),
            query_embedding: Some(emb),
            timestamp: 1700000000,
            config: ScoringPipelineConfig {
                rerank_blend: blend,
                ..Default::default()
            },
        }
    }

    #[test]
    fn identical_vectors_give_similarity_one() {
        let sim = cosine_similarity(&[1.0, 0.0, 0.0], &[1.0, 0.0, 0.0]);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn orthogonal_vectors_give_similarity_zero() {
        let sim = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn no_query_embedding_passthrough() {
        let stage = CosineRerankStage;
        let candidates = vec![scored("a", 0.8, Some(vec![1.0, 0.0])), scored("b", 0.5, Some(vec![0.0, 1.0]))];
        let ctx = ScoringContext {
            query: "test".to_string(),
            query_embedding: None,
            timestamp: 0,
            config: ScoringPipelineConfig::default(),
        };
        let result = stage.apply(candidates, &ctx);
        assert_eq!(result.len(), 2);
        assert!((result[0].score - 0.8).abs() < 1e-5);
        assert!((result[1].score - 0.5).abs() < 1e-5);
    }

    #[test]
    fn blend_reorders_candidates() {
        let stage = CosineRerankStage;
        // Candidate A: low original score but high similarity
        // Candidate B: high original score but low similarity
        let candidates = vec![
            scored("A", 0.3, Some(vec![1.0, 0.0])),
            scored("B", 0.9, Some(vec![0.0, 1.0])),
        ];
        // Query is aligned with A
        let ctx = ctx_with_emb(vec![1.0, 0.0], 0.8);
        let result = stage.apply(candidates, &ctx);
        // A should now rank first: (1-0.8)*0.3 + 0.8*1.0 = 0.06+0.8 = 0.86
        // B: (1-0.8)*0.9 + 0.8*0.0 = 0.18
        assert_eq!(result[0].fact.content, "A");
        assert!((result[0].score - 0.86).abs() < 1e-5);
        assert!((result[1].score - 0.18).abs() < 1e-5);
    }

    #[test]
    fn facts_without_embedding_keep_original_score() {
        let stage = CosineRerankStage;
        let candidates = vec![
            scored("with_emb", 0.5, Some(vec![1.0, 0.0])),
            scored("no_emb", 0.7, None),
        ];
        let ctx = ctx_with_emb(vec![1.0, 0.0], 0.5);
        let result = stage.apply(candidates, &ctx);
        // no_emb keeps 0.7; with_emb: (0.5)*0.5 + 0.5*1.0 = 0.75
        assert_eq!(result[0].fact.content, "with_emb");
        assert!((result[0].score - 0.75).abs() < 1e-5);
        assert!((result[1].score - 0.7).abs() < 1e-5);
    }

    #[test]
    fn zero_norm_vector_gives_zero_similarity() {
        let sim = cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]);
        assert!(sim.abs() < 1e-5);
    }
}
