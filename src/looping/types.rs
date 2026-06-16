//! In-memory loop state: a per-session timer-driven repeat. Mirrors
//! `goal::types` but is NEVER persisted — the registry lives in process
//! memory only, so a daemon restart clears all loops ("随会话消亡").
//!
//! Distinct from `goal` (condition-stop) and `cron` (persistent, own session):
//! a loop is "current session, multi-turn, clock-gated, never self-stops".

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How the loop picks the delay before its next tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cadence {
    /// Fixed interval: wait this many ms between ticks.
    Fixed { interval_ms: u64 },
    /// Model-paced: the model sets `next_wake_ms` each tick via
    /// `loop(action='update', next_wake=...)`. If unset, use `fallback_ms`.
    ModelPaced { fallback_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopStatus {
    Active,
    Stopped,
}

/// One session's loop. Immutable-update style: `with_*` / `spent_*` return
/// new copies, never mutate in place (project immutability rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoopState {
    pub session_id: String,
    /// The fixed prompt re-injected verbatim each tick.
    pub prompt: String,
    pub cadence: Cadence,
    /// Model-paced override (absolute epoch ms of the next wake). `None` →
    /// fall back to the cadence default. For `Fixed` cadence this is ignored.
    #[serde(default)]
    pub next_wake_ms: Option<u64>,
    pub iterations_used: u32,
    /// Optional safety cap: stop after this many ticks. `None` → unbounded.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Optional safety cap: absolute epoch-ms wall-clock deadline.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    /// Optional soft token budget: stop the loop once it has consumed more than
    /// this many session tokens since the baseline was captured. Enforced by the
    /// continuation hook via [`Self::over_budget`] (codex `tokenStartFresh`
    /// parity, mirroring `goal::Goal`).
    #[serde(default)]
    pub token_budget: Option<u64>,
    /// Session cumulative-token baseline at the moment budget enforcement began.
    /// Seeded lazily by the continuation hook on the first tick that sees a
    /// budget set (the `loop` tool has no live token counter at creation). Until
    /// `baseline_captured`, the token budget is not enforced. `#[serde(default)]`
    /// → old payloads read 0.
    #[serde(default)]
    pub tokens_at_start: u64,
    /// Whether `tokens_at_start` holds a real session-token baseline yet. Mirrors
    /// `goal::Goal::baseline_captured`. `#[serde(default)]` → old payloads read
    /// `false`.
    #[serde(default)]
    pub baseline_captured: bool,
    pub status: LoopStatus,
    pub created_at_ms: u64,
}

impl LoopState {
    #[must_use]
    pub fn new(session_id: &str, prompt: &str, cadence: Cadence, now_ms: u64) -> Self {
        Self {
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
            cadence,
            next_wake_ms: None,
            iterations_used: 0,
            max_iterations: None,
            deadline_ms: None,
            token_budget: None,
            tokens_at_start: 0,
            baseline_captured: false,
            status: LoopStatus::Active,
            created_at_ms: now_ms,
        }
    }

    #[must_use]
    pub fn spent_iteration(mut self) -> Self {
        self.iterations_used = self.iterations_used.saturating_add(1);
        self
    }

    #[must_use]
    pub fn with_status(mut self, status: LoopStatus) -> Self {
        self.status = status;
        self
    }

    #[must_use]
    pub const fn with_max_iterations(mut self, max: Option<u32>) -> Self {
        self.max_iterations = max;
        self
    }

    #[must_use]
    pub const fn with_deadline_ms(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }

    #[must_use]
    pub const fn with_token_budget(mut self, budget: Option<u64>) -> Self {
        self.token_budget = budget;
        self
    }

    /// Seed the real session-token baseline and mark it captured. Called once by
    /// the continuation hook on the first tick that sees a budget set. Mirrors
    /// `goal::Goal::with_baseline` (loop has no `updated_at` to bump).
    #[must_use]
    pub const fn with_baseline(mut self, tokens_at_start: u64) -> Self {
        self.tokens_at_start = tokens_at_start;
        self.baseline_captured = true;
        self
    }

    /// Tokens consumed since the baseline. Saturates on a counter reset so a
    /// backwards-moving total never reports phantom spend (mirrors `goal`).
    #[must_use]
    pub const fn tokens_used(&self, now_total_tokens: u64) -> u64 {
        now_total_tokens.saturating_sub(self.tokens_at_start)
    }

    /// True only when a budget is set and spend since baseline exceeds it.
    #[must_use]
    pub fn over_budget(&self, now_total_tokens: u64) -> bool {
        match self.token_budget {
            Some(b) => self.tokens_used(now_total_tokens) > b,
            None => false,
        }
    }

    #[must_use]
    pub const fn with_next_wake_ms(mut self, next_wake_ms: Option<u64>) -> Self {
        self.next_wake_ms = next_wake_ms;
        self
    }

    /// Re-target the watch: swap the prompt re-injected each tick without a
    /// stop/start cycle. Empty input is ignored by the caller.
    #[must_use]
    pub fn with_prompt(mut self, prompt: &str) -> Self {
        self.prompt = prompt.to_string();
        self
    }

    /// Re-pace the loop in place: change a Fixed interval, or convert a
    /// model-paced loop to a fixed cadence (and vice-versa). Clears any stale
    /// `next_wake_ms` when switching to Fixed, where it is meaningless.
    #[must_use]
    pub fn with_cadence(mut self, cadence: Cadence) -> Self {
        if matches!(cadence, Cadence::Fixed { .. }) {
            self.next_wake_ms = None;
        }
        self.cadence = cadence;
        self
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == LoopStatus::Active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LoopState {
        LoopState::new(
            "sess-1",
            "check deploy status",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        )
    }

    #[test]
    fn new_loop_is_active_with_zero_iterations() {
        let l = sample();
        assert_eq!(l.status, LoopStatus::Active);
        assert_eq!(l.iterations_used, 0);
        assert_eq!(l.session_id, "sess-1");
        assert_eq!(l.created_at_ms, 1_000);
        assert!(l.max_iterations.is_none());
        assert!(l.deadline_ms.is_none());
        assert!(l.next_wake_ms.is_none());
    }

    #[test]
    fn spent_iteration_increments_and_returns_new_copy() {
        let l = sample();
        let after = l.clone().spent_iteration();
        assert_eq!(after.iterations_used, 1);
        assert_eq!(l.iterations_used, 0, "original unchanged");
    }

    #[test]
    fn with_status_returns_new_copy() {
        let l = sample();
        let stopped = l.clone().with_status(LoopStatus::Stopped);
        assert_eq!(stopped.status, LoopStatus::Stopped);
        assert_eq!(l.status, LoopStatus::Active, "original unchanged");
    }

    #[test]
    fn with_caps_sets_optional_bounds() {
        let l = sample()
            .with_max_iterations(Some(20))
            .with_deadline_ms(Some(99_999));
        assert_eq!(l.max_iterations, Some(20));
        assert_eq!(l.deadline_ms, Some(99_999));
    }

    #[test]
    fn with_next_wake_sets_model_paced_delay() {
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(480_000));
        assert_eq!(l.next_wake_ms, Some(480_000));
    }

    #[test]
    fn with_prompt_retargets_without_touching_other_fields() {
        let l = sample()
            .with_max_iterations(Some(9))
            .with_prompt("watch staging");
        assert_eq!(l.prompt, "watch staging");
        assert_eq!(l.max_iterations, Some(9), "other fields preserved");
    }

    #[test]
    fn with_cadence_to_fixed_clears_stale_next_wake() {
        // A model-paced loop carrying a next_wake, re-paced to Fixed: the
        // next_wake is meaningless for Fixed and must be cleared.
        let l = LoopState::new(
            "s",
            "p",
            Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        )
        .with_next_wake_ms(Some(123_456))
        .with_cadence(Cadence::Fixed {
            interval_ms: 120_000,
        });
        assert!(matches!(
            l.cadence,
            Cadence::Fixed {
                interval_ms: 120_000
            }
        ));
        assert!(l.next_wake_ms.is_none(), "stale next_wake cleared on Fixed");
    }

    #[test]
    fn serde_roundtrip_preserves_state() {
        let l = sample()
            .with_max_iterations(Some(5))
            .with_token_budget(Some(2_000))
            .with_baseline(1_000);
        let json = serde_json::to_string(&l).unwrap();
        let back: LoopState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.iterations_used, l.iterations_used);
        assert_eq!(back.max_iterations, Some(5));
        assert_eq!(back.token_budget, Some(2_000));
        assert_eq!(back.tokens_at_start, 1_000);
        assert!(back.baseline_captured);
    }

    #[test]
    fn new_loop_has_no_baseline() {
        let l = sample();
        assert_eq!(l.tokens_at_start, 0);
        assert!(!l.baseline_captured);
        assert!(l.token_budget.is_none());
    }

    #[test]
    fn with_baseline_seeds_tokens_and_marks_captured() {
        let l = sample().with_baseline(12_345);
        assert_eq!(l.tokens_at_start, 12_345);
        assert!(l.baseline_captured);
        // original unchanged (§不可变性)
        assert_eq!(sample().tokens_at_start, 0);
    }

    #[test]
    fn tokens_used_saturates_on_counter_reset() {
        let l = sample().with_baseline(1_000);
        assert_eq!(l.tokens_used(1_750), 750);
        assert_eq!(l.tokens_used(500), 0, "counter going backwards saturates");
    }

    #[test]
    fn over_budget_only_when_budget_set_and_exceeded() {
        let l = sample().with_token_budget(Some(500)).with_baseline(1_000);
        assert!(!l.over_budget(1_200), "200 spent under a 500 budget");
        assert!(l.over_budget(1_600), "600 spent over a 500 budget");
        // No budget → never over budget, even at u64::MAX.
        assert!(!sample().with_baseline(1_000).over_budget(u64::MAX));
    }

    #[test]
    fn old_payload_without_baseline_fields_deserializes_defaults() {
        // JSON persisted before these fields existed (no tokens_at_start /
        // baseline_captured keys). Loop is never persisted today, but parity
        // with goal keeps the forward-compat guarantee.
        let json = r#"{"session_id":"s","prompt":"p",
            "cadence":{"kind":"fixed","interval_ms":300000},
            "iterations_used":0,"status":"active","created_at_ms":1}"#;
        let l: LoopState = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(l.tokens_at_start, 0);
        assert!(!l.baseline_captured);
        assert!(l.token_budget.is_none());
    }
}
