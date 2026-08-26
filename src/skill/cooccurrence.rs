//! Skill co-occurrence log — a bounded ring of recent skill *uses*.
//!
//! Sibling of [`crate::skill::usage`]'s `.usage.json` sidecar, living in the
//! same skills dir as `.cooccur.json`. Where the usage sidecar answers "how
//! often was THIS skill used", this log answers "which skills were used CLOSE
//! TOGETHER" — the raw signal the dream pipeline mines into `MetaSkill`
//! (workflow) proposals (see
//! [`crate::memory::dreaming::stages::WorkflowProposalStage`]).
//!
//! ## Why a separate sidecar
//!
//! `.usage.json` is a bare `HashMap<String, UsageStats>` on disk; bolting a
//! ring onto it would change that shape. A standalone `.cooccur.json` keeps
//! the usage format byte-stable and the concern isolated (P2 high cohesion).
//!
//! ## Why temporal proximity, not session ids
//!
//! Capture rides the existing `UsageStore::record_use` chokepoint (the skill
//! reader tool) — one extra call, no session-id threading through the harness.
//! Each use appends `{skill, at_ms}`. Co-occurrence is recovered
//! *deterministically* downstream by clustering entries that fall within a
//! short window: skills read within seconds of each other were used together.
//! No reasoning decides what co-occurs, only the clock — pure system-signal
//! aggregation (R7-safe, the same boundary as `SkillLifecycleStage`).
//!
//! Best-effort throughout, mirroring `usage.rs`: a corrupt or unwritable log
//! degrades to a warn and never breaks the underlying tool call.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::utils::atomic_io::{with_file_lock, write_atomic};

/// Maximum entries retained in the ring. Old entries are evicted FIFO once the
/// cap is reached. Sized to hold a few days of normal skill activity so the
/// miner sees enough repeats to clear its frequency floor, while keeping the
/// sidecar small and the read cheap.
const MAX_ENTRIES: usize = 512;

/// One recorded skill use: the skill id and the wall-clock millisecond it was
/// used. The timestamp is the *only* co-occurrence signal — entries close in
/// time are treated as used together by the downstream miner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentUse {
    /// Skill id (the `skill_id` the reader tool resolved).
    pub skill: String,
    /// Unix epoch milliseconds at record time.
    pub at_ms: u64,
}

/// Append-only (bounded) log of recent skill uses in `<dir>/.cooccur.json`.
pub struct CoOccurrenceLog {
    path: PathBuf,
    lock_path: PathBuf,
}

impl CoOccurrenceLog {
    /// Create a log backed by `<skills_dir>/.cooccur.json`.
    pub fn new(skills_dir: impl AsRef<Path>) -> Self {
        let path = skills_dir.as_ref().join(".cooccur.json");
        let lock_path = skills_dir.as_ref().join(".cooccur.json.lock");
        Self { path, lock_path }
    }

    fn load(&self) -> Vec<RecentUse> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(error = %e, path = %self.path.display(), "corrupted cooccur sidecar; resetting");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        }
    }

    fn save(&self, entries: &[RecentUse]) {
        match serde_json::to_vec(entries) {
            Ok(bytes) => {
                if let Err(e) = write_atomic(&self.path, &bytes) {
                    tracing::warn!(error = %e, "skill cooccur: atomic write failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "skill cooccur: serialize failed"),
        }
    }

    /// Append one skill use, stamped now. Bounded FIFO — once the ring is full
    /// the oldest entry is dropped. Best-effort: failures warn and return.
    pub fn record(&self, skill: &str) {
        if skill.is_empty() {
            return;
        }
        let at_ms = now_ms();
        if let Some(parent) = self.lock_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(error = %e, "skill cooccur: mkdir failed");
                return;
            }
        }
        let result = with_file_lock(&self.lock_path, |_| {
            let mut entries = self.load();
            entries.push(RecentUse {
                skill: skill.to_string(),
                at_ms,
            });
            let overflow = entries.len().saturating_sub(MAX_ENTRIES);
            if overflow > 0 {
                entries.drain(0..overflow);
            }
            self.save(&entries);
            Ok(())
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, skill, "skill cooccur: lock acquisition failed");
        }
    }

    /// Read the full ring, oldest first. Empty when nothing recorded.
    #[must_use]
    pub fn snapshot(&self) -> Vec<RecentUse> {
        self.load()
    }
}

