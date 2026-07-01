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
#[derive(Default)]
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
    fn cache_round_trips_and_model_change_misses() {
        let cache = OverheadCache::default();
        assert_eq!(cache.get("agentA", "kimi"), None);
        cache.insert("agentA", "kimi", 12_345);
        assert_eq!(cache.get("agentA", "kimi"), Some(12_345));
        // Model change = different key = natural miss (D5).
        assert_eq!(cache.get("agentA", "claude"), None);
    }
}
