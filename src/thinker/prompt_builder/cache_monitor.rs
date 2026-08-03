//! Cache hit/miss monitor for prompt caching.
//!
//! Tracks whether LLM responses include cache read tokens, emitting warnings
//! when consecutive misses suggest the stable prompt prefix has changed
//! unexpectedly.
//!
//! Two grain rules keep the signal honest (both were real false-positive /
//! masking sources when the monitor was a single flat counter):
//!
//! - **Per-agent tracking.** Each agent id gets its own consecutive-miss
//!   counter, so an interleaved cache-hitting agent can no longer reset the
//!   counter and mask another agent's genuine prefix breakage.
//! - **Armed only after observed cache activity.** Misses are counted only
//!   once an agent has reported at least one non-zero `cache_read` or
//!   `cache_creation` value. Endpoints that never report cache usage (mock
//!   providers, Ollama, `cache_retention = off`, hosts without
//!   `cache_control` support) produce `None`/0 readings that say nothing
//!   about the stable prefix — they no longer pollute the counters.

use crate::sync_primitives::Mutex;
use std::collections::HashMap;

// =============================================================================
// Internal state
// =============================================================================

#[derive(Default)]
struct AgentCacheState {
    consecutive_misses: u32,
    total_calls: u64,
    /// Set once this agent has observed any cache activity (read or write).
    /// Miss counting (and the warn) is gated on it — see module docs.
    armed: bool,
    /// Latch so the warn fires on the RISING edge of a streak rather than on
    /// every call past the threshold. Without it a genuinely broken agent
    /// emits one WARN per LLM call, which is how a line gets filtered out —
    /// and this is the only alarm this domain has. Cleared by a healthy call.
    warned: bool,
}

/// Bound on tracked agent ids. Subagent spawns can mint fresh ids over a
/// long daemon lifetime; past the cap an arbitrary entry is evicted (the
/// monitor is a best-effort watchdog, not an accounting ledger).
const MAX_TRACKED_AGENTS: usize = 64;

// =============================================================================
// CacheMonitor
// =============================================================================

/// Monitor for prompt cache hit/miss tracking.
///
/// Thread-safe via interior mutability. Callers record the
/// `cache_read_tokens` / `cache_creation_tokens` values from each call's
/// `TokenUsage` to detect consecutive cache misses per agent. (A per-call
/// stable-content hash comparator and an aggregate `hit_rate()` accessor
/// used to live here too; neither ever gained a production consumer —
/// removed per YAGNI. Aggregate hit-rate lives in the usage DB rollup and
/// the Panel Usage view, which are per-agent and persisted.)
pub struct CacheMonitor {
    state: Mutex<HashMap<String, AgentCacheState>>,
}

