//! Per-turn aggregate budget for tool results (Layer 3 of the
//! `compress → persist-if-large → turn-spill` cascade).
//!
//! Tracks the cumulative `tokens_in_context` of results produced inside
//! a single Think→Act turn. When the running total exceeds
//! `max_turn_tokens`, [`TurnResultBudget::record`] returns
//! [`SpillInstruction`]s — the caller (typically `harness/agent/act.rs`)
//! persists the in-context text to disk via the shared `ToolResultStore`
//! and rewrites the in-flight history entry from full text to the
//! returned marker.
//!
//! Spill order is **LIFO**: the most recently recorded non-persisted
//! result is the first candidate. Older results in the same turn have
//! either been processed already or were small enough to stay verbatim,
//! so dropping them adds little value while costing more recall.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Default per-turn budget. Mirrors hermes' `MAX_TURN_BUDGET_CHARS=200_000`,
/// converted to ~50 000 tokens at the standard ~4 chars/token ratio.
pub const DEFAULT_MAX_TURN_TOKENS: usize = 50_000;

// =============================================================================
// Process-wide installer
// =============================================================================

static GLOBAL_BUDGET: OnceLock<Arc<TurnResultBudget>> = OnceLock::new();

/// Install the process-wide `TurnResultBudget`. Called once at server
/// boot. Idempotent — subsequent calls are silently ignored.
pub fn set_global_turn_result_budget(budget: Arc<TurnResultBudget>) {
    let _ = GLOBAL_BUDGET.set(budget);
}

/// Read the process-wide `TurnResultBudget`, if installed.
pub fn global_turn_result_budget() -> Option<Arc<TurnResultBudget>> {
    GLOBAL_BUDGET.get().cloned()
}

/// Turn identifier. Wraps the same `Uuid` used by
/// [`crate::session::events::TurnId`] so the harness can pass its
/// session-side id straight through without an extra mapping table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TurnId(pub uuid::Uuid);

impl TurnId {
    pub fn new(id: uuid::Uuid) -> Self {
        Self(id)
    }

    #[cfg(test)]
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl From<uuid::Uuid> for TurnId {
    fn from(id: uuid::Uuid) -> Self {
        Self(id)
    }
}

/// One tool result recorded into the turn budget.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub call_id: String,
    pub tool_name: String,
    pub tokens_in_context: usize,
    pub in_context_text: String,
    /// `true` if the result already arrived as a `[Full output persisted:
    /// ...]` marker (Layer 2 already handled it). Such entries are not
    /// spilled again by Layer 3.
    pub already_persisted: bool,
}

/// Instruction for the caller to evict a recorded result.
#[derive(Debug, Clone)]
pub struct SpillInstruction {
    pub call_id: String,
    pub tool_name: String,
    /// The full text that was in context. The caller persists this via
    /// the shared `ToolResultStore` and rewrites the history entry's
    /// text to the persisted marker.
    pub original_text: String,
}

#[derive(Debug, Default)]
struct TurnState {
    /// Stack ordered oldest → newest. Spill scans from the back.
    results: Vec<TurnResult>,
    cumulative: usize,
}

/// LIFO turn-budget tracker. Cheap to `Clone` — wraps an `Arc<Mutex<_>>`.
#[derive(Debug, Clone)]
pub struct TurnResultBudget {
    inner: Arc<Mutex<HashMap<TurnId, TurnState>>>,
    max_turn_tokens: usize,
}

