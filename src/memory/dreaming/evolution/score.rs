//! Memory-health scoring — the deterministic substrate the evolution gate
//! compares against.
//!
//! `hard` reuses the existing `SignalSnapshot` (zero extra cost): memory is
//! *healthier* when contradiction / duplication / staleness are low and recall
//! hit-rates are high. `soft` is an optional LLM-judge score, populated only
//! when a candidate lands in the gate's epsilon band (so the model is consulted
//! exactly where arithmetic is ambiguous — R7).

use serde::{Deserialize, Serialize};

use crate::memory::dreaming::signals::SignalSnapshot;
use crate::memory::notes::KnowledgeNote;

/// A scored snapshot of memory health at a point in the dream cycle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct MemoryScore {
    /// Deterministic health score in `[0, 1]` (higher = healthier).
    pub hard: f64,
    /// Optional LLM-judge score in `[0, 1]`; `0.0` when not evaluated.
    pub soft: f64,
}

// Weights for the deterministic health projection. Recall quality is rewarded;
// the three "rot" signals are penalties. Tuned so a pristine corpus
// (perfect recall, zero rot) scores ~1.0 and a fully-rotten one ~0.0.
const W_HIT: f64 = 0.45;
const W_SKILL: f64 = 0.15;
const W_CONTRA: f64 = 0.20;
const W_DUP: f64 = 0.12;
const W_STALE: f64 = 0.08;

/// Compute the deterministic memory-health score from a signal snapshot.
///
/// Reuses the normalized `[0,1]` signals already produced each cycle by
/// `SignalSnapshot::from_metrics`, so this adds no new data collection.
#[must_use]
pub fn memory_health_score(snapshot: &SignalSnapshot) -> f64 {
    let hit = snapshot.score("note_hit_rate");
    let skill = snapshot.score("skill_recall_rate");
    let contra = snapshot.score("high_contradiction_rate");
    let dup = snapshot.score("high_duplication_rate");
    let stale = snapshot.score("high_staleness_rate");

    let raw = W_HIT * hit + W_SKILL * skill - W_CONTRA * contra - W_DUP * dup - W_STALE * stale;
    // Shift the reward-minus-penalty range into [0, 1]. Penalty mass is
    // (W_CONTRA + W_DUP + W_STALE) = 0.40; reward mass is (W_HIT + W_SKILL) = 0.60.
    ((raw + (W_CONTRA + W_DUP + W_STALE)) / (W_HIT + W_SKILL + W_CONTRA + W_DUP + W_STALE))
        .clamp(0.0, 1.0)
}

/// Local quality score for a proposed note merge, in `[0, 1]` (higher = safer).
///
/// A merge is *safe* when the two notes substantially overlap (so little
/// distinct information is lost) and share a category. A merge of two notes
/// with almost no shared facts risks collapsing distinct knowledge — the gate
/// should reject those even though an upstream LLM proposed them.
#[must_use]
pub fn score_merge_candidate(keeper: &KnowledgeNote, absorbed: &KnowledgeNote) -> f64 {
    let kf: std::collections::HashSet<&String> = keeper.facts.iter().collect();
    let af: std::collections::HashSet<&String> = absorbed.facts.iter().collect();

    let overlap = kf.intersection(&af).count();
    let union = kf.union(&af).count();

    // Jaccard fact overlap — high overlap means the absorbed note is mostly
    // redundant (a clean merge); low overlap means we'd be fusing distinct
    // knowledge under one title.
    let jaccard = if union == 0 {
        0.0
    } else {
        overlap as f64 / union as f64
    };

    // Distinct-fact-loss guard: if the absorbed note carries many facts the
    // keeper lacks, the merge buries them under an unrelated title. Penalise
    // proportionally to how much of the absorbed note is novel.
    let novel_in_absorbed = af.difference(&kf).count();
    let absorbed_total = absorbed.facts.len().max(1);
    let novelty_ratio = novel_in_absorbed as f64 / absorbed_total as f64;

    // Base score blends redundancy (good) against novelty loss (bad), with a
    // small floor so a category match alone can lift a borderline merge.
    let mut score = 0.5 * jaccard + 0.5 * (1.0 - novelty_ratio);

    // Title affinity: a shared leading token is a strong same-topic signal.
    if share_leading_token(&keeper.title, &absorbed.title) {
        score = (score + 0.15).min(1.0);
    }

    score.clamp(0.0, 1.0)
}

fn share_leading_token(a: &str, b: &str) -> bool {
    let first = |s: &str| -> String {
        s.split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase()
    };
    let fa = first(a);
    !fa.is_empty() && fa == first(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::signals::{RawMetrics, SignalSnapshot};

    fn note(title: &str, facts: &[&str]) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            facts: facts.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn pristine_corpus_scores_high() {
        let m = RawMetrics {
            note_hit_rate: 1.0,
            skill_notes_total: 10,
            skill_notes_recalled: 10,
            ..Default::default()
        };
        let s = memory_health_score(&SignalSnapshot::from_metrics(&m));
        assert!(s > 0.85, "pristine corpus should score high, got {s}");
    }

    #[test]
    fn rotten_corpus_scores_low() {
        let m = RawMetrics {
            note_hit_rate: 0.0,
            contradiction_rate: 1.0,
            duplication_rate: 1.0,
            staleness_rate: 1.0,
            ..Default::default()
        };
        let s = memory_health_score(&SignalSnapshot::from_metrics(&m));
        assert!(s < 0.15, "rotten corpus should score low, got {s}");
    }

    #[test]
    fn redundant_merge_scores_high() {
        let keeper = note("Rust async", &["uses tokio", "await syntax"]);
        let absorbed = note("Rust async patterns", &["uses tokio"]);
        let s = score_merge_candidate(&keeper, &absorbed);
        assert!(s > 0.7, "redundant same-topic merge should be safe, got {s}");
    }

    #[test]
    fn distinct_knowledge_merge_scores_low() {
        let keeper = note("Rust async", &["uses tokio", "await syntax"]);
        let absorbed = note("Postgres tuning", &["raise shared_buffers", "vacuum often"]);
        let s = score_merge_candidate(&keeper, &absorbed);
        assert!(
            s < 0.35,
            "merging distinct knowledge should be unsafe, got {s}"
        );
    }
}
