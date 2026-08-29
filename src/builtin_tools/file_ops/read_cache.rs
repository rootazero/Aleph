//! Cross-turn "unchanged re-read" detection for `file_read`.
//!
//! A model that re-issues the *same* `file_read` (identical path / offset /
//! limit) across turns re-pays the full token cost every time, and tight read
//! loops (read → think → read the same thing again) can stall a run. `file_read`
//! results are deliberately never persisted to the result store — see the §3.2
//! invariant "no read-file-marker loop" (`tools::result_processing`) — so the
//! per-turn spill path offers no protection on this code path specifically.
//!
//! This guard closes that gap *mechanically*, not by reasoning about intent
//! (R7 / P8): it keys on `(canonical_path, offset, limit)` and compares the
//! file's `(mtime, size)`. When a repeat read targets a byte-for-byte unchanged
//! window, the caller may omit the full rendered content and return a compact
//! stub — the model already received that content on the first read. A second
//! consecutive repeat escalates the wording to a firm "stop re-reading" nudge.
//! Any real change (mtime or size) resets the counter and yields a fresh read.
//!
//! The store fails *open*: if the filesystem timestamp can't be read, or the
//! lock is poisoned, the caller renders normally. A false "fresh" only costs
//! tokens; a false "unchanged" would starve the model, so the bias is
//! deliberate.

use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

use crate::sync_primitives::{Arc, Mutex};

/// Identity of a windowed read: same file + same window ⇒ same key.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ReadKey {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

/// What we remember about the last time this exact window was read.
#[derive(Clone, Copy)]
struct ReadFingerprint {
    mtime: SystemTime,
    size: u64,
    /// Count of *consecutive* unchanged re-reads already served as a stub
    /// (0 means the last observation rendered full content).
    repeats: u32,
}

/// Outcome of observing a read against the cache.
pub(super) enum ReadCacheDecision {
    /// File is new, changed, or unverifiable: execute and render in full.
    Fresh,
    /// Identical re-read of an unchanged file: the caller may serve a stub.
    /// `repeats` is 1 for the first stub and escalates on each further repeat.
    Unchanged { repeats: u32 },
}

/// Per-instance fingerprint store, shared across `Clone`s of one `FileReadTool`
/// (= one builtin registry = one session). A plain `std::sync::Mutex` is right
/// here: every critical section is a single map probe/insert, never held across
/// an `.await`.
#[derive(Clone, Default)]
pub(super) struct ReadCache {
    // Pair the map with an `insertion_order` VecDeque so the cap
    // eviction policy is FIFO (the oldest-inserted key is dropped
    // when the cap is exceeded) rather than SipHash-permutation-
    // dependent. The deque is kept in sync on every insert / remove
    // so the cap test is deterministic across runs.
    inner: Arc<Mutex<ReadCacheInner>>,
}

#[derive(Default)]
struct ReadCacheInner {
    map: HashMap<ReadKey, ReadFingerprint>,
    insertion_order: VecDeque<ReadKey>,
}

/// BT-A-R4-01: cap on the number of distinct `(path, offset, limit)` windows
/// tracked simultaneously. The map is keyed by the window identity, so a
/// long-lived session that reads millions of distinct windows accumulates
/// one row per window — the fingerprint is small (~64 bytes including the
/// string key + fingerprint), but the map itself, the hash table, and the
/// Arc/Mutex wrapper all grow linearly. 10 000 entries is ~1 MB of state
/// and covers the realistic upper end of any one session's distinct read
/// windows; older windows are evicted when the cap is hit.
///
/// Eviction policy: drop the entry that was least-recently *inserted* (no
/// recency tracking — this is a fingerprint cache, not an LRU; a true LRU
/// needs `lru::LruCache` which is not currently a direct dep and adds
/// nontrivial surface for a marginal accuracy gain). Reads of a window that
/// gets evicted simply re-render in full on the next observe.
const MAX_READ_CACHE_ENTRIES: usize = 10_000;

