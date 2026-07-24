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

impl GoalStatus {
    /// Canonical snake_case name — identical to the serde wire form and to the
    /// `status='…'` value the model passes to `goal(action='update', …)`. The
    /// tool's user-facing render uses this instead of a raw `{:?}` Debug dump so
    /// what the model reads back (`status=active`) matches what it must type,
    /// mirroring `looping::Cadence::describe` / `LoopState::human_summary`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PursuitMode {
    Passive,
    Active { max_iterations: u32 },
}

/// Maker/checker 分离的类型态：模型调用 `goal(complete)` 是一个 *claim*；
/// 客观闸门（config.toml `[[stop_hooks]]` 退出码）通过才是 *confirmation*。
/// 只有自主续跑（`PursuitMode::Active`）的 goal 会被闸门守护；交互/被动
/// goal 的 complete 立即终止，不经闸门。`#[serde(default)]` → 旧持久化
/// 反序列化为 `Unchecked`。
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateOutcome {
    /// 尚无闸门确认完成（默认；也是 Active/Paused/Blocked 的静息态）。
    #[default]
    Unchecked,
    /// 客观闸门确认了模型的 `Complete` 主张 → 循环真正终止。
    Passed,
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
    /// 模型自报的 `Complete` 是否已被客观闸门确认（见 [`GateOutcome`]）。
    /// `#[serde(default)]` → 旧持久化读为 `Unchecked`。
    #[serde(default)]
    pub gate_outcome: GateOutcome,
    /// Optional per-goal objective gate: a shell command evaluated like a
    /// `config.toml [[stop_hooks]]` entry (exit 0 = passed, exit 2 = vetoed,
    /// stdout = reason). Supplements the global gate (logical AND) — see
    /// the continuation hook. `#[serde(default)]` → old payloads read `None`.
    #[serde(default)]
    pub gate_command: Option<String>,
    /// Accumulated lessons (the article's "state file"): gate-failure reasons
    /// and model-authored insights, fed back into the continuation prompt so
    /// the loop does not repeat mistakes. Ring-capped at `MAX_LESSONS` (newest
    /// kept). `#[serde(default)]` → old payloads read empty.
    #[serde(default)]
    pub lessons: Vec<String>,
    /// Optional wall-clock deadline (Unix epoch ms). When set and exceeded, the
    /// autonomous loop stops re-pursuing and blocks the goal for the user — a
    /// structural stop condition alongside the iteration/token caps (R7: no
    /// judgment, pure time comparison). `#[serde(default)]` → old payloads read
    /// `None`.
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    /// Whether `tokens_at_start` has been seeded with a real session-token
    /// baseline. The `goal` tool seeds `tokens_at_start = 0` at set time (it has
    /// no live token counter), so the autonomous driver captures the true
    /// baseline lazily on the first continuation hook that sees a budget set —
    /// codex's `tokenStartFresh` pattern. Until captured, the token budget is
    /// not enforced (only iteration/deadline caps apply). `#[serde(default)]` →
    /// old payloads read `false`.
    #[serde(default)]
    pub baseline_captured: bool,
    /// Wall-clock (Unix epoch ms) at which the currently-claimed autonomous
    /// continuation is due to fire, or `None` when none is in flight. This is
    /// the fan-out gate + accounting anchor of the pursuit pipeline, mirroring
    /// `looping::LoopState::pending_tick_wake_ms`:
    /// [`crate::goal::GoalStore::try_claim_continuation`] refuses to claim a
    /// second continuation while one is pending, and
    /// [`crate::goal::GoalStore::rearm_after_busy`] re-stamps it (without
    /// spending another iteration) when the claimed run lost the session slot.
    /// Owned exclusively by the claim pipeline — never hand-written by the tool
    /// (see `GoalStore::commit_field_update`). `#[serde(default)]` → old
    /// payloads read `None`.
    #[serde(default)]
    pub pending_continuation_ms: Option<u64>,
    /// Wait barrier (hermes `wait_for_seconds` parity): park autonomous
    /// pursuit until this wall-clock instant (Unix epoch ms). The MODEL parks
    /// itself in-turn via `goal(update, wait_minutes=…)` (R7 — the decision
    /// to wait is the model's; the store only compares clocks). While set and
    /// in the future, the claim pipeline arms an exact timer wake instead of
    /// the next step. Mutually exclusive with [`Self::waiting_on_task`].
    /// `#[serde(default)]` → old payloads read `None`.
    #[serde(default)]
    pub waiting_until_ms: Option<u64>,
    /// Wait barrier (hermes `waiting_on_session` analog): park autonomous
    /// pursuit until this coordination task reaches a settled state. Wake is
    /// event-driven (`GoalWakeService` on the GlobalBus task-settle events)
    /// plus a boot recheck; fail-open — an unknown/vanished task reads as
    /// settled so a stale barrier can never wedge the pursuit forever.
    /// `#[serde(default)]` → old payloads read `None`.
    #[serde(default)]
    pub waiting_on_task: Option<String>,
    /// Why the pursuit parked (model-supplied, surfaced in renders).
    #[serde(default)]
    pub waiting_reason: Option<String>,
    /// Delegation sessions whose token spend counts against this goal's
    /// `token_budget` (codex `RolloutBudget` mapped onto A3's single
    /// persistent source: the tree total is the goal session's own live
    /// total plus each member's delta since it joined — summed from
    /// `SessionStore::get_total_tokens`, no in-memory shared counter).
    /// Registered by the `session_send` delegation seam when the calling
    /// session carries an active budgeted goal. Ring-capped at
    /// [`MAX_BUDGET_MEMBERS`]. `#[serde(default)]` → old payloads read empty.
    #[serde(default)]
    pub budget_members: Vec<BudgetMember>,
    /// The instant this goal last transitioned INTO `Complete` (Unix epoch
    /// ms); cleared on any transition out. The settle-notify CAS keys on this
    /// instant so post-completion field edits (lesson/note appends bump
    /// `updated_at_ms`) cannot mint a fresh watcher claim — only a genuine
    /// re-completion can. `#[serde(default)]` → old payloads read `None`
    /// (the stamp falls back to `updated_at_ms`).
    #[serde(default)]
    pub completed_at_ms: Option<u64>,
}

