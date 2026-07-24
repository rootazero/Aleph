//! Context-occupancy estimation for sessions that never ran an LLM turn.
//!
//! Pure token arithmetic + a per-(agent, model) static-overhead cache, so a
//! freshly-opened conversation can show a `≈N%` gauge before its first reply.
//! No LLM call, no decision — scaffolding only (R7/R10). Reuses the
//! `budget::pressure` estimators so the whole estimate is self-consistent.

use std::collections::HashMap;
use std::sync::Mutex;

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

/// Per-(agent_id, model_id) cache of the static prompt overhead
/// (system prompt + tool schemas) in tokens. Keyed so a model change is a
/// natural miss; no eviction (overhead drifts only on tool/skill/identity
/// edits, where a slightly stale `≈` estimate is acceptable — spec D5).
#[derive(Debug, Default)]
pub struct OverheadCache {
    inner: Mutex<HashMap<(String, String), usize>>,
}

impl OverheadCache {
    #[must_use]
    pub fn get(&self, agent_id: &str, model: &str) -> Option<usize> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(agent_id.to_string(), model.to_string()))
            .copied()
    }

    pub fn insert(&self, agent_id: &str, model: &str, overhead: usize) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((agent_id.to_string(), model.to_string()), overhead);
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
        assert_eq!(cache.get("agentA", "kimi"), None);
        cache.insert("agentA", "kimi", 12_345);
        assert_eq!(cache.get("agentA", "kimi"), Some(12_345));
        // Model change = different key = natural miss (D5).
        assert_eq!(cache.get("agentA", "claude"), None);
    }
}