impl CacheMonitor {
    /// Create a new monitor with empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
        }
    }

    /// Record cache usage from a completed LLM call attributed to `agent_id`.
    ///
    /// A `cache_read_tokens` value of `None` or `Some(0)` counts as a miss —
    /// but only once the agent is *armed* (has ever reported non-zero cache
    /// read or creation tokens). After 3 or more consecutive armed misses
    /// (and at least 4 total calls) a `warn!` is emitted so operators can
    /// investigate a possible stable-prefix change.
    pub fn record_cache_usage(
        &self,
        agent_id: &str,
        cache_read_tokens: Option<u32>,
        cache_creation_tokens: Option<u32>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());

        if !state.contains_key(agent_id) && state.len() >= MAX_TRACKED_AGENTS {
            // Evict an arbitrary entry to stay bounded.
            if let Some(k) = state.keys().next().cloned() {
                state.remove(&k);
            }
        }
        let agent = state.entry(agent_id.to_string()).or_default();

        agent.total_calls += 1;

        let reads = u64::from(cache_read_tokens.unwrap_or(0));
        let writes = u64::from(cache_creation_tokens.unwrap_or(0));
        if reads > 0 || writes > 0 {
            agent.armed = true;
        }

        // A call counts as healthy only when the cache was read *at least as
        // much as* it was written.
        //
        // Treating any non-zero read as healthy made the watchdog blind to the
        // failure mode this codebase actually ships into. The layout is one
        // breakpoint on the small stable system block and three on the
        // conversation; when a prefix ahead of the message breakpoints churns,
        // the system block still hits on every call, so `reads > 0` held
        // forever, the streak reset on every call, and the warn could never
        // accumulate — while 100% of the history was re-created at 1.25x.
        // Read-dominance separates "hitting" from "re-creating and hitting a
        // few hundred bytes".
        let healthy = reads > 0 && reads >= writes;

        if healthy {
            agent.consecutive_misses = 0;
            agent.warned = false;
        } else if agent.armed {
            agent.consecutive_misses += 1;
            if agent.consecutive_misses >= 3 && agent.total_calls > 3 && !agent.warned {
                agent.warned = true;
                tracing::warn!(
                    agent_id = %agent_id,
                    consecutive_misses = agent.consecutive_misses,
                    total_calls = agent.total_calls,
                    cache_read_tokens = reads,
                    cache_creation_tokens = writes,
                    "prompt cache is being re-created rather than read — a prefix \
                     ahead of the message breakpoints is churning, or the stable \
                     prefix changed"
                );
            }
        }
    }

    /// Notify the monitor that a compaction has occurred for `agent_id`.
    ///
    /// Compaction legitimately breaks the prompt cache (the message list is
    /// rewritten), so consecutive-miss tracking is reset to avoid false
    /// positive warnings immediately after compaction. The reset is scoped
    /// to the compacting agent when its id is known — a global reset would
    /// wipe every OTHER agent's in-progress miss streak on each compaction
    /// and mute the watchdog process-wide in a busy swarm. `None` falls back
    /// to the global reset for call sites without an agent identity.
    pub fn notify_compaction(&self, agent_id: Option<&str>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match agent_id {
            Some(id) => {
                if let Some(agent) = state.get_mut(id) {
                    agent.consecutive_misses = 0;
                    agent.warned = false;
                }
            }
            None => {
                for agent in state.values_mut() {
                    agent.consecutive_misses = 0;
                    agent.warned = false;
                }
            }
        }
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
// reports its cache token counts (keyed by agent id) to the singleton.
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

    fn misses(monitor: &CacheMonitor, agent: &str) -> u32 {
        monitor
            .state
            .lock()
            .expect("lock poisoned")
            .get(agent)
            .map_or(0, |s| s.consecutive_misses)
    }

    #[test]
    fn unarmed_agent_never_counts_misses() {
        // An endpoint that never reports cache usage (mock, Ollama, caching
        // off) must not accumulate misses — its readings carry no signal.
        let monitor = CacheMonitor::new();
        for _ in 0..10 {
            monitor.record_cache_usage("no-cache-agent", None, None);
        }
        assert_eq!(misses(&monitor, "no-cache-agent"), 0);
    }

    #[test]
    fn cache_write_arms_and_counts_as_miss() {
        // A cold cache write (creation > 0, read == 0) is real cache activity:
        // it arms the agent AND counts as the first consecutive miss.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("a", None, Some(500));
        assert_eq!(misses(&monitor, "a"), 1);
        // A subsequent hit resets the streak.
        monitor.record_cache_usage("a", Some(100), None);
        assert_eq!(misses(&monitor, "a"), 0);
    }

    #[test]
    fn agents_are_tracked_independently() {
        // A cache-hitting agent must not reset another agent's miss streak.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("broken", Some(10), None); // arm via a hit
        monitor.record_cache_usage("broken", None, Some(1)); // miss 1
        monitor.record_cache_usage("healthy", Some(999), None); // unrelated hit
        monitor.record_cache_usage("broken", None, Some(1)); // miss 2
        assert_eq!(misses(&monitor, "broken"), 2);
        assert_eq!(misses(&monitor, "healthy"), 0);
    }

    #[test]
    fn compaction_resets_consecutive_misses() {
        let monitor = CacheMonitor::new();
        // Drive up consecutive misses past the warning threshold
        // (needs > 3 total calls and >= 3 consecutive misses).
        monitor.record_cache_usage("a", Some(100), None); // hit — arms
        monitor.record_cache_usage("a", None, None); // miss
        monitor.record_cache_usage("a", None, None); // miss
        monitor.record_cache_usage("a", None, None); // miss — warn fires

        // Compaction resets the counter
        monitor.notify_compaction(Some("a"));

        // After reset, a single miss should NOT trigger a warning
        // (consecutive_misses is back to 0, so 1 miss = 1 consecutive)
        monitor.record_cache_usage("a", None, None);
        assert_eq!(
            misses(&monitor, "a"),
            1,
            "miss count should restart from 1 after compaction"
        );
    }

    #[test]
    fn scoped_compaction_reset_preserves_other_agents_streaks() {
        // Agent A compacting must not wipe agent B's in-progress miss streak
        // — a global reset would mute the watchdog process-wide whenever any
        // agent in a busy swarm compacts.
        let monitor = CacheMonitor::new();
        monitor.record_cache_usage("b", Some(10), None); // arm B
        monitor.record_cache_usage("b", None, None); // B miss 1
        monitor.record_cache_usage("b", None, None); // B miss 2
        monitor.record_cache_usage("a", Some(10), None); // arm A
        monitor.record_cache_usage("a", None, None); // A miss 1

        monitor.notify_compaction(Some("a")); // A compacts

        assert_eq!(misses(&monitor, "a"), 0, "compacting agent reset");
        assert_eq!(misses(&monitor, "b"), 2, "other agent's streak survives");

        // Global fallback (no identity) still resets everyone.
        monitor.notify_compaction(None);
        assert_eq!(misses(&monitor, "b"), 0);
    }

    #[test]
    fn tracked_agents_stay_bounded() {
        let monitor = CacheMonitor::new();
        for i in 0..(MAX_TRACKED_AGENTS + 10) {
            monitor.record_cache_usage(&format!("agent-{i}"), Some(1), None);
        }
        let len = monitor.state.lock().expect("lock poisoned").len();
        assert!(len <= MAX_TRACKED_AGENTS, "map stayed bounded, got {len}");
    }
}
