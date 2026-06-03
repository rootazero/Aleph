//! Retrieval-time score adjustments: recency decay + MMR diversity.
//!
//! These are *deterministic ranking refinements* applied after fusion/rerank
//! and before truncation. They close two gaps that reference memory systems
//! (hermes-agent, openclaw) apply at recall time but Aleph previously only
//! applied in the background dream daemon:
//!
//! * **Recency decay** — a fresh note should outrank a stale one of equal raw
//!   relevance. Reuses the exponential half-life decay semantics already used by
//!   the note-decay dream stage (`0.5^(age_days / half_life_days)`).
//! * **MMR diversity** — pure relevance ordering returns near-duplicate notes;
//!   Maximal Marginal Relevance trades a little relevance for coverage. We use
//!   token-Jaccard overlap as the redundancy proxy so no per-candidate
//!   embedding round-trip is required (the hybrid-search candidates do not carry
//!   their vectors).
//!
//! All functions are pure (time is injected as `now`) so they are trivially
//! testable and never call the clock themselves.

use std::collections::HashSet;

/// Exponential recency multiplier in `(0.0, 1.0]`: `0.5^(age_days / half_life_days)`.
///
/// A note updated *now* scores `1.0`; after one half-life it scores `0.5`, after
/// two `0.25`, and so on. A non-positive `half_life_days` disables decay
/// (returns `1.0`) so misconfiguration can never zero out results.
pub fn recency_multiplier(updated_at: i64, now: i64, half_life_days: f32) -> f32 {
    if half_life_days <= 0.0 {
        return 1.0;
    }
    let age_days = ((now - updated_at).max(0) as f32) / 86_400.0;
    0.5_f32.powf(age_days / half_life_days)
}

/// Blend a relevance `score` with its recency `mult` under `weight` in `[0,1]`:
/// `score * (1 - w + w * mult)`.
///
/// `weight == 0.0` leaves the score untouched (legacy behaviour); `weight == 1.0`
/// multiplies the score fully by the recency multiplier. Intermediate weights let
/// recency nudge ordering without dominating raw relevance.
pub fn apply_recency(score: f32, mult: f32, weight: f32) -> f32 {
    let w = weight.clamp(0.0, 1.0);
    score * (1.0 - w + w * mult)
}

/// Reinforcement (recall-frequency) salience boost.
///
/// Multiplies a relevance `score` by `1 + w * ln(1 + hit_count)`, so a note that
/// has been recalled many times outranks an equally-relevant one that never has.
/// The `ln(1 + hit_count)` shape grows sub-linearly (0 hits → ×1.0, 1 → ×1.21 at
/// w=0.3, 10 → ×1.72, 50 → ×2.18) so frequency *nudges* ordering without ever
/// dominating raw relevance. Inspired by memU's `sim × log(reinforcement+1)`.
///
/// `weight == 0.0` or `hit_count <= 0` leaves the score untouched (legacy
/// behaviour). Reads counts already recorded in `recall_signals`, so no extra
/// LLM or embedding call is required.
pub fn apply_reinforcement(score: f32, hit_count: i64, weight: f32) -> f32 {
    if weight <= 0.0 || hit_count <= 0 {
        return score;
    }
    let w = weight.clamp(0.0, 1.0);
    let boost = (hit_count as f32).ln_1p(); // ln(1 + hit_count)
    score * (1.0 + w * boost)
}

/// Lowercase whitespace-token set, dropping tokens shorter than 3 chars to skip
/// stopword noise. Used as a cheap content-similarity proxy for MMR.
fn token_set(content: &str) -> HashSet<&str> {
    content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .collect()
}

