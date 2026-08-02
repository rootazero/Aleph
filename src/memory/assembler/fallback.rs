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

/// Pack `candidates` into skeleton slots using the
/// `(relevance * recency_factor)` greedy strategy. Content is NOT truncated
/// here — only item selection happens. Hydration trims content against the
/// per-slot budget.
///
/// Deterministic slot packing used whenever the LLM re-rank is unavailable
/// (no reranker configured, tiny pool, timeout, provider error, unparseable
/// response). With no rerank provider this is 100% of traffic.
///
/// `total_budget` is the caller's runtime headroom. The `FallbackSkeleton`
/// figures are *static defaults* summing to ~8200 tokens; without clamping them
/// against the live budget this path emitted up to 8200 tokens regardless of
/// how little room the turn actually had, making the context-pressure back-off
/// a no-op on the dominant path (a budget of 0 — "inject nothing this turn" —
/// still produced a full envelope). Uses the same 70% cap and `.max(1)` floor
/// as the LLM path's `rerank::parse_response`, so both paths bound identically.
pub(crate) fn skeleton_pack(
    candidates: &[Candidate],
    skeleton: &FallbackSkeleton,
    total_budget: u32,
    now: i64,
) -> Vec<EnvelopeSlot> {
    let mut budgets = [
        (SlotKind::Feedback, skeleton.feedback_tokens),
        (SlotKind::UserProfile, skeleton.user_profile_tokens),
        (SlotKind::SessionRecent, skeleton.session_recent_tokens),
        (SlotKind::RelevantNotes, skeleton.relevant_notes_tokens),
        (SlotKind::RawFragments, skeleton.raw_fragments_tokens),
        (SlotKind::Nudges, skeleton.nudges_tokens),
    ];
    let sum: u64 = budgets.iter().map(|(_, b)| u64::from(*b)).sum();
    let cap = ((f64::from(total_budget)) * 0.7) as u64;
    if sum > cap {
        let scale = cap as f32 / sum.max(1) as f32;
        for (_, b) in &mut budgets {
            // 0 stays 0 (slot disabled by config); a live slot floors at 1 so
            // hydration cannot silently delete it — mirrors rerank.rs.
            if *b > 0 {
                *b = (((*b as f32) * scale).floor() as u32).max(1);
            }
        }
    }

    let mut slots = Vec::new();
    for (kind, budget) in budgets {
        if budget == 0 {
            continue;
        }
        let mut in_slot: Vec<&Candidate> =
            candidates.iter().filter(|c| c.slot_hint == kind).collect();
        sort_by_pinned_relevance(&mut in_slot, now);
        let items: Vec<EnvelopeItem> = in_slot
            .into_iter()
            .map(|c| EnvelopeItem {
                // rust-doctor-disable-next-line excessive-clone
                id: c.id.clone(),
                // rust-doctor-disable-next-line excessive-clone
                title: c.title.clone(),
                // rust-doctor-disable-next-line excessive-clone
                content: c.full_content.clone(), // hydration truncates later
                // rust-doctor-disable-next-line excessive-clone
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

/// Order candidates within one slot by recency-weighted relevance, descending.
///
/// Both assembly paths depend on this: `hydrate` charges each slot's token
/// budget strictly in item order and drops whatever truncates to empty, so
/// position *is* priority. Pinned entries (the always-on High/Critical feedback
/// floor, the user profile) carry `relevance: 1.0` while RRF retrieval scores
/// are far below 1.0, which is what keeps a query match from evicting a
/// standing rule.
pub(crate) fn sort_by_pinned_relevance(candidates: &mut [&Candidate], now: i64) {
    candidates.sort_by(|a, b| {
        let sa = a.relevance * recency_factor(a.updated_at, now);
        let sb = b.relevance * recency_factor(b.updated_at, now);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
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

    /// The always-on High/Critical feedback floor pins `relevance: 1.0` while
    /// query matches carry RRF scores far below 1.0. `gather` pushes retrieval
    /// matches into the pool BEFORE the floor, and `hydrate` charges the slot
    /// budget strictly in item order — so without this ordering the standing
    /// rule the floor exists to guarantee is the first thing evicted. The
    /// pinned entry is deliberately the OLDEST here: recency must not be able
    /// to overturn the pin.
    #[test]
    fn pinned_floor_entry_outranks_query_matches_even_when_older() {
        let now = 1_700_000_000;
        let month_ago = now - 30 * 86_400;
        let mut c = [
            &cand(
                "note://feedback/fresh-match-a",
                SlotKind::Feedback,
                0.02,
                now,
            ),
            &cand(
                "note://feedback/fresh-match-b",
                SlotKind::Feedback,
                0.03,
                now,
            ),
            &cand(
                "note://feedback/never-force-push",
                SlotKind::Feedback,
                1.0,
                month_ago,
            ),
        ];
        sort_by_pinned_relevance(&mut c, now);
        assert_eq!(
            c[0].id, "note://feedback/never-force-push",
            "a pinned floor entry must take the slot budget before query matches"
        );
    }

    /// A zero runtime budget must not yield a full static envelope.
    #[test]
    fn skeleton_budgets_scale_down_to_the_runtime_budget() {
        let now = 1_700_000_000;
        let c = [
            cand("note://reference/a", SlotKind::RelevantNotes, 0.9, now),
            cand("note://feedback/x", SlotKind::Feedback, 1.0, now),
        ];
        let slots = skeleton_pack(&c, &FallbackSkeleton::default(), 300, now);
        let total: u32 = slots.iter().map(|s| s.tokens_budget).sum();
        assert!(
            total <= 210,
            "slot budgets summed to {total}, above the 70% cap of a 300-token budget"
        );
        assert!(
            slots.iter().all(|s| s.tokens_budget >= 1),
            "a live slot must floor at 1 token, never 0 (hydration would drop it)"
        );
    }

    #[test]
    fn empty_pool_yields_no_slots() {
        let skel = FallbackSkeleton::default();
        assert!(skeleton_pack(&[], &skel, 100_000, 1_700_000_000).is_empty());
    }

    #[test]
    fn items_sorted_by_relevance_within_slot() {
        let now = 1_700_000_000;
        let c = [
            cand("note://reference/a", SlotKind::RelevantNotes, 0.3, now),
            cand("note://reference/b", SlotKind::RelevantNotes, 0.9, now),
            cand("note://reference/c", SlotKind::RelevantNotes, 0.5, now),
        ];
        let slots = skeleton_pack(&c, &FallbackSkeleton::default(), 100_000, now);
        let rel_slot = slots
            .iter()
            .find(|s| s.kind == SlotKind::RelevantNotes)
            .unwrap();
        assert_eq!(rel_slot.items[0].id, "note://reference/b");
        assert_eq!(rel_slot.items[1].id, "note://reference/c");
        assert_eq!(rel_slot.items[2].id, "note://reference/a");
    }

    #[test]
    fn feedback_candidates_pack_into_feedback_slot() {
        let now = 1_700_000_000;
        let c = [
            cand("note://feedback/no-jsdoc", SlotKind::Feedback, 0.4, now),
            cand("note://reference/a", SlotKind::RelevantNotes, 0.9, now),
        ];
        let slots = skeleton_pack(&c, &FallbackSkeleton::default(), 100_000, now);
        let fb = slots
            .iter()
            .find(|s| s.kind == SlotKind::Feedback)
            .expect("feedback slot present");
        assert_eq!(fb.items.len(), 1);
        assert_eq!(fb.items[0].id, "note://feedback/no-jsdoc");
        // Feedback is packed first → highest priority slot in the output.
        assert_eq!(slots[0].kind, SlotKind::Feedback);
    }

    #[test]
    fn zero_budget_slot_is_excluded() {
        let skel = FallbackSkeleton {
            relevant_notes_tokens: 0,
            ..Default::default()
        };
        let now = 1_700_000_000;
        let c = [cand(
            "note://reference/a",
            SlotKind::RelevantNotes,
            0.9,
            now,
        )];
        let slots = skeleton_pack(&c, &skel, 100_000, now);
        assert!(slots.iter().all(|s| s.kind != SlotKind::RelevantNotes));
    }

    #[test]
    fn recency_factor_bounded() {
        let now = 1_700_000_000;
        assert!((recency_factor(now, now) - 1.0).abs() < 1e-6);
        assert!(recency_factor(now - 86_400 * 10_000, now) >= 0.5);
    }
}
