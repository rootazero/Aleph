//! Score Fusion Strategies
//!
//! Provides Reciprocal Rank Fusion (RRF) and weighted fusion for combining
//! results from vector similarity and full-text search.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Selects the fusion algorithm used to combine ranked lists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum FusionStrategy {
    /// Reciprocal Rank Fusion — rank-based, parameter-light.
    #[default]
    Rrf,
    /// Simple weighted linear combination of normalised scores.
    Weighted,
}

/// A document id paired with its fused score.
pub struct FusedScore {
    pub id: String,
    pub score: f32,
}

/// Reciprocal Rank Fusion with optional BM25 bonus.
///
/// For each document the RRF score is `Σ 1 / (k + rank + 1)` where `rank`
/// is the 0-based position in the source list. Documents appearing in the
/// text results receive a multiplicative boost of `(1 + bm25_bonus)`.
///
/// Scores are normalised to `[0, 1]` by dividing by the maximum score.
///
/// # Arguments
/// * `vector_results` - `(id, score)` pairs from vector search, ordered by rank.
/// * `text_results` - `(id, score)` pairs from full-text search, ordered by rank.
/// * `k` - RRF constant (commonly 60).
/// * `bm25_bonus` - Additive bonus factor for text-matched documents (e.g. 0.2).
pub fn rrf_fuse(
    vector_results: &[(String, f32)],
    text_results: &[(String, f32)],
    k: u32,
    bm25_bonus: f32,
) -> Vec<FusedScore> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    let text_ids: std::collections::HashSet<&str> =
        text_results.iter().map(|(id, _)| id.as_str()).collect();

    // Accumulate RRF scores from vector results
    for (rank, (id, _)) in vector_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += 1.0 / (k as f32 + rank as f32 + 1.0);
    }

    // Accumulate RRF scores from text results
    for (rank, (id, _)) in text_results.iter().enumerate() {
        *scores.entry(id.clone()).or_default() += 1.0 / (k as f32 + rank as f32 + 1.0);
    }

    // Apply BM25 bonus to documents present in text results
    for (id, score) in &mut scores {
        if text_ids.contains(id.as_str()) {
            *score *= 1.0 + bm25_bonus;
        }
    }

    // Normalise to [0, 1]
    let max_score = scores.values().cloned().fold(0.0_f32, f32::max);
    let mut results: Vec<FusedScore> = scores
        .into_iter()
        .map(|(id, s)| FusedScore {
            id,
            score: if max_score > 0.0 { s / max_score } else { 0.0 },
        })
        .collect();

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results
}

/// Weighted linear fusion.
///
/// For each document: `score = vector_weight * vec_score + text_weight * text_score`.
/// Documents appearing in only one source get 0 for the missing score.
///
/// # Arguments
/// * `vector_results` - `(id, score)` pairs from vector search.
/// * `text_results` - `(id, score)` pairs from full-text search.
/// * `vector_weight` - Weight applied to vector scores.
/// * `text_weight` - Weight applied to text scores.
pub fn weighted_fuse(
    vector_results: &[(String, f32)],
    text_results: &[(String, f32)],
    vector_weight: f32,
    text_weight: f32,
) -> Vec<FusedScore> {
    let mut vec_map: HashMap<String, f32> = HashMap::new();
    let mut text_map: HashMap<String, f32> = HashMap::new();

    for (id, score) in vector_results {
        vec_map.insert(id.clone(), *score);
    }
    for (id, score) in text_results {
        text_map.insert(id.clone(), *score);
    }

    // Collect all unique ids
    let mut all_ids: std::collections::HashSet<String> = vec_map.keys().cloned().collect();
    all_ids.extend(text_map.keys().cloned());

    let mut results: Vec<FusedScore> = all_ids
        .into_iter()
        .map(|id| {
            let vs = vec_map.get(&id).copied().unwrap_or(0.0);
            let ts = text_map.get(&id).copied().unwrap_or(0.0);
            FusedScore {
                id,
                score: vector_weight * vs + text_weight * ts,
            }
        })
        .collect();

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fuse_combines_both_sources() {
        // "a" and "b" appear in both sources; "c" and "d" in one each.
        let vec_results = vec![
            ("a".into(), 0.9),
            ("b".into(), 0.8),
            ("c".into(), 0.7),
        ];
        let text_results = vec![
            ("b".into(), 0.9),
            ("a".into(), 0.7),
            ("d".into(), 0.5),
        ];

        let fused = rrf_fuse(&vec_results, &text_results, 60, 0.0);

        // Documents in both lists should rank highest
        assert!(fused.len() >= 2);
        let top2_ids: Vec<&str> = fused.iter().take(2).map(|f| f.id.as_str()).collect();
        assert!(top2_ids.contains(&"a"));
        assert!(top2_ids.contains(&"b"));

        // All scores in [0, 1]
        for f in &fused {
            assert!(f.score >= 0.0 && f.score <= 1.0, "score {} out of range", f.score);
        }
    }

    #[test]
    fn rrf_fuse_empty_text_returns_vector_only() {
        let vec_results = vec![
            ("x".into(), 0.9),
            ("y".into(), 0.5),
        ];
        let text_results: Vec<(String, f32)> = vec![];

        let fused = rrf_fuse(&vec_results, &text_results, 60, 0.0);

        assert_eq!(fused.len(), 2);
        let ids: Vec<&str> = fused.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"x"));
        assert!(ids.contains(&"y"));
    }

    #[test]
    fn weighted_fuse_applies_weights() {
        let vec_results = vec![("doc1".into(), 1.0_f32)];
        let text_results = vec![("doc1".into(), 0.5_f32)];

        let fused = weighted_fuse(&vec_results, &text_results, 0.7, 0.3);

        assert_eq!(fused.len(), 1);
        let expected = 0.7 * 1.0 + 0.3 * 0.5; // 0.85
        assert!(
            (fused[0].score - expected).abs() < 1e-5,
            "expected {expected}, got {}",
            fused[0].score
        );
    }

    #[test]
    fn bm25_bonus_boosts_text_hits() {
        // "a" only in vector; "b" in both vector and text.
        // Without bonus they would have similar RRF scores, but bonus should
        // push "b" above "a".
        let vec_results = vec![
            ("a".into(), 0.9),
            ("b".into(), 0.8),
        ];
        let text_results = vec![
            ("b".into(), 0.9),
        ];

        let fused = rrf_fuse(&vec_results, &text_results, 60, 0.5);

        // "b" should be ranked first because it appears in text and gets
        // both dual-list RRF contribution and the BM25 bonus multiplier.
        assert_eq!(fused[0].id, "b");
    }
}
