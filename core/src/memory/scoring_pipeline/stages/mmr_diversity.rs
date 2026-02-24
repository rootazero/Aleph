//! MMR (Maximal Marginal Relevance) diversity stage.
//!
//! Greedily selects candidates while demoting those that are too
//! similar to already-selected ones. Demoted candidates are appended
//! at the end rather than removed.

use crate::memory::scoring_pipeline::config::ScoringPipelineConfig;
use crate::memory::scoring_pipeline::context::ScoringContext;
use crate::memory::scoring_pipeline::stages::{cosine_similarity, ScoringStage};
use crate::memory::store::types::ScoredFact;

/// Greedy MMR diversity filter.
///
/// Iterates candidates in order. For each candidate, computes cosine
/// similarity against all already-selected candidates. If any similarity
/// exceeds the threshold, the candidate is deferred (appended at the end).
pub struct MmrDiversity;

impl ScoringStage for MmrDiversity {
    fn name(&self) -> &str {
        "mmr_diversity"
    }

    fn apply(
        &self,
        candidates: &mut Vec<ScoredFact>,
        _ctx: &ScoringContext,
        config: &ScoringPipelineConfig,
    ) {
        if candidates.len() <= 1 {
            return;
        }

        let threshold = config.mmr_similarity_threshold;

        let mut selected: Vec<ScoredFact> = Vec::with_capacity(candidates.len());
        let mut deferred: Vec<ScoredFact> = Vec::new();

        // Drain all candidates
        let all: Vec<ScoredFact> = candidates.drain(..).collect();

        for candidate in all {
            let is_too_similar = candidate.fact.embedding.as_ref().map_or(false, |cand_emb| {
                selected.iter().any(|sel| {
                    sel.fact
                        .embedding
                        .as_ref()
                        .map_or(false, |sel_emb| cosine_similarity(cand_emb, sel_emb) > threshold)
                })
            });

            if is_too_similar {
                deferred.push(candidate);
            } else {
                selected.push(candidate);
            }
        }

        // Rebuild: selected first, then deferred
        selected.extend(deferred);
        *candidates = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_fact_with_emb(score: f32, embedding: Vec<f32>, content: &str) -> ScoredFact {
        let mut fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        fact.embedding = Some(embedding);
        ScoredFact { fact, score }
    }

    fn make_fact_no_emb(score: f32, content: &str) -> ScoredFact {
        let fact = MemoryFact::new(content.to_string(), FactType::Other, vec![]);
        ScoredFact { fact, score }
    }

    #[test]
    fn defers_near_duplicate() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            mmr_similarity_threshold: 0.85,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact_with_emb(0.9, vec![1.0, 0.0, 0.0], "first"),
            make_fact_with_emb(0.8, vec![1.0, 0.01, 0.0], "near-dup"), // very similar to first
            make_fact_with_emb(0.7, vec![0.0, 1.0, 0.0], "different"),
        ];

        MmrDiversity.apply(&mut candidates, &ctx, &config);

        // "first" and "different" should come first; "near-dup" deferred
        assert_eq!(candidates[0].fact.content, "first");
        assert_eq!(candidates[1].fact.content, "different");
        assert_eq!(candidates[2].fact.content, "near-dup");
    }

    #[test]
    fn orthogonal_candidates_unchanged() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            mmr_similarity_threshold: 0.85,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact_with_emb(0.9, vec![1.0, 0.0], "a"),
            make_fact_with_emb(0.8, vec![0.0, 1.0], "b"),
        ];

        MmrDiversity.apply(&mut candidates, &ctx, &config);

        assert_eq!(candidates[0].fact.content, "a");
        assert_eq!(candidates[1].fact.content, "b");
    }

    #[test]
    fn facts_without_embedding_never_deferred() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig {
            mmr_similarity_threshold: 0.85,
            ..Default::default()
        };

        let mut candidates = vec![
            make_fact_with_emb(0.9, vec![1.0, 0.0], "with-emb"),
            make_fact_no_emb(0.8, "no-emb"),
        ];

        MmrDiversity.apply(&mut candidates, &ctx, &config);

        // no-emb cannot be compared → not deferred
        assert_eq!(candidates[0].fact.content, "with-emb");
        assert_eq!(candidates[1].fact.content, "no-emb");
    }

    #[test]
    fn single_candidate_unchanged() {
        let ctx = ScoringContext::new(None, 1000);
        let config = ScoringPipelineConfig::default();

        let mut candidates = vec![make_fact_with_emb(0.9, vec![1.0], "only")];
        MmrDiversity.apply(&mut candidates, &ctx, &config);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].fact.content, "only");
    }
}