impl TurnResultBudget {
    pub fn new(max_turn_tokens: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max_turn_tokens,
        }
    }

    pub fn max_turn_tokens(&self) -> usize {
        self.max_turn_tokens
    }

    /// Begin tracking a new turn. Idempotent — re-entering an existing
    /// `TurnId` is a no-op rather than an error so retries from upstream
    /// do not corrupt state.
    pub fn begin_turn(&self, id: TurnId) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(id).or_default();
    }

    /// Record a new result; return spill instructions if the cumulative
    /// total exceeds the budget. Spill order is LIFO over non-persisted
    /// entries; already-persisted entries are skipped.
    pub fn record(&self, id: &TurnId, result: TurnResult) -> Vec<SpillInstruction> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let state = g.entry(*id).or_default();
        state.cumulative = state.cumulative.saturating_add(result.tokens_in_context);
        state.results.push(result);

        let mut instructions = Vec::new();
        while state.cumulative > self.max_turn_tokens {
            let idx = state
                .results
                .iter()
                .enumerate()
                .rev()
                .find(|(_, r)| !r.already_persisted)
                .map(|(i, _)| i);
            let Some(idx) = idx else {
                break; // Nothing left to spill; remain over budget.
            };
            let r = &mut state.results[idx];
            instructions.push(SpillInstruction {
                call_id: r.call_id.clone(),
                tool_name: r.tool_name.clone(),
                original_text: std::mem::take(&mut r.in_context_text),
            });
            // Approximate: spilling the result is expected to reduce its
            // in-context footprint to ~10 % of the original (the marker
            // length). Credit 90 % back to the cumulative.
            let credit = r.tokens_in_context.saturating_mul(9) / 10;
            state.cumulative = state.cumulative.saturating_sub(credit);
            r.tokens_in_context = r.tokens_in_context.saturating_sub(credit);
            r.already_persisted = true;
        }
        instructions
    }

    /// Clear tracking for the given turn. Safe to call on a missing
    /// turn (no-op).
    pub fn end_turn(&self, id: &TurnId) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(id);
    }

    #[cfg(test)]
    pub fn cumulative(&self, id: &TurnId) -> usize {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.get(id).map(|s| s.cumulative).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tid(seq: u64) -> TurnId {
        // Deterministic UUIDs derived from the seq so tests with the
        // same `seq` collide intentionally (lifecycle assertions).
        let bytes = seq.to_be_bytes();
        let mut buf = [0u8; 16];
        buf[..8].copy_from_slice(&bytes);
        TurnId::new(uuid::Uuid::from_bytes(buf))
    }

    fn result(id: &str, tokens: usize) -> TurnResult {
        TurnResult {
            call_id: id.into(),
            tool_name: "bash".into(),
            tokens_in_context: tokens,
            in_context_text: "x".repeat(tokens.saturating_mul(4)),
            already_persisted: false,
        }
    }

    #[test]
    fn begin_end_lifecycle_clears_state() {
        let b = TurnResultBudget::new(100);
        let id = tid(1);
        b.begin_turn(id);
        let spilled = b.record(&id, result("c1", 30));
        assert!(spilled.is_empty());
        assert_eq!(b.cumulative(&id), 30);
        b.end_turn(&id);
        assert_eq!(b.cumulative(&id), 0);
    }

    #[test]
    fn under_budget_no_spill() {
        let b = TurnResultBudget::new(100);
        let id = tid(1);
        b.begin_turn(id);
        let s = b.record(&id, result("c1", 50));
        assert!(s.is_empty());
        assert_eq!(b.cumulative(&id), 50);
    }

    #[test]
    fn over_budget_spills_newest_first() {
        let b = TurnResultBudget::new(100);
        let id = tid(1);
        b.begin_turn(id);
        b.record(&id, result("c1", 40));
        b.record(&id, result("c2", 40));
        let instr = b.record(&id, result("c3", 40));
        // Cumulative reaches 120; spilling newest (c3) credits 36
        // tokens back, reducing to 84 — under budget. Exactly one spill.
        assert_eq!(instr.len(), 1, "expected 1 spill, got: {:?}", instr);
        assert_eq!(instr[0].call_id, "c3");
    }

    #[test]
    fn already_persisted_entries_are_not_respilled() {
        let b = TurnResultBudget::new(50);
        let id = tid(1);
        b.begin_turn(id);
        let mut already = result("c1", 100);
        already.already_persisted = true;
        let instr = b.record(&id, already);
        // Only entry is already persisted → no spill candidate.
        assert!(instr.is_empty());
        // Cumulative still tracks the entry's tokens.
        assert_eq!(b.cumulative(&id), 100);
    }

    #[test]
    fn multiple_spills_until_under_budget() {
        let b = TurnResultBudget::new(40);
        let id = tid(1);
        b.begin_turn(id);
        b.record(&id, result("c1", 30));
        b.record(&id, result("c2", 30));
        // After c2: cumulative = 60 > 40 → spill c2 (credit 27) → 33 → under.
        // After c3: cumulative = 33 + 30 = 63 > 40 → spill c3 (credit 27) → 36 → under.
        let instr_c3 = b.record(&id, result("c3", 30));
        assert_eq!(instr_c3.len(), 1);
        assert_eq!(instr_c3[0].call_id, "c3");
    }

    #[test]
    fn end_turn_on_missing_id_is_noop() {
        let b = TurnResultBudget::new(100);
        let id = tid(99);
        b.end_turn(&id);
        assert_eq!(b.cumulative(&id), 0);
    }
}