impl ReadCache {
    /// Record this read and decide whether the caller may skip full rendering.
    ///
    /// `fingerprint` carries the file's current `(mtime, size)`, or `None` when
    /// the filesystem metadata could not be read. On `None` we fail open
    /// (return [`ReadCacheDecision::Fresh`]) and forget any prior entry, so a
    /// later successful stat starts the counter clean rather than comparing
    /// against a stale fingerprint.
    pub(super) fn observe(
        &self,
        path: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        fingerprint: Option<(SystemTime, u64)>,
    ) -> ReadCacheDecision {
        let key = ReadKey {
            path: path.to_string(),
            offset,
            limit,
        };

        // Poison-safe per P7: recover the guard rather than panicking.
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let Some((mtime, size)) = fingerprint else {
            inner.map.remove(&key);
            return ReadCacheDecision::Fresh;
        };

        match inner.map.get_mut(&key) {
            Some(fp) if fp.mtime == mtime && fp.size == size => {
                fp.repeats = fp.repeats.saturating_add(1);
                ReadCacheDecision::Unchanged {
                    repeats: fp.repeats,
                }
            }
            _ => {
                // Refresh the FIFO position BEFORE moving `key` into
                // the map, so the retain closure can borrow `key`
                // without conflicting with the insertion.
                inner.insertion_order.retain(|k| k != &key);
                inner.insertion_order.push_back(key.clone());
                inner.map.insert(
                    key,
                    ReadFingerprint {
                        mtime,
                        size,
                        repeats: 0,
                    },
                );
                // BT-A-R4-01: enforce the entry cap. When the map is full we
                // drop one existing entry (arbitrary choice — any is fine
                // since the cap is a leak guard, not a correctness
                // requirement) before inserting the new window. The dropped
                // window simply re-renders in full on its next observe.
                if inner.map.len() > MAX_READ_CACHE_ENTRIES {
                    // FIFO eviction: drop the oldest-inserted key.
                    // The previous `map.keys().next()` shape was
                    // HashMap-iteration-order-dependent (SipHash
                    // permutation), which would make the cap test
                    // order-dependent across runs.
                    if let Some(evicted) = inner.insertion_order.pop_front() {
                        inner.map.remove(&evicted);
                    }
                }
                ReadCacheDecision::Fresh
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
    }

    #[test]
    fn first_read_is_fresh() {
        let cache = ReadCache::default();
        let d = cache.observe("/a.txt", None, None, Some((t0(), 42)));
        assert!(matches!(d, ReadCacheDecision::Fresh));
    }

    #[test]
    fn unchanged_repeat_escalates() {
        let cache = ReadCache::default();
        let fp = Some((t0(), 42));
        assert!(matches!(
            cache.observe("/a.txt", None, None, fp),
            ReadCacheDecision::Fresh
        ));
        assert!(matches!(
            cache.observe("/a.txt", None, None, fp),
            ReadCacheDecision::Unchanged { repeats: 1 }
        ));
        assert!(matches!(
            cache.observe("/a.txt", None, None, fp),
            ReadCacheDecision::Unchanged { repeats: 2 }
        ));
    }

    #[test]
    fn changed_mtime_resets_to_fresh() {
        let cache = ReadCache::default();
        cache.observe("/a.txt", None, None, Some((t0(), 42)));
        // Same size, newer mtime ⇒ the file changed.
        let newer = t0() + Duration::from_secs(5);
        assert!(matches!(
            cache.observe("/a.txt", None, None, Some((newer, 42))),
            ReadCacheDecision::Fresh
        ));
        // And the counter restarts from this new baseline.
        assert!(matches!(
            cache.observe("/a.txt", None, None, Some((newer, 42))),
            ReadCacheDecision::Unchanged { repeats: 1 }
        ));
    }

    #[test]
    fn changed_size_resets_to_fresh() {
        let cache = ReadCache::default();
        cache.observe("/a.txt", None, None, Some((t0(), 42)));
        assert!(matches!(
            cache.observe("/a.txt", None, None, Some((t0(), 99))),
            ReadCacheDecision::Fresh
        ));
    }

    #[test]
    fn different_window_is_independent() {
        let cache = ReadCache::default();
        let fp = Some((t0(), 42));
        cache.observe("/a.txt", None, None, fp);
        // Same file, different offset ⇒ a different window, tracked separately.
        assert!(matches!(
            cache.observe("/a.txt", Some(100), None, fp),
            ReadCacheDecision::Fresh
        ));
    }

    #[test]
    fn missing_fingerprint_fails_open_and_forgets() {
        let cache = ReadCache::default();
        let fp = Some((t0(), 42));
        cache.observe("/a.txt", None, None, fp);
        // An unverifiable stat fails open and clears the prior entry...
        assert!(matches!(
            cache.observe("/a.txt", None, None, None),
            ReadCacheDecision::Fresh
        ));
        // ...so the next verifiable read is treated as a fresh baseline.
        assert!(matches!(
            cache.observe("/a.txt", None, None, fp),
            ReadCacheDecision::Fresh
        ));
    }
}
