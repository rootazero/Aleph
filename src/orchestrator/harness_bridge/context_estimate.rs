//! Context-occupancy estimation for sessions that never ran an LLM turn.
//!
//! Pure token arithmetic + a per-(session, model) prompt-overhead cache, so a
//! freshly-opened conversation can show a `≈N%` gauge before its first reply.
//! No LLM call, no decision — scaffolding only (R7/R10). Reuses the
//! `budget::pressure` estimators so the whole estimate is self-consistent.

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;

use crate::context::budget::pressure::{
    estimate_message_tokens_aware, estimate_tokens_aware, DEFAULT_PROSE_RATIO,
};
use crate::providers::message::UnifiedMessage;

/// Prose anchor for the estimate. CJK/code density overrides still apply inside
/// `estimate_tokens_aware`, so this only sets the natural-language baseline.
pub const ESTIMATE_RATIO: f64 = DEFAULT_PROSE_RATIO;

/// Estimated context occupancy for a session's *next* prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextEstimate {
    pub used_tokens: u32,
    pub window_tokens: u32,
}

/// How many (session, model) overhead entries to retain. Sessions are
/// unbounded — agents were not — so the map that used to hold this needs a
/// ceiling. Sized for a plausible sidebar-switching working set; an entry is
/// two short strings plus a `usize`, so the whole cache is a few KB.
const OVERHEAD_CACHE_CAPACITY: usize = 64;

/// Per-(session_key, model_id) cache of the assembled prompt overhead
/// (system prompt + tool schemas) in tokens.
///
/// **Keyed by session, not by agent.** The measured prompt is assembled from a
/// *real* `SessionId`, and `prompt_build::resolve_prompt_context` fills it from
/// session-scoped reads: the execution plan, standing goal, timer loop, welded
/// strategy, governance topology, voice flag, and the curated-memory freeze
/// point. Keying that value by `(agent, model)` served one conversation's
/// overhead as another's — the Panel gauge showed session A's plan/strategy
/// bytes while sitting in session B. A session key is strictly more
/// discriminating *and* one field smaller: `SessionKey` already encodes the
/// agent id.
///
/// Model stays in the key so a model change is a natural miss. No TTL —
/// overhead drifts only on tool/skill/identity edits, where a slightly stale
/// `≈` estimate is acceptable (spec D5) — but the map is now a bounded LRU.
#[derive(Debug)]
pub struct OverheadCache {
    inner: Mutex<LruCache<(String, String), usize>>,
}

impl Default for OverheadCache {
    fn default() -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(OVERHEAD_CACHE_CAPACITY)
                    .unwrap_or_else(|| unreachable!("OVERHEAD_CACHE_CAPACITY > 0")),
            )),
        }
    }
}

impl OverheadCache {
    /// Read the cached overhead for this session under `model`, marking the
    /// entry most-recently-used.
    #[must_use]
    pub fn get(&self, session_key: &str, model: &str) -> Option<usize> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(session_key.to_string(), model.to_string()))
            .copied()
    }

    pub fn insert(&self, session_key: &str, model: &str, overhead: usize) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put((session_key.to_string(), model.to_string()), overhead);
    }
}

/// Token cost of the tool schemas as sent on the wire (name + description +
/// JSON params), content-aware. Mirrors the budget sensor's per-tool charge but
/// reuses the `budget::pressure` estimator (kept here, not imported from
/// `harness`, to avoid widening the harness boundary — R10).
#[must_use]
pub fn tool_schema_tokens(tools: &[crate::tool_metadata::ToolDefinition], ratio: f64) -> usize {
    tools
        .iter()
        .map(|t| {
            estimate_tokens_aware(&t.name, ratio)
                + estimate_tokens_aware(&t.description, ratio)
                + estimate_tokens_aware(&t.parameters.to_string(), ratio)
        })
        .sum()
}

/// Compose the final estimate: static overhead + this session's history message
/// tokens, against the resolved window. Pure → unit-testable without a runner.
#[must_use]
pub fn compose_estimate(
    overhead_tokens: usize,
    history: &[UnifiedMessage],
    window: u32,
    ratio: f64,
) -> ContextEstimate {
    let msg_tokens: usize = history
        .iter()
        .map(|m| estimate_message_tokens_aware(m, ratio))
        .sum();
    let used = overhead_tokens.saturating_add(msg_tokens);
    ContextEstimate {
        used_tokens: u32::try_from(used).unwrap_or(u32::MAX),
        window_tokens: window,
    }
}