/// One delegation session enrolled in a goal's shared token budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BudgetMember {
    /// The member's session key string (gateway form).
    pub session_id: String,
    /// The member session's cumulative token total at enrollment — only
    /// spend AFTER joining counts against the shared budget (the per-member
    /// twin of `tokens_at_start`).
    pub tokens_at_join: u64,
}

/// Cap on enrolled budget members, bounding the goal row and the per-claim
/// token-summing fan-out. Oldest kept (first writer wins) — a pursuit
/// delegating to more than this many DISTINCT sessions has bigger problems
/// than accounting.
pub const MAX_BUDGET_MEMBERS: usize = 32;

/// Ring cap on accumulated lessons kept per goal (newest retained). Bounds the
/// state file so an unbounded loop cannot grow the goal row without limit.
pub const MAX_LESSONS: usize = 5;

impl Goal {
    #[must_use]
    pub fn new(session_id: &str, objective: &str, now_total_tokens: u64, now_ms: u64) -> Self {
        let id = format!(
            "goal-{:x}",
            fxhash_str(&format!("{session_id}:{objective}"))
        );
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
            gate_outcome: GateOutcome::Unchecked,
            gate_command: None,
            lessons: Vec::new(),
            deadline_ms: None,
            baseline_captured: false,
            pending_continuation_ms: None,
            waiting_until_ms: None,
            waiting_on_task: None,
            waiting_reason: None,
            budget_members: Vec::new(),
            completed_at_ms: None,
        }
    }

    /// Park pursuit until `until_ms` (clears any task barrier — the two wait
    /// kinds are mutually exclusive, hermes parity).
    #[must_use]
    pub fn with_wait_until(mut self, until_ms: u64, reason: Option<String>, now_ms: u64) -> Self {
        self.waiting_until_ms = Some(until_ms);
        self.waiting_on_task = None;
        self.waiting_reason = reason;
        self.updated_at_ms = now_ms;
        self
    }

    /// Park pursuit until the given coordination task settles (clears any
    /// deadline barrier).
    #[must_use]
    pub fn with_wait_on_task(
        mut self,
        task_id: String,
        reason: Option<String>,
        now_ms: u64,
    ) -> Self {
        self.waiting_on_task = Some(task_id);
        self.waiting_until_ms = None;
        self.waiting_reason = reason;
        self.updated_at_ms = now_ms;
        self
    }

    /// Drop any wait barrier (explicit un-park, barrier satisfaction, or a
    /// lifecycle transition that makes waiting meaningless).
    #[must_use]
    pub fn without_wait(mut self, now_ms: u64) -> Self {
        self.waiting_until_ms = None;
        self.waiting_on_task = None;
        self.waiting_reason = None;
        self.updated_at_ms = now_ms;
        self
    }

    /// Whether any wait barrier is configured (regardless of satisfaction).
    #[must_use]
    pub const fn has_wait_barrier(&self) -> bool {
        self.waiting_until_ms.is_some() || self.waiting_on_task.is_some()
    }

    /// Stamp (or clear) the in-flight continuation marker. Scheduling state, not
    /// a lifecycle transition — deliberately does not bump `updated_at_ms`
    /// (mirrors `looping::LoopState::with_pending_tick`).
    #[must_use]
    pub const fn with_pending_continuation(mut self, wake_ms: Option<u64>) -> Self {
        self.pending_continuation_ms = wake_ms;
        self
    }

    /// Record that one autonomous continuation was enqueued for this goal.
    /// Bumps `updated_at_ms` since this is a lifecycle event.
    #[must_use]
    pub const fn spent_continuation(mut self, now_ms: u64) -> Self {
        self.continuations_used = self.continuations_used.saturating_add(1);
        self.updated_at_ms = now_ms;
        self
    }

    #[must_use]
    pub const fn with_status(mut self, status: GoalStatus, now_ms: u64) -> Self {
        // Stamp the completion instant only on the transition INTO `Complete`;
        // clear it when leaving so a reopened-then-re-completed goal mints a
        // fresh settle-notify stamp (fires watchers again, by design).
        match (
            matches!(self.status, GoalStatus::Complete),
            matches!(status, GoalStatus::Complete),
        ) {
            (false, true) => self.completed_at_ms = Some(now_ms),
            (true, false) => self.completed_at_ms = None,
            _ => {}
        }
        self.status = status;
        self.updated_at_ms = now_ms;
        self
    }

    #[must_use]
    pub fn with_note(mut self, note: Option<String>, now_ms: u64) -> Self {
        self.note = note;
        self.updated_at_ms = now_ms;
        self
    }

    /// Lifecycle transition（闸门确认/复位）——bump `updated_at_ms`，
    /// 与 `with_status`/`with_note` 同型。返回新 `Goal`（§不可变性）。
    #[must_use]
    pub const fn with_gate_outcome(mut self, outcome: GateOutcome, now_ms: u64) -> Self {
        self.gate_outcome = outcome;
        self.updated_at_ms = now_ms;
        self
    }

    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (mirrors `with_budget`/`with_pursuit`).
    #[must_use]
    pub fn with_gate_command(mut self, gate_command: Option<String>) -> Self {
        self.gate_command = gate_command;
        self
    }

    /// Append a lesson to the state file, keeping at most `MAX_LESSONS` (newest).
    /// Appending a lesson is progress, so it bumps `updated_at_ms` (like
    /// `with_note`). Returns a new `Goal` (§不可变性).
    #[must_use]
    pub fn with_lesson_appended(mut self, lesson: String, now_ms: u64) -> Self {
        self.lessons.push(lesson);
        if self.lessons.len() > MAX_LESSONS {
            let drop = self.lessons.len() - MAX_LESSONS;
            self.lessons.drain(0..drop);
        }
        self.updated_at_ms = now_ms;
        self
    }

    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (unlike `with_status`/`with_note`).
    #[must_use]
    pub const fn with_budget(mut self, token_budget: Option<u64>) -> Self {
        self.token_budget = token_budget;
        self
    }

    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (unlike `with_status`/`with_note`).
    #[must_use]
    pub const fn with_pursuit(mut self, pursuit: PursuitMode) -> Self {
        self.pursuit = pursuit;
        self
    }

    /// Configuration, not a lifecycle transition — deliberately does not bump
    /// `updated_at_ms` (mirrors `with_budget`/`with_pursuit`). `None` clears.
    #[must_use]
    pub const fn with_deadline_ms(mut self, deadline_ms: Option<u64>) -> Self {
        self.deadline_ms = deadline_ms;
        self
    }

    /// Seed the real token baseline (the session's cumulative total at the
    /// moment autonomous pursuit begins consuming budget) and mark it captured.
    /// Capturing the baseline is a lifecycle event, so it bumps `updated_at_ms`
    /// (like `with_status`). Returns a new `Goal` (§不可变性).
    #[must_use]
    pub const fn with_baseline(mut self, tokens_at_start: u64, now_ms: u64) -> Self {
        self.tokens_at_start = tokens_at_start;
        self.baseline_captured = true;
        self.updated_at_ms = now_ms;
        self
    }

    #[must_use]
    pub const fn tokens_used(&self, now_total_tokens: u64) -> u64 {
        now_total_tokens.saturating_sub(self.tokens_at_start)
    }

    /// Over the token budget — enforceable ONLY once a real baseline is captured.
    ///
    /// Without `baseline_captured`, `tokens_at_start` is the tool's placeholder 0,
    /// so `tokens_used` would return the session's ENTIRE lifetime token count and
    /// a fresh goal in a long-running session would read as instantly over budget.
    /// The continuation hook used to hold this invariant by hand (passing
    /// `tokens_now = 0` until it had seeded the baseline); making it a property of
    /// the type instead means no future caller can get it wrong.
    #[must_use]
    pub fn over_budget(&self, now_total_tokens: u64) -> bool {
        match self.token_budget {
            Some(b) if self.baseline_captured => self.tokens_used(now_total_tokens) > b,
            _ => false,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == GoalStatus::Active
    }
}

