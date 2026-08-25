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
use std::sync::Arc;

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Mutex;

/// Default per-turn budget. Mirrors hermes' `MAX_TURN_BUDGET_CHARS=200_000`,
/// converted to ~50 000 tokens at the standard ~4 chars/token ratio.
///
/// This is the **ceiling**, not the value: [`budget_for_window`] clamps down
/// from it on small windows. See that function for why the bare constant is
/// wrong on a 32k model.
pub const DEFAULT_MAX_TURN_TOKENS: usize = 50_000;

/// Fraction of the model's usable window one tool result may occupy (Layer 2).
const RESULT_WINDOW_FRACTION: f64 = 0.15;

/// Fraction of the model's usable window a whole Think→Act turn's tool results
/// may occupy (Layer 3).
const TURN_WINDOW_FRACTION: f64 = 0.30;

/// Floor for the per-result budget. Below this a tool result is too mutilated
/// to be worth keeping in context at all, so we stop scaling down.
const MIN_RESULT_TOKENS: usize = 2_000;

/// Floor for the per-turn budget. Same rationale as [`MIN_RESULT_TOKENS`].
const MIN_TURN_TOKENS: usize = 4_000;

/// Size the two tool-output budgets — `(per_result, per_turn)` — for a model
/// whose usable context window is `token_budget` tokens.
///
/// **This is a small-window clamp-down, not "bigger models get more."** Both
/// values clamp *up* to today's constants ([`DEFAULT_MAX_TURN_TOKENS`] and
/// `result_processing::DEFAULT_RESULT_BUDGET_TOKENS`), so every model with a
/// window above ~53k / ~167k tokens gets byte-for-byte the same budgets it gets
/// today. Nothing here loosens anything.
///
/// What it fixes is the other end. The two limits were fixed constants that
/// never looked at the model, and hermes — whose `MAX_TURN_BUDGET_CHARS` the
/// per-turn constant was copied from — has since made both window-relative. On
/// a 32k local model the old numbers are absurd: one `bash` result at 8k tokens
/// eats a quarter of the window and lands in the compaction-protected fresh
/// tail, while the 50k per-turn cap is 156 % of the entire window and can
/// therefore never fire. The combination overflows the context, and on the
/// OpenAI-compatible endpoints those small models live behind an overflow is
/// fatal to the run rather than recoverable.
///
/// Pure arithmetic over a figure the config already derived — it never reads a
/// message, so it is a static budget, not an intent filter (R10).
#[must_use]
pub fn budget_for_window(token_budget: u64) -> (usize, usize) {
    let window = token_budget as f64;
    let per_result = ((window * RESULT_WINDOW_FRACTION) as usize).clamp(
        MIN_RESULT_TOKENS,
        crate::tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS,
    );
    let per_turn =
        ((window * TURN_WINDOW_FRACTION) as usize).clamp(MIN_TURN_TOKENS, DEFAULT_MAX_TURN_TOKENS);
    (per_result, per_turn)
}

// =============================================================================
// Process-wide installer
// =============================================================================

/// `FailsOpen`. Both production readers end an `Option` chain with this handle
/// (`runner_impl`: `self.turn_budget.or(windowed).or_else(global)`;
/// `subagent_spawner`: a per-window budget `.or_else(global)`), and
/// `runner_impl`'s own comment states the consequence of the chain running out:
/// "`None` (nothing anywhere) keeps the legacy behavior — Layer 2 / Layer 3 are
/// inert."
///
/// Inert is the open direction, not the closed one. This is the *only* cap on
/// how much tool output one turn may keep in context; without it nothing spills
/// and nothing is evicted, and `budget_for_window`'s doc a few lines above says
/// an overflow on the small-window models this exists for "is fatal to the run
/// rather than recoverable". So a missing handle does not disable a feature —
/// it removes a bound while every caller keeps behaving as though it were
/// enforced.
static GLOBAL_BUDGET: CapabilitySlot<Arc<TurnResultBudget>> =
    CapabilitySlot::new("tools/turn-budget", MissingSemantics::FailsOpen);