/// Cap a raw estimate at the occupancy the context-management system lets a
/// real turn reach. The raw sum walks the session's *uncompacted* event log
/// (compaction rewrites only the in-memory prompt, never the log — see
/// `harness/agent/prompt.rs`), so a long, previously-compacted session would
/// otherwise estimate near/above 100% while its next real turn compacts back
/// under the warning band. `warning_threshold × window` is a deliberate upper
/// bound of that band (the threshold is configured against the *usable*
/// budget ≤ window), so the cap never understates the real next-turn
/// occupancy regime. No-op when compaction is disabled (call sites pass the
/// raw estimate through) or when the estimate is already below the cap.
#[must_use]
pub fn cap_by_compaction(est: ContextEstimate, warning_threshold: f64) -> ContextEstimate {
    if !(warning_threshold > 0.0 && warning_threshold <= 1.0) {
        return est;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let cap = (f64::from(est.window_tokens) * warning_threshold).round() as u32;
    ContextEstimate {
        used_tokens: est.used_tokens.min(cap),
        window_tokens: est.window_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    #[test]
    fn compose_empty_history_is_overhead_only() {
        let est = compose_estimate(10_000, &[], 200_000, ESTIMATE_RATIO);
        assert_eq!(est.used_tokens, 10_000);
        assert_eq!(est.window_tokens, 200_000);
    }

    #[test]
    fn compose_adds_history_message_tokens() {
        let history = vec![UnifiedMessage::user("hello there, this is a user turn")];
        let est = compose_estimate(10_000, &history, 200_000, ESTIMATE_RATIO);
        assert!(
            est.used_tokens > 10_000,
            "history tokens must add on top of overhead"
        );
    }

    #[test]
    fn tool_schema_tokens_empty_is_zero() {
        assert_eq!(tool_schema_tokens(&[], ESTIMATE_RATIO), 0);
    }

    #[test]
    fn cap_by_compaction_caps_overflow_at_warning_band() {
        // A previously-compacted long session: raw walk of the uncompacted
        // log lands above the window, but the next real turn compacts back
        // under the warning band — the estimate must reflect that regime.
        let raw = ContextEstimate {
            used_tokens: 300_000,
            window_tokens: 200_000,
        };
        let capped = cap_by_compaction(raw, 0.70);
        assert_eq!(capped.used_tokens, 140_000);
        assert_eq!(capped.window_tokens, 200_000);
    }

    #[test]
    fn cap_by_compaction_passes_small_estimates_through() {
        let raw = ContextEstimate {
            used_tokens: 50_000,
            window_tokens: 200_000,
        };
        assert_eq!(cap_by_compaction(raw, 0.70), raw);
    }

    #[test]
    fn cap_by_compaction_ignores_degenerate_thresholds() {
        let raw = ContextEstimate {
            used_tokens: 300_000,
            window_tokens: 200_000,
        };
        // 0.0 and >1.0 are defensive no-ops, mirroring the budget config gate.
        assert_eq!(cap_by_compaction(raw, 0.0), raw);
        assert_eq!(cap_by_compaction(raw, 1.5), raw);
    }

    #[test]
    fn cache_round_trips_and_model_change_misses() {
        let cache = OverheadCache::default();
        assert_eq!(cache.get("agent:main:s1", "kimi"), None);
        cache.insert("agent:main:s1", "kimi", 12_345);
        assert_eq!(cache.get("agent:main:s1", "kimi"), Some(12_345));
        // Model change = different key = natural miss (D5).
        assert_eq!(cache.get("agent:main:s1", "claude"), None);
    }

    #[test]
    fn two_sessions_of_one_agent_do_not_share_an_entry() {
        // The cached value is measured from a prompt assembled with a REAL
        // session (execution plan, standing goal, strategy, graph topology and
        // the curated freeze point are all session-scoped). An agent-scoped key
        // leaked session A's overhead into session B's gauge.
        let cache = OverheadCache::default();
        cache.insert("agent:main:s1", "kimi", 40_000);
        assert_eq!(cache.get("agent:main:s2", "kimi"), None);
        cache.insert("agent:main:s2", "kimi", 9_000);
        assert_eq!(cache.get("agent:main:s1", "kimi"), Some(40_000));
        assert_eq!(cache.get("agent:main:s2", "kimi"), Some(9_000));
    }

    #[test]
    fn capacity_is_bounded_and_evicts_least_recently_used() {
        // Sessions are unbounded, so the old never-evicted HashMap grew with
        // every conversation ever gauged.
        let cache = OverheadCache::default();
        for i in 0..OVERHEAD_CACHE_CAPACITY {
            cache.insert(&format!("agent:main:s{i}"), "kimi", i);
        }
        // Touch the oldest so it is no longer the eviction victim.
        assert_eq!(cache.get("agent:main:s0", "kimi"), Some(0));
        cache.insert("agent:main:overflow", "kimi", 999);

        assert_eq!(
            cache.get("agent:main:s0", "kimi"),
            Some(0),
            "recently-read entry must survive"
        );
        assert_eq!(
            cache.get("agent:main:s1", "kimi"),
            None,
            "least-recently-used entry must be the one evicted"
        );
        assert_eq!(
            cache
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            OVERHEAD_CACHE_CAPACITY
        );
    }
}
