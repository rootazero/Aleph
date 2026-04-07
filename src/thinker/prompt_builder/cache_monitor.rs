//! Cache hit/miss monitor for prompt caching.
//!
//! Tracks whether LLM responses include cache read tokens, emitting warnings
//! when consecutive misses suggest the stable prompt hash has changed
//! unexpectedly.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

// =============================================================================
// Internal state
// =============================================================================

struct MonitorState {
    stable_hash: Option<u64>,
    consecutive_misses: u32,
    total_calls: u64,
    total_hits: u64,
}

impl MonitorState {
    fn new() -> Self {
        Self {
            stable_hash: None,
            consecutive_misses: 0,
            total_calls: 0,
            total_hits: 0,
        }
    }
}

// =============================================================================
// CacheMonitor
// =============================================================================

/// Monitor for prompt cache hit/miss tracking.
///
/// Thread-safe via interior mutability.  Callers update the stable-content
/// hash before each LLM call and record the `cache_read_tokens` value from
/// the resulting `TokenUsage` to detect consecutive cache misses.
pub struct CacheMonitor {
    state: Mutex<MonitorState>,
}

impl CacheMonitor {
    /// Create a new monitor with empty state.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MonitorState::new()),
        }
    }

    /// Hash `stable_content` and compare with the previously stored hash.
    ///
    /// Returns `true` when the hash has changed from its previous value
    /// (indicating the stable portion of the prompt has been modified),
    /// and `false` on the first call or when the content is unchanged.
    pub fn update_stable_hash(&self, stable_content: &str) -> bool {
        // Fast non-cryptographic hash for change detection within a single process.
        //
        // NOTE: `DefaultHasher` is NOT stable across Rust versions or process restarts.
        // This is acceptable here because `CacheMonitor` is session-scoped and in-memory.
        // Do NOT persist or compare hashes across processes.
        let mut hasher = DefaultHasher::new();
        stable_content.hash(&mut hasher);
        let new_hash = hasher.finish();

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let changed = state.stable_hash.is_some_and(|old| old != new_hash);
        state.stable_hash = Some(new_hash);
        changed
    }

    /// Record cache usage from a completed LLM call.
    ///
    /// A `cache_read_tokens` value of `None` or `Some(0)` counts as a miss.
    /// After 3 or more consecutive misses (and at least 4 total calls) a
    /// `warn!` is emitted so operators can investigate.
    pub fn record_cache_usage(&self, cache_read_tokens: Option<u32>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());

        state.total_calls += 1;

        let is_hit = cache_read_tokens.is_some_and(|t| t > 0);
        if is_hit {
            state.total_hits += 1;
            state.consecutive_misses = 0;
        } else {
            state.consecutive_misses += 1;
            if state.consecutive_misses >= 3 && state.total_calls > 3 {
                tracing::warn!(
                    consecutive_misses = state.consecutive_misses,
                    total_calls = state.total_calls,
                    "prompt cache consecutive misses detected — stable prefix may have changed"
                );
            }
        }
    }

    /// Notify the monitor that a compaction has occurred.
    ///
    /// Compaction legitimately breaks the prompt cache (the message list is
    /// rewritten), so consecutive-miss tracking is reset to avoid false
    /// positive warnings immediately after compaction.
    pub fn notify_compaction(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.consecutive_misses = 0;
    }

    /// Return the cache hit rate as a percentage in [0.0, 100.0].
    ///
    /// Returns `100.0` when no calls have been recorded yet.
    pub fn hit_rate(&self) -> f64 {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.total_calls == 0 {
            return 100.0;
        }
        (state.total_hits as f64 / state.total_calls as f64) * 100.0
    }
}

impl Default for CacheMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_hash_update_returns_false() {
        let monitor = CacheMonitor::new();
        assert!(!monitor.update_stable_hash("hello world"));
    }

    #[test]
    fn same_hash_returns_false() {
        let monitor = CacheMonitor::new();
        monitor.update_stable_hash("same content");
        assert!(!monitor.update_stable_hash("same content"));
    }

    #[test]
    fn changed_hash_returns_true() {
        let monitor = CacheMonitor::new();
        monitor.update_stable_hash("first content");
        assert!(monitor.update_stable_hash("different content"));
    }

    #[test]
    fn hit_rate_tracks_correctly() {
        let monitor = CacheMonitor::new();
        // 3 hits
        monitor.record_cache_usage(Some(100));
        monitor.record_cache_usage(Some(200));
        monitor.record_cache_usage(Some(50));
        // 1 miss
        monitor.record_cache_usage(None);

        // 3 hits out of 4 total = 75%
        assert!((monitor.hit_rate() - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn compaction_resets_consecutive_misses() {
        let monitor = CacheMonitor::new();
        // Drive up consecutive misses past the warning threshold
        // (needs > 3 total calls and >= 3 consecutive misses)
        monitor.record_cache_usage(Some(100)); // hit — total_calls=1
        monitor.record_cache_usage(None); // miss — total_calls=2
        monitor.record_cache_usage(None); // miss — total_calls=3
        monitor.record_cache_usage(None); // miss — total_calls=4, warn fires

        // Compaction resets the counter
        monitor.notify_compaction();

        // After reset, a single miss should NOT trigger a warning
        // (consecutive_misses is back to 0, so 1 miss = 1 consecutive)
        monitor.record_cache_usage(None); // miss — consecutive=1, no warn
        let state = monitor.state.lock().unwrap();
        assert_eq!(
            state.consecutive_misses, 1,
            "miss count should restart from 1 after compaction"
        );
    }

    #[test]
    fn hit_rate_is_100_when_no_calls() {
        let monitor = CacheMonitor::new();
        assert!((monitor.hit_rate() - 100.0).abs() < f64::EPSILON);
    }
}
