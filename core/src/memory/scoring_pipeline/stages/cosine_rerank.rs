//! Cosine rerank stage.
//!
//! Blends the original retrieval score with cosine similarity between
//! the query embedding and each fact's embedding.

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::{cosine_similarity, sort_by_score_desc, ScoringStage};
use crate::memory::store::types::ScoredFact;

/// Blends original scores with cosine similarity to the query.
pub struct CosineRerank;

impl ScoringStage for CosineRerank {
    fn name(&self) -> &str {
        "cosine_rerank"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    ) {
        let query_emb = match &ctx.query_embedding {
            Some(e) => e,
            None => return, // no query embedding — skip stage
        };

        let blend = config.rerank_blend;

        for candidate in candidates.iter_mut() {
            if let Some(ref fact_emb) = candidate.fact.embedding {
                let sim = cosine_similarity(query_emb, fact_emb);
                candidate.score = (1.0 - blend) * candidate.score + blend * sim;
            }
            // Facts without embedding keep their original score
        }

        sort_by_score_desc(candidates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_scored_fact(score: f32, embedding: Option<Vec<f32>>, content: &str) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.embedding = embedding;
        ScoredFact { fact, score }
    }

    #[test]
    fn blends_score_with_cosine_similarity() {
        let query_emb = vec![1.0, 0.0, 0.0];
        let ctx = ScoringContext::new(Some(query_emb), 1000);
        let config = ScoringPipelineConfig {
            rerank_blend: 0.5,
            ..Default::default()
        };

        // Fact embedding identical to query → cosine_sim = 1.0
        let mut candidates = vec![make_scored_fact(0.6, Some(vec![1.0, 0.0, 0.0]), "match")];

        CosineRerank.apply(&mut candidates, &ctx, &config);

        // score = 0.5 * 0.6 + 0.5 * 1.0 = 0.8
        assert!((candidates[0].score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn skips_when_no_query_embedding() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig::default();

        let mut candidates = vec![make_scored_fact(0.9, Some(vec![1.0, 0.0]), "a")];
        let original = candidates[0].score;

        CosineRerank.apply(&mut candidates, &ctx, &config);

        assert!((candidates[0].score - original).abs() < 1e-5);
    }

    #[test]
    fn keeps_original_score_for_facts_without_embedding() {
        let query_emb = vec![1.0, 0.0];
        let ctx = ScoringContext::new(Some(query_emb), 1000);
        let config = ScoringPipelineConfig {
            rerank_blend: 0.5,
            ..Default::default()
        };

        let mut candidates = vec![
            make_scored_fact(0.8, None, "no-emb"),
            make_scored_fact(0.5, Some(vec![1.0, 0.0]), "has-emb"),
        ];

        CosineRerank.apply(&mut candidates, &ctx, &config);

        // no-emb keeps 0.8; has-emb = 0.5*0.5 + 0.5*1.0 = 0.75
        let no_emb = candidates.iter().find(|c| c.fact.content == "no-emb").unwrap();
        let has_emb = candidates.iter().find(|c| c.fact.content == "has-emb").unwrap();
        assert!((no_emb.score - 0.8).abs() < 1e-5);
        assert!((has_emb.score - 0.75).abs() < 1e-5);
    }

    #[test]
    fn sorts_by_descending_score_after_blend() {
        let query_emb = vec![1.0, 0.0];
        let ctx = ScoringContext::new(Some(query_emb), 1000);
        let config = ScoringPipelineConfig {
            rerank_blend: 0.8,
            ..Default::default()
        };

        // "low" has high original but orthogonal embedding → low cosine
        // "high" has low original but identical embedding → high cosine
        let mut candidates = vec![
            make_scored_fact(0.9, Some(vec![0.0, 1.0]), "low"),
            make_scored_fact(0.1, Some(vec![1.0, 0.0]), "high"),
        ];

        CosineRerank.apply(&mut candidates, &ctx, &config);

        // low:  0.2*0.9 + 0.8*0.0 = 0.18
        // high: 0.2*0.1 + 0.8*1.0 = 0.82
        assert_eq!(candidates[0].fact.content, "high");
        assert_eq!(candidates[1].fact.content, "low");
    }
}