fn fxhash_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
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
    fn status_as_str_matches_serde_wire_form() {
        // The render vocabulary the model reads must equal the `status='…'`
        // value it types — assert as_str() never drifts from the serde form.
        for st in [
            GoalStatus::Active,
            GoalStatus::Paused,
            GoalStatus::Blocked,
            GoalStatus::Complete,
        ] {
            let wire = serde_json::to_string(&st).unwrap();
            assert_eq!(format!("\"{}\"", st.as_str()), wire);
        }
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
        assert_eq!(
            g.tokens_used(500),
            0,
            "counter going backwards saturates to 0"
        );
    }

    #[test]
    fn over_budget_only_when_budget_set_and_exceeded() {
        // Baseline captured at the goal's own start token count (1_000).
        let g = sample().with_budget(Some(500)).with_baseline(1_000, 0);
        assert!(!g.over_budget(1_200));
        assert!(g.over_budget(1_600));
        let no_budget = sample();
        assert!(!no_budget.over_budget(u64::MAX));
    }

    #[test]
    fn over_budget_is_unenforceable_until_the_baseline_is_captured() {
        // The tool seeds tokens_at_start = 0 (it has no live counter), so without
        // this guard a brand-new goal in a long session reads as instantly over
        // budget and is blocked before it takes a single step.
        let fresh = sample().with_budget(Some(500));
        assert!(!fresh.baseline_captured);
        assert!(!fresh.over_budget(u64::MAX));
    }

    #[test]
    fn active_pursuit_carries_iteration_cap() {
        let g = sample().with_pursuit(PursuitMode::Active { max_iterations: 8 });
        match g.pursuit {
            PursuitMode::Active { max_iterations } => assert_eq!(max_iterations, 8),
            _ => panic!("expected Active pursuit"),
        }
    }

    #[test]
    fn new_goal_gate_outcome_is_unchecked() {
        assert_eq!(sample().gate_outcome, GateOutcome::Unchecked);
    }

    #[test]
    fn with_gate_outcome_returns_new_goal_and_bumps_updated_at() {
        let g = sample();
        let after = g.clone().with_gate_outcome(GateOutcome::Passed, 9_000);
        assert_eq!(after.gate_outcome, GateOutcome::Passed);
        assert_eq!(after.updated_at_ms, 9_000);
        assert_eq!(g.gate_outcome, GateOutcome::Unchecked, "original unchanged");
        // 其它字段不受影响
        assert_eq!(after.status, g.status);
        assert_eq!(after.objective, g.objective);
    }

    #[test]
    fn old_payload_without_gate_outcome_deserializes_unchecked() {
        // 模拟本字段引入前持久化的 JSON（无 gate_outcome 键）。
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(g.gate_outcome, GateOutcome::Unchecked);
    }

    #[test]
    fn new_goal_has_no_gate_command_and_no_lessons() {
        let g = sample();
        assert_eq!(g.gate_command, None);
        assert!(g.lessons.is_empty());
    }

    #[test]
    fn with_gate_command_sets_without_bumping_updated_at() {
        let g = sample();
        let after = g.clone().with_gate_command(Some("cargo test".into()));
        assert_eq!(after.gate_command.as_deref(), Some("cargo test"));
        assert_eq!(after.updated_at_ms, g.updated_at_ms, "config, no bump");
        assert_eq!(g.gate_command, None, "original unchanged");
    }

    #[test]
    fn with_lesson_appended_keeps_last_five_and_bumps_updated_at() {
        let mut g = sample();
        for i in 0..7 {
            g = g.with_lesson_appended(format!("lesson {i}"), 1_000 + i as u64);
        }
        assert_eq!(g.lessons.len(), MAX_LESSONS);
        assert_eq!(g.lessons.first().unwrap(), "lesson 2", "oldest dropped");
        assert_eq!(g.lessons.last().unwrap(), "lesson 6", "newest kept");
        assert_eq!(g.updated_at_ms, 1_006);
    }

    #[test]
    fn old_payload_without_new_fields_deserializes_defaults() {
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0,"gate_outcome":"unchecked"}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(g.gate_command, None);
        assert!(g.lessons.is_empty());
    }

    #[test]
    fn new_goal_has_no_deadline() {
        assert_eq!(sample().deadline_ms, None);
    }

    #[test]
    fn with_deadline_ms_sets_without_bumping_updated_at() {
        let g = sample();
        let after = g.clone().with_deadline_ms(Some(99_999));
        assert_eq!(after.deadline_ms, Some(99_999));
        assert_eq!(after.updated_at_ms, g.updated_at_ms, "config, no bump");
        assert_eq!(g.deadline_ms, None, "original unchanged");
    }

    #[test]
    fn old_payload_without_deadline_deserializes_none() {
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0,"gate_outcome":"unchecked"}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert_eq!(g.deadline_ms, None);
    }

    #[test]
    fn new_goal_baseline_not_captured() {
        assert!(!sample().baseline_captured);
    }

    #[test]
    fn with_baseline_seeds_tokens_and_marks_captured_and_bumps_updated_at() {
        let g = sample();
        let after = g.clone().with_baseline(12_345, 9_000);
        assert_eq!(after.tokens_at_start, 12_345);
        assert!(after.baseline_captured);
        assert_eq!(after.updated_at_ms, 9_000);
        // original unchanged (§不可变性)
        assert_eq!(g.tokens_at_start, 1_000);
        assert!(!g.baseline_captured);
        // budget now measures from the seeded baseline, not session lifetime
        let budgeted = after.with_budget(Some(100));
        assert_eq!(budgeted.tokens_used(12_345), 0, "no spend at baseline");
        assert!(budgeted.over_budget(12_500), "250 over a 100 budget");
    }

    #[test]
    fn old_payload_without_baseline_captured_deserializes_false() {
        let json = r#"{"id":"goal-1","session_id":"s","objective":"o",
            "status":"active","token_budget":null,"tokens_at_start":0,
            "pursuit":{"mode":"passive"},"created_at_ms":1,"updated_at_ms":1,
            "note":null,"continuations_used":0,"gate_outcome":"unchecked"}"#;
        let g: Goal = serde_json::from_str(json).expect("deserialize old payload");
        assert!(!g.baseline_captured);
    }
}