/// Jaccard similarity of two pre-tokenised sets in `[0,1]`. Two empty sets are
/// treated as maximally dissimilar (`0.0`) so empty content never crowds out
/// real candidates.
fn jaccard(a: &HashSet<&str>, b: &HashSet<&str>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = (a.len() + b.len()) as f32 - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Maximal Marginal Relevance reordering.
///
/// Greedily selects items maximising `lambda * relevance - (1 - lambda) * maxSim`,
/// where `maxSim` is the highest token-Jaccard overlap with already-selected
/// items. `lambda` in `[0,1]`: `1.0` = pure relevance (identity order on a
/// relevance-sorted input), `0.0` = pure diversity.
///
/// `contents[i]` and `relevance[i]` must align. Returns a permutation of the
/// input indices. Inputs of length < 2 are returned in original order.
pub fn mmr_reorder(contents: &[String], relevance: &[f32], lambda: f32) -> Vec<usize> {
    let n = contents.len();
    if n < 2 || contents.len() != relevance.len() {
        return (0..n).collect();
    }
    let lambda = lambda.clamp(0.0, 1.0);
    let tokens: Vec<HashSet<&str>> = contents.iter().map(|c| token_set(c)).collect();

    let mut remaining: Vec<usize> = (0..n).collect();
    let mut selected: Vec<usize> = Vec::with_capacity(n);

    while !remaining.is_empty() {
        let mut best_pos = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for (pos, &idx) in remaining.iter().enumerate() {
            let max_sim = selected
                .iter()
                .map(|&s| jaccard(&tokens[idx], &tokens[s]))
                .fold(0.0_f32, f32::max);
            let mmr = lambda * relevance[idx] - (1.0 - lambda) * max_sim;
            if mmr > best_score {
                best_score = mmr;
                best_pos = pos;
            }
        }
        selected.push(remaining.remove(best_pos));
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn recency_multiplier_is_one_when_fresh() {
        assert!((recency_multiplier(1000, 1000, 90.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recency_multiplier_halves_at_one_half_life() {
        let now = 100 * DAY;
        let updated = now - 90 * DAY;
        let m = recency_multiplier(updated, now, 90.0);
        assert!(
            (m - 0.5).abs() < 1e-3,
            "expected ~0.5 at one half-life, got {m}"
        );
    }

    #[test]
    fn recency_multiplier_disabled_for_nonpositive_half_life() {
        assert_eq!(recency_multiplier(0, 100 * DAY, 0.0), 1.0);
        assert_eq!(recency_multiplier(0, 100 * DAY, -5.0), 1.0);
    }

    #[test]
    fn recency_multiplier_clamps_negative_age_to_one() {
        // updated_at in the future → age clamped to 0 → multiplier 1.0
        assert!((recency_multiplier(200 * DAY, 100 * DAY, 90.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn apply_recency_zero_weight_is_identity() {
        assert!((apply_recency(0.8, 0.1, 0.0) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn apply_recency_full_weight_multiplies() {
        assert!((apply_recency(0.8, 0.5, 1.0) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn apply_recency_partial_weight_blends() {
        // 0.8 * (1 - 0.5 + 0.5 * 0.5) = 0.8 * 0.75 = 0.6
        assert!((apply_recency(0.8, 0.5, 0.5) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn reinforcement_zero_weight_is_identity() {
        assert!((apply_reinforcement(0.8, 50, 0.0) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn reinforcement_zero_hits_is_identity() {
        assert!((apply_reinforcement(0.8, 0, 0.3) - 0.8).abs() < 1e-6);
        assert!((apply_reinforcement(0.8, -1, 0.3) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn reinforcement_boosts_frequently_recalled() {
        // ln(1+10) = 2.3979; 0.8 * (1 + 0.3 * 2.3979) = 0.8 * 1.7194 = 1.3755
        let boosted = apply_reinforcement(0.8, 10, 0.3);
        assert!((boosted - 1.3755).abs() < 1e-3, "got {boosted}");
        // More hits → strictly larger boost, but sub-linear.
        assert!(apply_reinforcement(0.8, 50, 0.3) > boosted);
    }

    #[test]
    fn reinforcement_promotes_hot_note_over_equal_relevance() {
        // Two equally-relevant notes; the one recalled more often should outscore.
        let cold = apply_reinforcement(0.7, 0, 0.3);
        let hot = apply_reinforcement(0.7, 20, 0.3);
        assert!(hot > cold);
    }

    #[test]
    fn mmr_pure_relevance_preserves_order() {
        let contents = vec!["alpha one".to_string(), "beta two".to_string()];
        let relevance = vec![0.9, 0.5];
        assert_eq!(mmr_reorder(&contents, &relevance, 1.0), vec![0, 1]);
    }

    #[test]
    fn mmr_demotes_near_duplicate() {
        // Items 0 and 1 are near-identical; item 2 is distinct but lower scored.
        // With balanced lambda, the diverse item 2 should be promoted above the
        // redundant duplicate item 1.
        let contents = vec![
            "rust async tokio runtime scheduler".to_string(),
            "rust async tokio runtime scheduler details".to_string(),
            "python pandas dataframe analysis".to_string(),
        ];
        let relevance = vec![0.95, 0.90, 0.60];
        let order = mmr_reorder(&contents, &relevance, 0.5);
        assert_eq!(order[0], 0, "top relevance stays first");
        assert_eq!(
            order[1], 2,
            "diverse item promoted over redundant duplicate"
        );
        assert_eq!(order[2], 1, "redundant duplicate demoted");
    }

    #[test]
    fn mmr_short_input_unchanged() {
        let contents = vec!["solo".to_string()];
        let relevance = vec![0.5];
        assert_eq!(mmr_reorder(&contents, &relevance, 0.5), vec![0]);
    }

    #[test]
    fn mmr_mismatched_lengths_unchanged() {
        let contents = vec!["a".to_string(), "b".to_string()];
        let relevance = vec![0.5];
        assert_eq!(mmr_reorder(&contents, &relevance, 0.5), vec![0, 1]);
    }
}
