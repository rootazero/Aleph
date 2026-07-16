//! Cache hit/miss monitor for prompt caching.
//!
//! Tracks whether LLM responses include cache read tokens, emitting warnings
//! when consecutive misses suggest the stable prompt hash has changed
//! unexpectedly.

use crate::sync_primitives::Mutex;

// =============================================================================
// Internal state
// =============================================================================

struct MonitorState {
    consecutive_misses: u32,
    total_calls: u64,
}

impl MonitorState {
    const fn new() -> Self {
        Self {
            consecutive_misses: 0,
            total_calls: 0,
        }
    }
}

// =============================================================================
// CacheMonitor
// =============================================================================

/// Monitor for prompt cache hit/miss tracking.
///
/// Thread-safe via interior mutability.  Callers record the
/// `cache_read_tokens` value from each call's `TokenUsage` to detect
/// consecutive cache misses. (A per-call stable-content hash comparator
/// used to live here too; it never gained a production caller and was
/// wrong-grained for the process-wide singleton — removed per YAGNI.)
pub struct CacheMonitor {
    state: Mutex<MonitorState>,
}

impl CacheMonitor {
    /// Create a new monitor with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MonitorState::new()),
        }
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
}

impl Default for CacheMonitor {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Process-wide singleton
// =============================================================================
//
// The cache monitor is wired into the metering provider — every LLM call
// reports its `cache_read_tokens` to the singleton, which warns on a run of
// consecutive misses (a hint the stable prefix changed unexpectedly).
//
// Same `OnceLock` shape as `pricing` / `tool_result_store`: lazy install on
// first read, `&'static` reads on the hot path.

static GLOBAL_CACHE_MONITOR: std::sync::OnceLock<CacheMonitor> = std::sync::OnceLock::new();

/// Return the process-wide `CacheMonitor`, lazily creating it on first use.
pub fn global_cache_monitor() -> &'static CacheMonitor {
    GLOBAL_CACHE_MONITOR.get_or_init(CacheMonitor::new)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
        let state = monitor.state.lock().expect("lock poisoned");
        assert_eq!(
            state.consecutive_misses, 1,
            "miss count should restart from 1 after compaction"
        );
    }
}