/// Cluster a time-ordered slice of uses into chains: a new chain starts
/// whenever the gap to the previous use exceeds `window_secs`. Within a chain
/// the *first* occurrence order of each distinct skill is preserved (so a
/// proposal's steps mirror how the user actually sequenced the skills), and a
/// skill repeated inside one chain is de-duplicated.
///
/// Pure function — no I/O, deterministic. Entries are assumed roughly sorted by
/// `at_ms` (the ring appends in time order); a defensive sort guards against a
/// hand-edited sidecar.
#[must_use]
pub fn cluster_chains(entries: &[RecentUse], window_secs: u64) -> Vec<Vec<String>> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|e| e.at_ms);

    let window_ms = window_secs.saturating_mul(1_000);
    let mut chains: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    // Parallel HashSet so the in-chain dedup is O(1) instead of
    // `Vec::contains` (O(n)). For MAX_ENTRIES = 512 with every entry a
    // distinct skill the linear search previously spent ~131k
    // `String::eq` comparisons per call — this brings it back to ~512
    // hash lookups.
    let mut current_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut prev_ms: Option<u64> = None;

    for e in &sorted {
        let break_chain = match prev_ms {
            Some(p) => e.at_ms.saturating_sub(p) > window_ms,
            None => false,
        };
        if break_chain && !current.is_empty() {
            chains.push(std::mem::take(&mut current));
            current_set.clear();
        }
        if current_set.insert(e.skill.clone()) {
            current.push(e.skill.clone());
        }
        prev_ms = Some(e.at_ms);
    }
    if !current.is_empty() {
        chains.push(current);
    }
    chains
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_at(skill: &str, at_ms: u64) -> RecentUse {
        RecentUse {
            skill: skill.into(),
            at_ms,
        }
    }

    #[test]
    fn empty_snapshot_when_nothing_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let log = CoOccurrenceLog::new(tmp.path());
        assert!(log.snapshot().is_empty());
    }

    #[test]
    fn record_then_snapshot_roundtrips_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let log = CoOccurrenceLog::new(tmp.path());
        log.record("a");
        log.record("b");
        let snap = log.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].skill, "a");
        assert_eq!(snap[1].skill, "b");
        assert!(snap[1].at_ms >= snap[0].at_ms);
    }

    #[test]
    fn empty_skill_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let log = CoOccurrenceLog::new(tmp.path());
        log.record("");
        assert!(log.snapshot().is_empty());
    }

    #[test]
    fn ring_is_bounded_fifo() {
        let tmp = tempfile::tempdir().unwrap();
        let log = CoOccurrenceLog::new(tmp.path());
        for i in 0..(MAX_ENTRIES + 10) {
            log.record(&format!("s{i}"));
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), MAX_ENTRIES);
        // Oldest 10 evicted; the very first surviving entry is s10.
        assert_eq!(snap[0].skill, "s10");
    }

    #[test]
    fn cluster_splits_on_window_gap() {
        // a,b within window; then a 1h gap; then c,d within window.
        let entries = vec![
            use_at("a", 1_000),
            use_at("b", 2_000),
            use_at("c", 3_600_000 + 5_000),
            use_at("d", 3_600_000 + 6_000),
        ];
        let chains = cluster_chains(&entries, 60);
        assert_eq!(chains.len(), 2);
        assert_eq!(chains[0], vec!["a", "b"]);
        assert_eq!(chains[1], vec!["c", "d"]);
    }

    #[test]
    fn cluster_dedups_repeat_within_chain_preserving_first_order() {
        let entries = vec![
            use_at("a", 1_000),
            use_at("b", 2_000),
            use_at("a", 3_000),
            use_at("c", 4_000),
        ];
        let chains = cluster_chains(&entries, 60);
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn cluster_handles_unsorted_input() {
        let entries = vec![use_at("b", 2_000), use_at("a", 1_000)];
        let chains = cluster_chains(&entries, 60);
        assert_eq!(chains[0], vec!["a", "b"]);
    }
}
