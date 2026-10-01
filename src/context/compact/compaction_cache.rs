//! Cross-run fingerprint-cache carry-over for [`ContextCompactor`](super::compactor::ContextCompactor).
//!
//! The compactor is built fresh per run (`runner_impl`), so without the
//! process-wide per-session slot below its fingerprint cache dies at every run
//! boundary and a long high-pressure conversation re-pays the side-channel
//! summarization call — with freshly-worded summary text that re-keys the
//! provider prompt cache — at the start of every run.
//!
//! Same shape as the runner's `CALIBRATION_CARRYOVER`: process-wide because the
//! bridge is a boot-time singleton; never persisted to disk; safe because every
//! read is hash-validated against the rebuilt history before reuse (a stale
//! entry misses and is purged).
//!
//! Also homes [`SummaryReuse`], the wiring for the zero-API-cost
//! session-summary reuse path: the memory backend holding the d0/d1/d2
//! summaries plus the agent id they were written under. Same lifetime
//! (`Option<…>` on the compactor) as the rest of the cache plumbing.

use crate::memory::store::MemoryBackend;
use crate::sync_primitives::Mutex;

/// Un-summarized pre-tail growth beyond the cached summary that triggers an
/// incremental LLM merge instead of a pure cache reapply. Below both
/// thresholds the gap rides along uncompacted (it is recent, small, and will
/// be folded into the summary once it crosses either bound).
pub(super) const CACHE_EXTEND_MIN_MESSAGES: usize = 8;
pub(super) const CACHE_EXTEND_MIN_TOKENS: usize = 4096;

/// Bound on cross-run carry-over slots. Sessions beyond the cap evict the
/// least-recently-WRITTEN entry (every `carryover_put` moves its key to the
/// back) — a long-lived interactive session that keeps compacting stays hot
/// even while daemon/cron fires churn one-shot session keys through the
/// front. A linear-scan `Vec` is fine at this size.
pub(super) const CARRYOVER_MAX_SESSIONS: usize = 16;

/// Cached result of the last successful compaction, expressed in coordinates
/// of the *rebuilt* (uncompacted) message list: `[start, end)` is the covered
/// range, `hash` fingerprints the covered messages, and `summary` is the full
/// `[Context Summary]…` text that replaces them.
#[derive(Clone)]
pub(super) struct CompactionCache {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) hash: u64,
    pub(super) summary: String,
}

/// Wiring for the zero-API-cost session-summary reuse path: the memory backend
/// holding the d0/d1/d2 summaries plus the agent id they were written under.
/// Both are required together — `get_raw_by_path_prefix` filters by agent id.
pub(super) struct SummaryReuse {
    pub(super) backend: MemoryBackend,
    pub(super) agent_id: String,
}

/// Cross-run fingerprint-cache carry-over, keyed by session key.
pub(super) static COMPACTION_CARRYOVER: Mutex<Vec<(String, CompactionCache)>> =
    Mutex::new(Vec::new());

/// Read the carried-over cache entry for `key`, if present. Slot-parametric
/// so tests can exercise eviction/purge without touching the process-global.
pub(super) fn carryover_get(
    slot: &Mutex<Vec<(String, CompactionCache)>>,
    key: &str,
) -> Option<CompactionCache> {
    let guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, entry)| entry.clone())
}

/// Store `entry` under `key`. Re-writing an existing key moves it to the
/// back (LRU-on-write); when the slot is full the least-recently-written
/// entry at the front is evicted. FIFO-by-first-insertion would evict the
/// feature's primary beneficiary first: the long-lived session inserted
/// earliest and updated most often.
pub(super) fn carryover_put(
    slot: &Mutex<Vec<(String, CompactionCache)>>,
    key: &str,
    entry: CompactionCache,
) {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(pos) = guard.iter().position(|(k, _)| k == key) {
        guard.remove(pos);
    } else if guard.len() >= CARRYOVER_MAX_SESSIONS {
        guard.remove(0);
    }
    guard.push((key.to_string(), entry));
}

/// Drop the entry for `key` (no-op when absent) — called when a hash
/// validation fails so the next run does not re-seed a dead entry.
pub(super) fn carryover_remove(slot: &Mutex<Vec<(String, CompactionCache)>>, key: &str) {
    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|(k, _)| k != key);
}
