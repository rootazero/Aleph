//! Standing-goal entity — a persistent user objective with lifecycle +
//! budget, distinct from the per-task `scratchpad` working memory.
//!
//! Immutable by construction (CLAUDE.md coding-style §不可变性): every
//! mutator returns a new `Goal`; the store overwrites the row.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PursuitMode {
    Passive,
    Active { max_iterations: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Goal {
    pub id: String,
    pub session_id: String,
    pub objective: String,
    pub status: GoalStatus,
    pub token_budget: Option<u64>,
    pub tokens_at_start: u64,
    pub pursuit: PursuitMode,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub note: Option<String>,
    /// Autonomous continuations already spent on this goal (the `Active`
    /// pursuit backstop counter). `#[serde(default)]` so goals persisted
    /// before this field deserialize as 0.
    #[serde(default)]
    pub continuations_used: u32,
}

impl Goal {
    pub fn new(session_id: &str, objective: &str, now_total_tokens: u64, now_ms: u64) -> Self {
        let id = format!("goal-{:x}", fxhash_str(&format!("{session_id}:{objective}")));
        Self {
            id,
            session_id: session_id.to_string(),
            objective: objective.to_string(),
            status: GoalStatus::Active,
            token_budget: None,
            tokens_at_start: now_total_tokens,
            pursuit: PursuitMode::Passive,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            note: None,
            continuations_used: 0,
        }
    }

    /// Record that one autonomous continuation was enqueued for this goal.
    /// Bumps `updated_at_ms` since this is a lifecycle event.
    pub fn spent_continuation(mut self, now_ms: u64) -> Self {
        self.continuations_used = self.continuations_used.saturating_add(1);
        self.updated_at_ms = now_ms;
        self
    }

    pub fn with_status(mut self, status: GoalStatus, now_ms: u64) -> Self {
        self.status = status;
        self.updated_at_ms = now_ms;
        self
    }

    pub fn with_note(mut self, note: Option<String>, now_ms: u64) -> Self {
        self.note = note;
        self.updated_at_ms = now_ms;
        self
    }

    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (unlike `with_status`/`with_note`).
    pub fn with_budget(mut self, token_budget: Option<u64>) -> Self {
        self.token_budget = token_budget;
        self
    }

    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (unlike `with_status`/`with_note`).
    pub fn with_pursuit(mut self, pursuit: PursuitMode) -> Self {
        self.pursuit = pursuit;
        self
    }

    pub fn tokens_used(&self, now_total_tokens: u64) -> u64 {
        now_total_tokens.saturating_sub(self.tokens_at_start)
    }

    pub fn over_budget(&self, now_total_tokens: u64) -> bool {
        match self.token_budget {
            Some(b) => self.tokens_used(now_total_tokens) > b,
            None => false,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == GoalStatus::Active
    }
}

fn fxhash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Goal {
        Goal::new("sess-1", "Migrate auth to new API", 1_000, 5_000)
    }

    #[test]
    fn new_goal_is_active_passive() {
        let g = sample();
        assert_eq!(g.status, GoalStatus::Active);
        assert!(matches!(g.pursuit, PursuitMode::Passive));
        assert_eq!(g.tokens_at_start, 1_000);
        assert_eq!(g.token_budget, None);
        assert_eq!(g.created_at_ms, 5_000);
        assert_eq!(g.updated_at_ms, 5_000);
        assert_eq!(g.continuations_used, 0);
        assert!(!g.id.is_empty());
    }

    #[test]
    fn spent_continuation_increments_and_bumps_updated_at() {
        let g = sample();
        let after = g.clone().spent_continuation(9_000);
        assert_eq!(after.continuations_used, 1);
        assert_eq!(after.updated_at_ms, 9_000);
        assert_eq!(g.continuations_used, 0, "original unchanged");
        let after2 = after.spent_continuation(9_500);
        assert_eq!(after2.continuations_used, 2);
    }

    #[test]
    fn with_status_returns_new_copy_and_bumps_updated_at() {
        let g = sample();
        let done = g.clone().with_status(GoalStatus::Complete, 2_500);
        assert_eq!(done.status, GoalStatus::Complete);
        assert_eq!(g.status, GoalStatus::Active, "original must be unchanged");
        assert_eq!(done.updated_at_ms, 2_500);
        assert_eq!(done.id, g.id, "identity is stable across updates");
    }

    #[test]
    fn tokens_used_saturates_on_counter_reset() {
        let g = sample();
        assert_eq!(g.tokens_used(1_750), 750);
        assert_eq!(g.tokens_used(500), 0, "counter going backwards saturates to 0");
    }

    #[test]
    fn over_budget_only_when_budget_set_and_exceeded() {
        let g = sample().with_budget(Some(500));
        assert!(!g.over_budget(1_200));
        assert!(g.over_budget(1_600));
        let no_budget = sample();
        assert!(!no_budget.over_budget(u64::MAX));
    }

    #[test]
    fn active_pursuit_carries_iteration_cap() {
        let g = sample().with_pursuit(PursuitMode::Active { max_iterations: 8 });
        match g.pursuit {
            PursuitMode::Active { max_iterations } => assert_eq!(max_iterations, 8),
            _ => panic!("expected Active pursuit"),
        }
    }
}
