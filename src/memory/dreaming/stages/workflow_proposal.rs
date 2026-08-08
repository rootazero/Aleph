//! `WorkflowProposalStage` — drafts `MetaSkill` (workflow) candidates from
//! recurring skill co-occurrence.
//!
//! This is the dream pipeline's "replay and dream mode" closing the loop: it reads the
//! [`CoOccurrenceLog`](crate::skill::cooccurrence) rings captured at skill-use
//! time, clusters them into chains by temporal proximity, and for any chain
//! that recurs above a frequency floor drafts a gated
//! [`WorkflowProposal`](crate::workflow::proposal) — a `MetaSkill` candidate the
//! user can review and accept. Capabilities thus grow quietly in the
//! background.
//!
//! ## R7 / R10 boundary
//!
//! Everything here is **deterministic system-signal aggregation** — the same
//! boundary as [`super::SkillLifecycleStage`]. Clustering is clock-driven,
//! the frequency floor is a count, and the drafted workflow is a literal
//! transcription of the observed skill sequence. No reasoning runs in this
//! stage and **no LLM call is incurred**. The intelligence — refining the
//! steps, assigning real agents, deciding whether the `MetaSkill` is worth
//! keeping — is the user's/LLM's at accept time, behind the proposal gate.
//! The stage never activates a workflow; it only writes drafts to the gated
//! `proposals/` directory.

use std::collections::HashMap;

use async_trait::async_trait;

use super::DreamStage;
use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::skill::cooccurrence::{cluster_chains, CoOccurrenceLog};
use crate::skill::default_skill_dirs;
use crate::workflow::proposal;

/// Co-occurrence window: skill uses within this many seconds of each other are
/// treated as one chain. Five minutes captures a single working session's
/// skill sequence without bridging unrelated bursts hours apart.
const DEFAULT_WINDOW_SECS: u64 = 300;

/// Frequency floor: a chain must recur at least this many times across the ring
/// before it is worth drafting. Keeps one-off sequences from becoming noise.
const DEFAULT_MIN_OBSERVATIONS: u32 = 3;

/// Cap on proposals drafted per dream cycle, so a busy ring cannot flood the
/// gated dir in one pass. The highest-frequency chains win.
const DEFAULT_MAX_PER_CYCLE: usize = 3;

/// Drafts `MetaSkill` proposals from recurring skill co-occurrence chains.
pub struct WorkflowProposalStage {
    pub window_secs: u64,
    pub min_observations: u32,
    pub max_per_cycle: usize,
}

impl Default for WorkflowProposalStage {
    fn default() -> Self {
        Self {
            window_secs: DEFAULT_WINDOW_SECS,
            min_observations: DEFAULT_MIN_OBSERVATIONS,
            max_per_cycle: DEFAULT_MAX_PER_CYCLE,
        }
    }
}

/// Aggregate co-occurrence chains keyed by their canonical (sorted) identity.
/// Returns `(canonical_key → (observation_count, representative_chain))`, where
/// the representative chain preserves the *first observed* order so the drafted
/// workflow mirrors how the user actually sequenced the skills.
///
/// Pure function — testable without disk.
#[must_use]
pub fn aggregate_chains(chains: Vec<Vec<String>>) -> HashMap<String, (u32, Vec<String>)> {
    let mut counts: HashMap<String, (u32, Vec<String>)> = HashMap::new();
    for chain in chains {
        if chain.len() < 2 {
            continue;
        }
        let key = proposal::canonical_name(&chain);
        counts
            .entry(key)
            .and_modify(|(n, _)| *n = n.saturating_add(1))
            .or_insert((1, chain));
    }
    counts
}

#[async_trait]
impl DreamStage for WorkflowProposalStage {
    fn name(&self) -> &'static str {
        "workflow_proposal"
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let dirs = default_skill_dirs();
        if dirs.is_empty() {
            tracing::debug!("WorkflowProposal: no skill dirs; nothing to mine");
            return Ok(ctx);
        }

        // Merge co-occurrence rings across every registered skill dir, then
        // cluster each ring independently (a ring's entries share a clock).
        let mut all_chains: Vec<Vec<String>> = Vec::new();
        for dir in &dirs {
            let log = CoOccurrenceLog::new(dir);
            let snap = log.snapshot();
            all_chains.extend(cluster_chains(&snap, self.window_secs));
        }

        let aggregated = aggregate_chains(all_chains);

        // Rank by observation count, descending; take the strongest signals
        // first so the per-cycle cap keeps the best chains.
        let mut ranked: Vec<(u32, Vec<String>)> = aggregated
            .into_values()
            .filter(|(n, _)| *n >= self.min_observations)
            .collect();
        ranked.sort_by_key(|(n, _)| std::cmp::Reverse(*n));

        let mut drafted = 0u32;
        let mut skipped_covered = 0u32;
        for (observations, chain) in ranked {
            if drafted as usize >= self.max_per_cycle {
                break;
            }
            // Gate against existing coverage: an active workflow or a pending
            // proposal with the same identity, or a hand-built workflow with
            // the same step set under any name.
            if proposal::already_covered(&chain) || proposal::covered_by_step_set(&chain) {
                skipped_covered += 1;
                continue;
            }
            let Some(def) = proposal::skeleton_from_chain(&chain, observations) else {
                continue;
            };
            match proposal::save_proposal(&def) {
                Ok(path) => {
                    drafted += 1;
                    tracing::info!(
                        name = %def.name,
                        observations,
                        path = %path.display(),
                        "WorkflowProposal: drafted MetaSkill candidate"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, name = %def.name, "WorkflowProposal: save failed");
                }
            }
        }

        tracing::info!(drafted, skipped_covered, "WorkflowProposal: stage complete");
        Ok(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_counts_repeated_chains() {
        let chains = vec![
            vec!["a".into(), "b".into()],
            vec!["b".into(), "a".into()], // same canonical identity
            vec!["c".into(), "d".into()],
        ];
        let agg = aggregate_chains(chains);
        let ab = proposal::canonical_name(&["a".into(), "b".into()]);
        assert_eq!(agg.get(&ab).unwrap().0, 2);
        // Representative preserves first observed order.
        assert_eq!(
            agg.get(&ab).unwrap().1,
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn aggregate_drops_single_skill_chains() {
        let agg = aggregate_chains(vec![vec!["solo".into()]]);
        assert!(agg.is_empty());
    }

    #[test]
    fn stage_has_sane_defaults() {
        let s = WorkflowProposalStage::default();
        assert_eq!(s.min_observations, DEFAULT_MIN_OBSERVATIONS);
        assert!(s.window_secs > 0);
        assert!(s.max_per_cycle >= 1);
    }
}
