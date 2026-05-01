//! Deterministic skeleton fallback (strategy C) — used when the LLM re-rank
//! path times out, returns invalid JSON, yields no valid slots, or when the
//! candidate pool is too small to be worth asking the LLM about.

use super::envelope::{EnvelopeItem, EnvelopeSlot, ItemSource, SlotKind};
use crate::config::types::memory::FallbackSkeleton;
use crate::memory::context::FactSource;

/// An un-rendered candidate before hydration — kept internal to the
/// assembler module. Produced by `gather` (Task 7), consumed here and by
/// `rerank` (Task 8).
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub id: String,
    pub title: String,
    pub full_content: String,
    pub source: ItemSource,
    pub relevance: f32,
    pub updated_at: i64,
    pub slot_hint: SlotKind,
    /// Semantic source classification used by [`FactSourceFilter`] to
    /// post-filter the candidate pool. Defaults to [`FactSource::Extracted`]
    /// for all non-SessionCompressed raw memories and for note results.
    pub fact_source: FactSource,
}

/// Pack `candidates` into skeleton slots using fixed budgets and the
/// `(relevance * recency_factor)` greedy strategy. Content is NOT truncated
/// here — only item selection happens. Hydration trims content against the
/// per-slot budget.
pub(crate) fn skeleton_pack(
    candidates: &[Candidate],
    skeleton: &FallbackSkeleton,
    now: i64,
) -> Vec<EnvelopeSlot> {
    let mut slots = Vec::new();
    for (kind, budget) in [
        (SlotKind::UserProfile, skeleton.user_profile_tokens),
        (SlotKind::SessionRecent, skeleton.session_recent_tokens),
        (SlotKind::RelevantNotes, skeleton.relevant_notes_tokens),
        (SlotKind::RawFragments, skeleton.raw_fragments_tokens),
        (SlotKind::Nudges, skeleton.nudges_tokens),
    ] {
        if budget == 0 {
            continue;
        }
        let mut in_slot: Vec<&Candidate> =
            candidates.iter().filter(|c| c.slot_hint == kind).collect();
        in_slot.sort_by(|a, b| {
            let sa = a.relevance * recency_factor(a.updated_at, now);
            let sb = b.relevance * recency_factor(b.updated_at, now);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        let items: Vec<EnvelopeItem> = in_slot
            .into_iter()
            .map(|c| EnvelopeItem {
                id: c.id.clone(),
                title: c.title.clone(),
                content: c.full_content.clone(), // hydration truncates later
                source: c.source.clone(),
                relevance: c.relevance,
                tokens: 0, // set by hydration
                updated_at: c.updated_at,
                extra: serde_json::Map::new(),
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        slots.push(EnvelopeSlot {
            kind,
            items,
            tokens_used: 0,
            tokens_budget: budget,
        });
    }
    slots
}

pub(crate) fn recency_factor(updated_at: i64, now: i64) -> f32 {
    let age_days = ((now - updated_at).max(0) as f32) / 86_400.0;
    0.5 + 0.5 * (-age_days / 14.0).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: &str, slot: SlotKind, rel: f32, updated: i64) -> Candidate {
        Candidate {
            id: id.into(),
            title: id.into(),
            full_content: format!("body of {id}"),
            source: ItemSource::Note {
                path: id.trim_start_matches("note://").into(),
                category: "reference".into(),
            },
            relevance: rel,
            updated_at: updated,
            slot_hint: slot,
            fact_source: FactSource::Extracted,
        }
    }

    #[test]
    fn empty_pool_yields_no_slots() {
        let skel = FallbackSkeleton::default();
        assert!(skeleton_pack(&[], &skel, 1_700_000_000).is_empty());
    }

    #[test]
    fn items_sorted_by_relevance_within_slot() {
        let now = 1_700_000_000;
        let c = [
            cand("note://reference/a", SlotKind::RelevantNotes, 0.3, now),
            cand("note://reference/b", SlotKind::RelevantNotes, 0.9, now),
            cand("note://reference/c", SlotKind::RelevantNotes, 0.5, now),
        ];
        let slots = skeleton_pack(&c, &FallbackSkeleton::default(), now);
        let rel_slot = slots
            .iter()
            .find(|s| s.kind == SlotKind::RelevantNotes)
            .unwrap();
        assert_eq!(rel_slot.items[0].id, "note://reference/b");
        assert_eq!(rel_slot.items[1].id, "note://reference/c");
        assert_eq!(rel_slot.items[2].id, "note://reference/a");
    }

    #[test]
    fn zero_budget_slot_is_excluded() {
        let mut skel = FallbackSkeleton::default();
        skel.relevant_notes_tokens = 0;
        let now = 1_700_000_000;
        let c = [cand(
            "note://reference/a",
            SlotKind::RelevantNotes,
            0.9,
            now,
        )];
        let slots = skeleton_pack(&c, &skel, now);
        assert!(slots.iter().all(|s| s.kind != SlotKind::RelevantNotes));
    }

    #[test]
    fn recency_factor_bounded() {
        let now = 1_700_000_000;
        assert!((recency_factor(now, now) - 1.0).abs() < 1e-6);
        assert!(recency_factor(now - 86_400 * 10_000, now) >= 0.5);
    }
}