/// Install the process-wide `TurnResultBudget`. Called once at server
/// boot. Idempotent — subsequent calls are silently ignored.
#[inline]
pub fn set_global_turn_result_budget(budget: Arc<TurnResultBudget>) {
    let _ = GLOBAL_BUDGET.install(budget);
}

/// Read the process-wide `TurnResultBudget`, if installed.
///
/// ⚠️ `None` says nothing about whether boot reached this slot. Ask
/// [`global_turn_result_budget_slot`]`().outcome()` for that — the difference
/// between "this deployment sets no turn cap" and "boot never got here" is
/// invisible from this return value, and the second one is a silent regression.
#[inline]
pub fn global_turn_result_budget() -> Option<Arc<TurnResultBudget>> {
    GLOBAL_BUDGET.get().cloned()
}

/// The handle above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_turn_result_budget_slot() -> &'static dyn SlotStatus {
    &GLOBAL_BUDGET
}

/// Turn identifier. Wraps the same `Uuid` used by
/// [`crate::session::events::TurnId`] so the harness can pass its
/// session-side id straight through without an extra mapping table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TurnId(pub uuid::Uuid);

impl TurnId {
    #[must_use]
    pub const fn new(id: uuid::Uuid) -> Self {
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
    #[must_use]
    pub fn new(max_turn_tokens: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            max_turn_tokens,
        }
    }

    #[must_use]
    pub const fn max_turn_tokens(&self) -> usize {
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
    #[must_use]
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

    // ========================================================================
    // The process-global handle, as a capability slot
    // ========================================================================

    /// See `session::service::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    ///
    /// `FailsOpen` is pinned rather than merely written down: it is the whole
    /// reason this handle is worth a roster entry. Softened to `FailsClosed` it
    /// would tell an operator that a missing turn cap is the safe direction,
    /// which is backwards — see the declaration's doc.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = global_turn_result_budget_slot();
        assert_eq!(slot.id(), "tools/turn-budget");
        assert!(matches!(slot.missing(), MissingSemantics::FailsOpen));
    }

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
        let _ = b.record(&id, result("c1", 40));
        let _ = b.record(&id, result("c2", 40));
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
        let _ = b.record(&id, result("c1", 30));
        let _ = b.record(&id, result("c2", 30));
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

    // ---- window-aware budgets (B14) ----

    use crate::tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS;

    #[test]
    fn large_window_is_byte_for_byte_todays_constants() {
        // The clamp is upward: a 200k-window model must see exactly the budgets
        // it sees today, or this change is a behavior regression dressed up as a
        // fix.
        let (per_result, per_turn) = budget_for_window(200_000);
        assert_eq!(per_result, DEFAULT_RESULT_BUDGET_TOKENS);
        assert_eq!(per_turn, DEFAULT_MAX_TURN_TOKENS);
        // 1M window: still the same ceilings, not 150k/300k.
        assert_eq!(
            budget_for_window(1_000_000),
            (DEFAULT_RESULT_BUDGET_TOKENS, DEFAULT_MAX_TURN_TOKENS)
        );
    }

    #[test]
    fn small_window_turn_cap_can_actually_fire() {
        // The bug: on a 16k usable window the fixed 50k per-turn cap is >3x the
        // whole window, so Layer 3 never spills and the context overflows.
        let window = 16_000u64;
        let (per_result, per_turn) = budget_for_window(window);
        assert!(
            per_turn < window as usize,
            "per-turn cap {per_turn} must fit inside the {window}-token window"
        );
        assert!(
            per_result < DEFAULT_RESULT_BUDGET_TOKENS,
            "per-result must clamp below the 8k constant on a small window, got {per_result}"
        );
        // 30 % / 15 % of 16k.
        assert_eq!((per_result, per_turn), (2_400, 4_800));
    }

    #[test]
    fn tiny_window_hits_the_floors_not_zero() {
        // A mis-declared or genuinely tiny window must not scale the budgets to
        // nothing — a 0-token result budget would truncate every tool result to
        // an empty string.
        let (per_result, per_turn) = budget_for_window(1_000);
        assert_eq!(per_result, MIN_RESULT_TOKENS);
        assert_eq!(per_turn, MIN_TURN_TOKENS);
        assert_eq!(budget_for_window(0), (MIN_RESULT_TOKENS, MIN_TURN_TOKENS));
    }
}
