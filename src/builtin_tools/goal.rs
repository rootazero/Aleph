//! Goal Tool — manage the session's standing goal (R8: everything-is-a-tool).
//!
//! A standing goal is a persistent user objective the assistant keeps
//! pursuing across turns. The model creates one ONLY when the user asks,
//! marks it `complete`/`blocked` when self-reporting, and the system
//! re-surfaces it every turn via `StandingGoalLayer`. Completion is the
//! model's explicit call here — there is no judge LLM (R7/R10).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::error::{AlephError, Result};
use crate::goal::{Goal, GoalStatus, GoalStore, PursuitMode};
use crate::memory::notes::NoteIndexer;
use crate::memory::store::SqliteMemoryBackend;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalAction {
    /// Create or replace the standing goal. Use ONLY when the user explicitly
    /// asks you to pursue a standing objective.
    Set,
    /// Read the current standing goal: objective, status, budget.
    Get,
    /// Update status (`complete`/`blocked` to self-report; `paused`/`active`
    /// only when the user asks) and/or attach a note.
    Update,
    /// Clear the standing goal entirely.
    Clear,
    /// List every standing goal across ALL sessions (not just this one), so the
    /// user can ask "what goals am I pursuing?" from any channel. One compact
    /// line per goal; the current session's goal is flagged.
    List,
    /// Kill switch: pause EVERY actively-pursued goal in every session at once.
    /// For "stop all the autonomous work" / incident response. Objectives and
    /// lessons are kept — each session resumes its own with
    /// `update(status='active')`. Operator only.
    PauseAll,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GoalArgs {
    pub action: GoalAction,
    /// Objective text — required for `set`.
    pub objective: Option<String>,
    /// New status — for `update`.
    pub status: Option<GoalStatus>,
    /// Optional status note — for `update`/`set`.
    pub note: Option<String>,
    /// Optional soft token budget — for `set`; on `update` adjusts it in place.
    /// Raise it (with `status='active'`) to resume a goal blocked at its budget.
    pub token_budget: Option<u64>,
    /// If present on `set`, enables autonomous continuation (opt-in,
    /// default-off) bounded by this many Think→Act iterations. On `update` it
    /// resets the TOTAL iteration cap in place (a total, not an increment) —
    /// raise it above the used count (with `status='active'`) to resume a goal
    /// blocked at its iteration cap, without losing accumulated lessons.
    pub pursuit_max_iterations: Option<u32>,
    /// Optional per-goal objective gate shell command — for `set`; on `update`
    /// replaces it (pass an empty string to clear). Evaluated like a stop hook
    /// (exit 0 = pass, exit 2 = veto). Supplements the global gate. Use a real
    /// pass/fail command (tests/build/lint), not prose.
    pub gate_command: Option<String>,
    /// Optional lesson to append to the goal's state file — for `update`.
    /// Record a durable, transferable insight (a missed step, a constraint, an
    /// approach that worked) so future autonomous iterations don't repeat it.
    /// Do NOT record environment-specific failures, transient errors that are
    /// now resolved, or negative claims that a tool / the codebase is "broken" —
    /// those harden into self-imposed refusals the agent cites against itself
    /// for the rest of the pursuit.
    pub lesson: Option<String>,
    /// Wall-clock budget in minutes. Converted to an absolute deadline
    /// (now + minutes). On `set` it bounds the pursuit; on `update` it sets a
    /// FRESH deadline from now — use it (with `status='active'`) to resume a
    /// goal blocked at its deadline. None = leave unchanged (no time limit).
    pub timeout_minutes: Option<u32>,
    /// Park the autonomous pursuit for this many minutes — for `update` on an
    /// active autonomous goal, when the next step is blocked on slow external
    /// work (a rate-limit cooldown, a long build). The pursuit wakes with an
    /// exact timer instead of burning iterations polling. Pass a `note`
    /// explaining why. Mutually exclusive with `wait_for_task`.
    pub wait_minutes: Option<u32>,
    /// Park the autonomous pursuit until this coordination task id settles
    /// (completes / fails / is cancelled or skipped) — for `update`, when the
    /// goal's next step depends on a team/workflow task. Wakes on the task's
    /// settle event. Mutually exclusive with `wait_minutes`.
    pub wait_for_task: Option<String>,
    /// Target ANOTHER session's standing goal — the session key exactly as
    /// `action='list'` prints it. Omit for this session (the normal case).
    ///
    /// Honored by `get`, `clear`, and by `update` restricted to
    /// `status='paused'` (optionally with a `note`). Everything that ARMS a
    /// pursuit — `set`, `status='active'`, budget/cap/deadline raises, wait
    /// barriers — refuses it: an autonomous goal is re-driven by its own
    /// session's completion hook, so one armed from elsewhere would sit
    /// `Active` with nothing to claim its next continuation while `get` and
    /// `list` both report it as running. Requires operator authorization.
    pub session: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoalOutput {
    pub success: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct GoalTool {
    store: Arc<GoalStore>,
    session_key: Option<Arc<RwLock<String>>>,
    /// Tool-free planner provider; `None` → no Strategy is minted on `set`
    /// (byte-identical to today). Injected at the construction site.
    planner_provider: Option<Arc<dyn AiProvider>>,
    /// Note indexer used to graduate a goal's lessons on `clear`. `None` → the
    /// promotion is skipped (memory not configured, unit tests): a missing
    /// handle must never fail the user's `clear`.
    lesson_indexer: Option<Arc<NoteIndexer<SqliteMemoryBackend>>>,
}

impl GoalTool {
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self {
            store,
            session_key: None,
            planner_provider: None,
            lesson_indexer: None,
        }
    }

    #[must_use]
    pub fn with_planner_provider(mut self, provider: Option<Arc<dyn AiProvider>>) -> Self {
        self.planner_provider = provider;
        self
    }

    #[must_use]
    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    /// Attach the note indexer used to graduate lessons before a goal row dies.
    ///
    /// `Goal.lessons` is a ring buffer that lives only on the goal row, and
    /// `GoalLessonsPromoteStage` graduates it into `goal-lessons/<id>` at night.
    /// Both `clear` (DELETE) and a re-objectiving `set` (overwrite) destroy the
    /// row hours before that, so without this handle everything the pursuit
    /// learned since the last dream window dies with it. `None` keeps the old
    /// behaviour (destroy only) rather than failing the user's command.
    #[must_use]
    pub fn with_lesson_indexer(
        mut self,
        indexer: Option<Arc<NoteIndexer<SqliteMemoryBackend>>>,
    ) -> Self {
        self.lesson_indexer = indexer;
        self
    }

    /// Land `goal`'s lessons in its per-goal note before the row goes away
    /// (`clear`'s DELETE, or a `set` that replaces the objective). Returns the
    /// number of facts appended (0 when there is no indexer, no lesson, or the
    /// note already holds them).
    ///
    /// Awaited rather than spawned on purpose: this is the LAST moment these
    /// lessons exist anywhere, so a fire-and-forget task that loses the race
    /// with process exit would lose them for good. The write is a local file +
    /// SQLite index.
    ///
    /// The agent id comes from the TARGET session key (a cross-session `clear`
    /// may act on another agent's goal), falling back to the ambient turn's
    /// agent and finally to the default agent.
    async fn promote_lessons_before_loss(&self, session: &str, goal: &Goal) -> u32 {
        let Some(indexer) = &self.lesson_indexer else {
            return 0;
        };
        let agent_id = crate::routing::session_key::SessionKey::parse(session)
            .map(|k| k.agent_id().to_string())
            .or_else(crate::tools::turn_context::current_agent_id)
            .unwrap_or_else(|| crate::routing::DEFAULT_AGENT_ID.to_string());
        crate::memory::dreaming::stages::goal_lessons_promote::promote_one(indexer, &agent_id, goal)
            .await
    }

    async fn session(&self) -> String {
        // Per-run truth first: the shared registry handle is process-global
        // and rewritten at every run start, so a concurrent run of another
        // agent can overwrite it mid-turn. The task-local is scoped per tool
        // call by the dispatch chokepoint and cannot race.
        if let Some(sk) = crate::tools::turn_context::current_session_key() {
            return sk;
        }
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    /// Is this turn allowed to reach across session boundaries? Goals are
    /// per-session, so before cross-session targeting the blast radius of the
    /// `goal` tool was the caller's own conversation — which is why `goal` is
    /// deliberately NOT in `method_authz::OPERATOR_TOOLS`. `session=` and
    /// `pause_all` widen it across the trust boundary and carry their own gate,
    /// mirroring `LoopTool::caller_is_operator`. Absent role = trusted
    /// local/internal run.
    fn caller_is_operator() -> bool {
        crate::tools::turn_context::current_turn_context()
            .is_none_or(|ctx| ctx.caller_is_operator())
    }

    /// Resolve which session a verb acts on. `None` → this session. `Some(key)`
    /// → that session, once the operator gate passes and the key names a goal
    /// that exists (a typo must not silently act on nothing, or read back as
    /// "no goal is set"). Loop sibling: `LoopTool::resolve_target`.
    fn resolve_target(&self, session: &str, requested: Option<&str>, verb: &str) -> Result<String> {
        let Some(target) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(session.to_string());
        };
        if target == session {
            return Ok(session.to_string());
        }
        if !Self::caller_is_operator() {
            return Err(AlephError::tool(format!(
                "{verb} on another session requires operator authorization; this \
                 conversation may only manage its own goal"
            )));
        }
        if self.store.get(target)?.is_none() {
            return Err(AlephError::tool(format!(
                "no standing goal is set for session '{target}' — call \
                 goal(action='list') and pass a session key exactly as it prints"
            )));
        }
        Ok(target.to_string())
    }

    /// " in session 'x'" when the verb reached across sessions, "" when it acted
    /// here, so a remote effect can never read as a local one.
    fn scope_suffix(session: &str, target: &str) -> String {
        if target == session {
            String::new()
        } else {
            format!(" in session '{target}'")
        }
    }

    /// Reject `session=` on a verb that ARMS a pursuit. See [`GoalArgs::session`]
    /// for why remote arming is refused rather than silently producing a goal
    /// nothing will ever drive. Loop sibling: `LoopTool::reject_remote`.
    fn reject_remote(session: &str, args: &GoalArgs, verb: &str) -> Result<()> {
        match args.session.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() && t != session => Err(AlephError::tool(format!(
                "{verb} only works on the current session: an autonomous goal is \
                 re-driven by its own session's completion hook, so one armed from \
                 elsewhere would never continue. Run {verb} from that session, or \
                 use goal(action='update', status='paused', session='…') / \
                 goal(action='clear', session='…') to quiet it from here"
            ))),
            _ => Ok(()),
        }
    }

    /// The one cross-session write on `update`: hold another session's pursuit.
    /// Every other `update` field is refused rather than silently ignored — a
    /// caller that passed `pursuit_max_iterations` alongside the pause would
    /// otherwise be told "Paused" while its cap change vanished.
    ///
    /// Deliberately does NOT delete the goal-welded Strategy: the plan is still
    /// the plan when the owner resumes, and `goal_tier_live` already stops a
    /// `Paused` goal's weld from steering that session's ordinary turns.
    fn remote_pause(
        &self,
        session: &str,
        target: &str,
        args: &GoalArgs,
        now: u64,
    ) -> Result<GoalOutput> {
        let target = self.resolve_target(session, Some(target), "update")?;
        if args.status != Some(GoalStatus::Paused) {
            return Err(AlephError::tool(
                "a cross-session update may only pause: pass status='paused' \
                 (optionally with a note). Resuming, re-budgeting, re-capping and \
                 parking must run in the goal's own session, whose completion hook \
                 is the only thing that can drive the pursuit"
                    .to_string(),
            ));
        }
        let extras = [
            args.objective.is_some().then_some("objective"),
            args.token_budget.is_some().then_some("token_budget"),
            args.pursuit_max_iterations
                .is_some()
                .then_some("pursuit_max_iterations"),
            args.gate_command.is_some().then_some("gate_command"),
            args.lesson.is_some().then_some("lesson"),
            args.timeout_minutes.is_some().then_some("timeout_minutes"),
            args.wait_minutes.is_some().then_some("wait_minutes"),
            args.wait_for_task.is_some().then_some("wait_for_task"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        if !extras.is_empty() {
            return Err(AlephError::tool(format!(
                "a cross-session update may only pause; drop {} and run those \
                 changes from the goal's own session",
                extras.join(", ")
            )));
        }
        let note = args
            .note
            .clone()
            .unwrap_or_else(|| "Paused by user request from another session.".to_string());
        if self.store.pause_if_active(&target, &note, now)? {
            return Ok(GoalOutput {
                success: true,
                message: format!(
                    "Standing goal paused in session '{target}'. That session \
                     resumes it with goal(action='update', status='active')."
                ),
            });
        }
        // Not active: paused already, or terminal (blocked/complete). Report the
        // truth instead of a bare failure so the model can relay why.
        let current = self
            .store
            .get(&target)?
            .map_or("gone".to_string(), |g| g.status.as_str().to_string());
        Ok(GoalOutput {
            success: false,
            message: format!(
                "Standing goal in session '{target}' is not being actively pursued \
                 (status={current}) — nothing to pause."
            ),
        })
    }

    fn render(goal: &Goal, now_ms: u64) -> String {
        // status as snake_case (the serde wire form the model types in `update`),
        // not a raw `{:?}` Debug dump — see `GoalStatus::as_str`.
        let mut s = format!(
            "Standing goal: {}\nstatus={}",
            goal.objective,
            goal.status.as_str()
        );
        if let Some(b) = goal.token_budget {
            s.push_str(&format!(", token_budget={b}"));
            if !goal.budget_members.is_empty() {
                s.push_str(&format!(
                    " (shared with {} delegated session(s))",
                    goal.budget_members.len()
                ));
            }
        }
        if let PursuitMode::Active { max_iterations } = goal.pursuit {
            s.push_str(&format!(
                ", pursuit=active({}/{max_iterations} iterations used)",
                goal.continuations_used
            ));
        }
        if let Some(deadline) = goal.deadline_ms {
            // Surface the *remaining* wall-clock budget so an autonomous goal
            // pacing against its deadline can see it, instead of an info-free
            // "deadline set" (parity with the loop tool's `human_summary`).
            // Minutes match the input unit (`timeout_minutes`). Falls back when
            // the clock is unavailable (now_ms == 0) or the deadline has passed.
            if now_ms != 0 && deadline > now_ms {
                let mins_left = (deadline - now_ms).div_ceil(60_000);
                s.push_str(&format!(", deadline in ~{mins_left}m"));
            } else {
                s.push_str(", deadline set");
            }
        }
        if let Some(note) = goal.note.as_deref() {
            if !note.is_empty() {
                s.push_str(&format!("\nnote: {note}"));
            }
        }
        // Parked state: name the barrier and (for a timer) the remaining wait,
        // so `get` never shows a silently-idle pursuit with no explanation.
        if let Some(task_id) = goal.waiting_on_task.as_deref() {
            s.push_str(&format!("\nwaiting: parked until task '{task_id}' settles"));
        } else if let Some(until) = goal.waiting_until_ms {
            if now_ms != 0 && until > now_ms {
                s.push_str(&format!(
                    "\nwaiting: parked for ~{}m more",
                    (until - now_ms).div_ceil(60_000)
                ));
            }
        }
        if let Some(reason) = goal.waiting_reason.as_deref() {
            if goal.has_wait_barrier() && !reason.is_empty() {
                s.push_str(&format!(" — {reason}"));
            }
        }
        if goal.gate_command.is_some() {
            // The gate is only evaluated for autonomous (Active pursuit) goals —
            // a Passive/interactive goal's `complete` terminates immediately and
            // the gate never runs. Say so instead of advertising it as live.
            if matches!(goal.pursuit, PursuitMode::Active { .. }) {
                s.push_str("\ngate: per-goal command set");
            } else {
                s.push_str(
                    "\ngate: per-goal command set (inactive — gate only runs for autonomous goals)",
                );
            }
        }
        if !goal.lessons.is_empty() {
            s.push_str(&format!(
                "\nlessons ({}): {}",
                goal.lessons.len(),
                goal.lessons.last().map(String::as_str).unwrap_or_default()
            ));
        }
        s
    }

    /// One compact line per goal for the cross-session `list` action. Pure:
    /// takes the goal plus the caller's own session (to flag "this session")
    /// and the current wall-clock so an autonomous goal's pace/deadline shows.
    /// Uses `status.as_str()` so the vocabulary matches what the model types in
    /// `update`, mirroring `render`.
    ///
    /// The session key leads every line for goals that are NOT the current
    /// one: it is the handle `clear` / `update(status='paused')` take in
    /// `session=`. Listing a goal the caller then has no way to name is exactly
    /// how "visible but unstoppable" happened.
    fn render_list_line(goal: &Goal, current_session: &str, now_ms: u64) -> String {
        let here = if goal.session_id == current_session {
            " (this session)".to_string()
        } else {
            format!(" (session '{}')", goal.session_id)
        };
        let mut s = format!("- [{}] {}{here}", goal.status.as_str(), goal.objective);
        if let PursuitMode::Active { max_iterations } = goal.pursuit {
            s.push_str(&format!(
                " | autonomous {}/{max_iterations}",
                goal.continuations_used
            ));
        }
        if goal.has_wait_barrier() {
            s.push_str(" | parked (waiting)");
        }
        if let Some(b) = goal.token_budget {
            s.push_str(&format!(" | budget={b}"));
        }
        if let Some(d) = goal.deadline_ms {
            if now_ms != 0 && d > now_ms {
                s.push_str(&format!(
                    " | deadline in ~{}m",
                    (d - now_ms).div_ceil(60_000)
                ));
            } else {
                s.push_str(" | deadline set");
            }
        }
        s
    }

    /// Fire the tool-free planner ONCE for this session's goal, fail-soft.
    /// No-op when no provider is injected, no global StrategyStore exists, a
    /// Strategy already exists for the key, or the planner self-gates/errs.
    async fn maybe_plan_strategy(&self, session: &str, goal: &Goal) {
        let Some(provider) = &self.planner_provider else {
            return;
        };
        let Some(store) = crate::strategy::global() else {
            return;
        };
        let key = crate::strategy::goal_key(session);
        // Fire-exactly-once: plan only when the slot is provably empty. A
        // continuation / re-set (Ok(Some)) or an unreadable row (Err) both skip,
        // so a transient get failure never risks a double-write (P7).
        if !matches!(store.get(&key), Ok(None)) {
            return;
        }
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: crate::strategy::planner::env_summary(),
            lessons: goal.lessons.clone(),
        };
        if let Some(strategy) = crate::strategy::planner::plan_strategy(
            provider,
            &goal.objective,
            &ctx,
            Some(goal.id.clone()),
        )
        .await
        {
            // Best-effort: a put failure must not fail the goal command.
            let _ = store.put(&key, &strategy);
        }
    }

    /// Delete this session's goal-welded Strategy (best-effort, no-op when no
    /// global store exists). Single source for every tool-side authoritative
    /// termination — `clear`, and a self-reported terminal `update` — so the
    /// stale plan does not bleed into later plain turns of the reused session
    /// (the goal tier resolves FIRST in `resolve_active_strategy`). Mirrors the
    /// gateway-side `clear_goal_welded_strategy`; the loop-keyed Strategy (if
    /// any) is untouched.
    fn clear_welded_strategy(&self, session: &str) {
        if let Some(strat) = crate::strategy::global() {
            if let Err(e) = strat.delete(&crate::strategy::goal_key(session)) {
                info!(session = %session, error = %e,
                    "goal: failed to delete welded strategy on termination (ignored)");
            }
        }
    }
}

/// Hard ceiling on autonomous continuations a single goal may request,
/// regardless of what the caller asks for (R5 menu-bar-first backstop).
const MAX_PURSUIT_ITERATIONS: u32 = 50;

/// BT-B-R4-01: per-field byte cap on goal strings (objective / note /
/// lesson). A single LLM-supplied unbounded string becomes a re-render
/// tax on every turn through StandingGoalLayer. 16 KiB matches the
/// upper bound note_manage and other text-bearing tools already use.
const MAX_GOAL_STRING: usize = 16 * 1024;

/// Clamp a requested autonomous-iteration cap to the hard ceiling
/// (`MAX_PURSUIT_ITERATIONS`, R5 menu-bar-first backstop). Shared by `set` and the
/// in-place `update` adjuster so both honour the same ceiling.
const fn clamp_iterations(requested: u32) -> u32 {
    if requested > MAX_PURSUIT_ITERATIONS {
        MAX_PURSUIT_ITERATIONS
    } else {
        requested
    }
}

/// Convert a wall-clock budget in minutes to an absolute deadline (Unix epoch
/// ms) measured from `now`. Saturating throughout so a large value can never
/// overflow. Shared by `set` and `update`.
const fn deadline_from_minutes(now: u64, minutes: u32) -> u64 {
    now.saturating_add((minutes as u64).saturating_mul(60_000))
}

/// Wall-clock milliseconds since the Unix epoch; 0 if the clock is before
/// the epoch (never in practice).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Reject a born-dead cap at the boundary (P7). `pursuit_max_iterations=0` and
/// `timeout_minutes=0` were both accepted: the goal was created, the very next
/// continuation hook found it instantly exhausted, and the user got a baffling
/// "⏹ reached its iteration cap (0 iterations)" push for work that never
/// started. The loop tool rejects the identical input (`reject_zero_cap`); this
/// is goal's parity.
fn reject_zero_caps(args: &GoalArgs) -> Result<()> {
    if args.pursuit_max_iterations == Some(0) {
        return Err(AlephError::tool(
            "pursuit_max_iterations must be at least 1 — 0 would create a goal \
             that is exhausted before its first autonomous step. Omit it for an \
             interactive (non-autonomous) goal."
                .to_string(),
        ));
    }
    if args.timeout_minutes == Some(0) {
        return Err(AlephError::tool(
            "timeout_minutes must be at least 1 — 0 sets a deadline in the past, \
             which blocks the goal on its next turn. Omit it for no time limit."
                .to_string(),
        ));
    }
    if args.token_budget == Some(0) {
        return Err(AlephError::tool(
            "token_budget must be at least 1 — 0 exhausts the budget after the \
             first step (a near-born-dead pursuit with a baffling \"reached its \
             token budget (0 tokens)\" push). Omit it for no budget."
                .to_string(),
        ));
    }
    if args.wait_minutes == Some(0) {
        return Err(AlephError::tool(
            "wait_minutes must be at least 1 — a zero wait parks and wakes in the \
             same instant. Omit it to continue immediately."
                .to_string(),
        ));
    }
    Ok(())
}

/// Boundary validation for the wait-barrier parameters (P7): they only make
/// sense on a still-active AUTONOMOUS goal — the continuation path is the
/// barrier's sole consumer, so parking a passive/terminal goal would silently
/// do nothing and the model must learn that here, not never.
///
/// `goal` is the goal AS THIS UPDATE LEAVES IT, not the stored snapshot: one
/// call can both arm autonomous pursuit and park it, and validating the
/// pre-update row rejected that legal combination while accepting the illegal
/// one (a park on a goal the same call moves out of `Active`).
fn validate_wait_args(args: &GoalArgs, goal: &Goal) -> Result<()> {
    if args.wait_minutes.is_none() && args.wait_for_task.is_none() {
        return Ok(());
    }
    if args.wait_minutes.is_some() && args.wait_for_task.is_some() {
        return Err(AlephError::tool(
            "pass either wait_minutes or wait_for_task, not both — a pursuit \
             parks on exactly one barrier."
                .to_string(),
        ));
    }
    if !matches!(goal.pursuit, PursuitMode::Active { .. }) {
        return Err(AlephError::tool(
            "wait barriers only apply to autonomous goals (set with \
             pursuit_max_iterations) — an interactive goal has no continuation \
             loop to park."
                .to_string(),
        ));
    }
    if goal.status != GoalStatus::Active {
        return Err(AlephError::tool(format!(
            "a wait barrier only wakes an active pursuit; this goal is \
             '{}'. Pass status='active' in the same update to resume it, or \
             drop the wait.",
            goal.status.as_str()
        )));
    }
    if args
        .wait_for_task
        .as_deref()
        .is_some_and(|t| t.trim().is_empty())
    {
        return Err(AlephError::tool(
            "wait_for_task requires a non-empty coordination task id.".to_string(),
        ));
    }
    // BT-B-R4-02: cap the wait_for_task id length and reject control bytes.
    // The id is later passed to the team / coord-task store for lookup; a
    // multi-megabyte string would sit in the validation path until the
    // store call returns, and a control-byte-laden id could confuse the
    // store's display layer when the wake-up banner is rendered.
    // Existence is checked at the wait-barrier arm (where we have access
    // to the store) so this layer only needs to bound the input.
    if let Some(task_id) = args.wait_for_task.as_deref() {
        if task_id.len() > 256 {
            return Err(AlephError::tool(format!(
                "wait_for_task id is {} bytes; max 256",
                task_id.len()
            )));
        }
        if task_id.chars().any(|c| c.is_control()) {
            return Err(AlephError::tool(
                "wait_for_task id contains a control byte; expected a printable coord task id"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Name the limit that will immediately re-block an `Active` autonomous goal, or
/// `None` when the pursuit really can run.
///
/// The resume recipe is "lift the binding limit AND set status=active"; doing
/// only the second half persists an `Active` goal that the very next continuation
/// hook finds exhausted, re-Blocks, and announces to the user's channel as
/// "⏹ reached its iteration cap" — for a resume they thought had worked. Only the
/// two limits the tool can actually evaluate are checked here: the iteration cap
/// and the wall-clock deadline. The token budget needs the live session counter,
/// which only the continuation hook has.
fn inert_resume_reason(goal: &Goal, now: u64) -> Option<String> {
    let PursuitMode::Active { max_iterations } = goal.pursuit else {
        return None; // interactive goal: nothing autonomous to resume.
    };
    if goal.status != GoalStatus::Active {
        return None; // paused/blocked/complete on purpose.
    }
    if goal.continuations_used >= max_iterations {
        return Some(format!(
            "Not resumed: all {max_iterations} autonomous iterations are already \
             spent. Pass pursuit_max_iterations greater than {} to give it more \
             runway.",
            goal.continuations_used
        ));
    }
    if goal.deadline_ms.is_some_and(|d| now != 0 && now > d) {
        return Some(
            "Not resumed: the wall-clock deadline has already passed. Pass a fresh \
             timeout_minutes to give it more time."
                .to_string(),
        );
    }
    None
}

/// Reject a gate command the gate runner cannot execute (P7). The per-goal gate
/// is run through `ShellStopHook::shell_safe`, which refuses shell
/// metacharacters — so `gate_command='cargo build && cargo test'` produced a gate
/// that could never render a verdict. The model has to learn that HERE, at the
/// moment it sets the gate, not silently several autonomous iterations later at
/// the completion claim (where it now fails closed and burns an iteration).
fn validate_gate_command(cmd: &str) -> Result<()> {
    if crate::verification::stop_hooks::is_shell_safe(cmd) {
        return Ok(());
    }
    Err(AlephError::tool(format!(
        "gate_command contains shell metacharacters the gate runner refuses to \
         execute (no &&, ||, ;, |, $, backticks, redirects): {cmd}. Use ONE \
         command (e.g. 'cargo test'); wrap a multi-step check in a script and \
         call that."
    )))
}

/// Who owns this session's objective via an `owns_reference` edge — refusing
/// the write rather than granting it when the graph cannot be read.
///
/// The ACL's whole job is to stop a governed loop rewriting its own reference,
/// so "I could not find out" must land on the deny side. Previously a locked /
/// busy `loop_graph.db` collapsed to `None` and the write went through
/// silently; an ungoverned install still costs nothing, because "subsystem
/// never booted" stays `Ok(None)`.
fn governing_owner_or_refuse(session: &str) -> Result<Option<String>> {
    crate::loop_graph::service::governing_owner(session).map_err(|e| {
        AlephError::tool(format!(
            "无法读取治理图（{e}）——拒绝改动一个可能被治理的 objective。请稍后重试。"
        ))
    })
}

#[async_trait]
impl AlephTool for GoalTool {
    const NAME: &'static str = "goal";
    const DESCRIPTION: &'static str =
        "Manage a STANDING GOAL — a persistent objective you keep pursuing \
         across turns until it is achieved. Create one with action='set' ONLY \
         when the user explicitly asks you to pursue a standing objective \
         (optionally with a token_budget, pursuit_max_iterations to let \
         the system continue autonomously, and timeout_minutes to cap wall-clock pursuit). Optionally attach a gate_command (a shell test like 'cargo test' that must \
exit 0 before an autonomous goal is accepted as complete). On action='update' \
you may also pass a lesson to record a durable, transferable insight for future \
iterations (never an environment-specific or transient failure, nor a 'tool is \
broken' claim — those poison later iterations). action='update' can ALSO adjust \
token_budget, pursuit_max_iterations, gate_command, and timeout_minutes in place \
without losing accumulated progress (continuation count, lessons) — to resume a \
goal blocked at its iteration cap / deadline / token budget, update status='active' \
together with a higher pursuit_max_iterations / fresh timeout_minutes / higher \
token_budget. \
         On an autonomous goal, action='update' with wait_minutes=N parks the \
         pursuit until an exact timer wake (use when blocked on slow external \
         work — a rate-limit cooldown, a long build — instead of burning \
         iterations polling), and wait_for_task='<coord task id>' parks it until \
         that team/workflow task settles; pass a note saying why. \
         status='active' un-parks. \
         Read it with action='get'. When \
         you have achieved the objective, self-report with action='update', \
         status='complete'; if you are stuck and need the user, use \
         status='blocked'. Use status='paused'/'active' only when the user \
         explicitly asks to pause or resume. Remove it with action='clear'. \
         List every standing goal across ALL sessions with action='list' (use \
         this to answer 'what goals am I pursuing?' since action='get' only sees \
         the current session). To act on a goal `list` shows in ANOTHER session, \
         pass session='<key as list prints it>' — accepted by get, clear, and by \
         update restricted to status='paused'; everything that arms a pursuit \
         must run in that goal's own session. action='pause_all' is the kill \
         switch for 'stop all the autonomous work' (operator only; objectives \
         and lessons are kept). \
         The goal is re-surfaced into your prompt every turn while active.";

    type Args = GoalArgs;
    type Output = GoalOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session = self.session().await;
        if session.is_empty() {
            return Err(AlephError::tool(
                "goal tool has no active session binding".to_string(),
            ));
        }
        info!(session = %session, action = ?args.action, "goal operation");
        let now = now_ms();

        match args.action {
            GoalAction::Set => {
                Self::reject_remote(&session, &args, "set")?;
                let objective = args.objective.as_deref().ok_or_else(|| {
                    AlephError::tool("goal 'set' requires 'objective'".to_string())
                })?;
                // BT-B-R4-01: cap the per-field string size. A single
                // model-supplied objective / note / lesson of unbounded
                // length is a one-call DoS that re-renders through
                // StandingGoalLayer on every turn. The cap matches what
                // note_manage and other text-bearing tools already use.
                if objective.len() > MAX_GOAL_STRING {
                    return Err(AlephError::tool(format!(
                        "objective is {} bytes; max {MAX_GOAL_STRING}",
                        objective.len()
                    )));
                }
                if let Some(ref note) = args.note {
                    if note.len() > MAX_GOAL_STRING {
                        return Err(AlephError::tool(format!(
                            "note is {} bytes; max {MAX_GOAL_STRING}",
                            note.len()
                        )));
                    }
                }
                reject_zero_caps(&args)?;
                if args.wait_minutes.is_some() || args.wait_for_task.is_some() {
                    return Err(AlephError::tool(
                        "wait_minutes / wait_for_task apply to action='update' — a \
                         goal is never born parked; set it first, then park when \
                         the pursuit actually hits the wait."
                            .to_string(),
                    ));
                }
                if let Some(cmd) = args.gate_command.as_deref() {
                    if !cmd.trim().is_empty() {
                        validate_gate_command(cmd)?;
                    }
                }
                // owns_reference write-protection (loop_graph §6.2): `set` on a
                // session that already has a goal REPLACES its objective — a
                // reference change. A governed loop is read-only on its own
                // reference; structural field ownership, not judgment (R7-clean,
                // same class as the exec-tier metadata rules).
                let previous = self.store.get(&session)?;
                if previous.is_some() {
                    if let Some(owner) = governing_owner_or_refuse(&session)? {
                        return Err(AlephError::tool(format!(
                            "此会话的 goal objective 由 {owner} 治理（owns_reference edge）。\
                             变更理由请写成提案 note（note_manage，tag: reference-proposal），\
                             由治理环在其周期裁决；或经用户确认先解除托管：\
                             loop_graph(action='unlink', from_id='{owner}', \
                             to_id='goal:{session}', edge='owns_reference')。"
                        )));
                    }
                }
                // `tokens_at_start` is seeded to 0 here: the tool has no live
                // per-session token counter at call time. The autonomous driver
                // captures the real baseline lazily on the first continuation
                // hook that sees a budget set (`Goal::with_baseline`, codex's
                // `tokenStartFresh` pattern) and then enforces `token_budget`
                // against the live session total. Until then only the iteration
                // and deadline caps apply, so an interactive (non-pursuit) goal's
                // budget remains a soft, surfaced guardrail.
                let mut goal = Goal::new(&session, objective, 0, now)
                    .with_budget(args.token_budget)
                    .with_note(args.note.clone(), now)
                    // P1 data isolation: stamp the owning user/scope from the
                    // ambient attribution of THIS creating run. `None` outside
                    // any scope (e.g. no caller user resolved) leaves the goal
                    // unscoped — legacy owner semantics, never guessed.
                    .with_owner_scope(crate::scope::current_scope().as_ref());
                if let Some(requested) = args.pursuit_max_iterations {
                    // Hard cap autonomous continuations (R5 menu-bar-first): an
                    // unbounded value would let a single session self-run for
                    // days. The clamped value is surfaced to the model via
                    // `render` so it sees the effective cap.
                    let max_iterations = clamp_iterations(requested);
                    goal = goal.with_pursuit(PursuitMode::Active { max_iterations });
                }
                goal = goal.with_gate_command(args.gate_command.clone());
                if let Some(minutes) = args.timeout_minutes {
                    goal = goal.with_deadline_ms(Some(deadline_from_minutes(now, minutes)));
                }
                // A `set` over an existing goal OVERWRITES the session's row, so
                // a new objective destroys the old goal's lessons exactly the
                // way `clear` does. Graduate them first, unless the id is
                // unchanged (same objective = a re-set/continuation, whose
                // lessons the nightly stage still owns).
                if let Some(prev) = previous.filter(|p| p.id != goal.id) {
                    let promoted = self.promote_lessons_before_loss(&session, &prev).await;
                    if promoted > 0 {
                        info!(session = %session, promoted,
                            "goal set: promoted the replaced goal's lessons before overwrite");
                    }
                }
                self.store.put(&goal)?;
                // Objective-change auto-invalidation (spec §6): if a welded
                // Strategy exists for this session and it cross-references a
                // DIFFERENT goal id than the one we just minted, the objective
                // changed under it — drop the stale map (BEFORE the planner fires
                // below) so the planner re-mints a fresh map for the new
                // objective. A Strategy with no cross-ref (goal_id None) or a
                // matching id is left intact.
                if let Some(strat_store) = crate::strategy::global() {
                    let key = crate::strategy::goal_key(&session);
                    match strat_store.get(&key) {
                        Ok(Some(existing)) => {
                            if existing
                                .goal_id
                                .as_deref()
                                .is_some_and(|old| old != goal.id)
                            {
                                if let Err(e) = strat_store.delete(&key) {
                                    info!(session = %session, error = %e,
                                        "goal set: failed to invalidate stale strategy (ignored)");
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            info!(session = %session, error = %e,
                                "goal set: failed to read strategy for invalidation (ignored)");
                        }
                    }
                }
                self.maybe_plan_strategy(&session, &goal).await;
                Ok(GoalOutput {
                    success: true,
                    message: format!("Set. {}", Self::render(&goal, now)),
                })
            }
            GoalAction::Get => {
                let target = self.resolve_target(&session, args.session.as_deref(), "get")?;
                let scope = Self::scope_suffix(&session, &target);
                match self.store.get(&target)? {
                    Some(goal) => Ok(GoalOutput {
                        success: true,
                        message: if scope.is_empty() {
                            Self::render(&goal, now)
                        } else {
                            format!("{target}: {}", Self::render(&goal, now))
                        },
                    }),
                    None => Ok(GoalOutput {
                        success: true,
                        message: match scope.is_empty() {
                            true => "No standing goal set for this session.".to_string(),
                            false => format!("No standing goal set{scope}."),
                        },
                    }),
                }
            }
            GoalAction::Update => {
                // Cross-session `update` is QUIET-ONLY and handled entirely
                // here, before the local path: the single remote transition is
                // `status='paused'` (plus an optional note). Everything else in
                // this arm — budget/cap/deadline raises, wait barriers, gate
                // swaps, lessons — either arms the pursuit or edits state whose
                // owner session is the only one with the context to judge it.
                if let Some(target) = args
                    .session
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty() && *t != session)
                {
                    return self.remote_pause(&session, target, &args, now);
                }
                let mut goal = self
                    .store
                    .get(&session)?
                    .ok_or_else(|| AlephError::tool("no standing goal to update".to_string()))?;
                reject_zero_caps(&args)?;
                // `update` never reads `objective`, so passing one used to be
                // silently discarded — and the reply echoed the OLD objective
                // under a `success: true` "Updated." The cross-session path
                // refuses smuggled fields for exactly this reason; the local
                // path owes the same honesty.
                //
                // Refused rather than implemented: the objective is the
                // reference an `owns_reference` edge governs, and `set` is
                // where that ACL lives (`governing_owner_or_refuse`).
                // Implementing an objective change here would mean a second
                // copy of the same gate — and a gate with two implementations
                // is how a governed loop rewrites its own reference through
                // the door nobody guarded.
                if args.objective.is_some() {
                    return Err(AlephError::tool(
                        "goal 'update' cannot change the objective — it only adjusts \
                         status, caps, budget, deadline, gate, lessons and notes. Use \
                         goal(action='set', objective='…') to replace the objective \
                         (that path carries the governance check a replacement needs); \
                         note it resets the autonomous iteration count."
                            .to_string(),
                    ));
                }
                let prev_status = goal.status;
                // The armed timer instant BEFORE this update mutates the
                // barrier. A parked timer's continuation is claimed at the
                // parking run's post_run (pending_continuation_ms == the wake
                // instant), and that marker is store-owned — `without_wait` /
                // `with_wait_until` never touch it, and `commit_field_update`
                // preserves it. So an explicit un-park (or re-park) that only
                // clears the barrier would leave the detached timer armed:
                // the un-parking run's post_run would hit the in-flight
                // pending gate and stay Idle until the ORIGINAL wake. Capture
                // it here; after the commit, supersede that stale marker so
                // the fresh claim can fire immediately.
                let pre_wait_until = goal.waiting_until_ms;
                let barrier_touched = args.status.is_some()
                    || args.wait_minutes.is_some()
                    || args.wait_for_task.is_some();
                if let Some(status) = args.status {
                    goal = goal.with_status(status, now);
                    // ANY explicit status write drops a wait barrier: away
                    // from Active it is meaningless (hermes: pause/clear drop
                    // it), and an explicit `status='active'` on a parked goal
                    // is the un-park command.
                    goal = goal.without_wait(now);
                    if status != GoalStatus::Complete {
                        // `Passed` is the objective gate's verdict on ONE completion
                        // claim, and `awaiting_gate` skips the gate whenever it is
                        // set. Leaving it on a re-activated goal permanently
                        // disarmed the gate: the model's next `complete` claim —
                        // and every one after it — was accepted unverified. Reset it
                        // to the resting state the type documents, exactly as
                        // `reopen_after_gate_failure` does on the veto path.
                        goal = goal.with_gate_outcome(crate::goal::GateOutcome::Unchecked, now);
                    }
                }
                // In-place reconfiguration (R8: natural-language tuning without a
                // destructive clear+set that would wipe continuations_used /
                // lessons / gate_outcome). Each field is touched ONLY when the
                // caller provides it, so an omitted field stays unchanged.
                // Raising a cap / budget / deadline here is exactly how a goal
                // blocked at one of those limits is resumed (see the cap notes in
                // goal_pursuit.rs): once the binding limit is lifted and status is
                // set back to `active`, the continuation hook fires again.
                if args.token_budget.is_some() {
                    goal = goal.with_budget(args.token_budget);
                }
                if let Some(requested) = args.pursuit_max_iterations {
                    goal = goal.with_pursuit(PursuitMode::Active {
                        max_iterations: clamp_iterations(requested),
                    });
                }
                if let Some(minutes) = args.timeout_minutes {
                    goal = goal.with_deadline_ms(Some(deadline_from_minutes(now, minutes)));
                }
                if let Some(cmd) = args.gate_command.clone() {
                    // The gate command IS part of the reference: it is the
                    // objective's operational definition — the command whose
                    // exit code decides "was this achieved". A governed loop
                    // that cannot rewrite its objective but CAN swap
                    // `cargo test` for `true` (or clear the gate outright) has
                    // simply moved the goalpost by another door, and the very
                    // next `update(status='complete')` sails through. Same ACL,
                    // same escape hatch, same wording as the objective itself.
                    if let Some(owner) = governing_owner_or_refuse(&session)? {
                        return Err(AlephError::tool(format!(
                            "此会话 goal 的 gate_command 由 {owner} 治理（owns_reference edge）——\
                             它是 objective 的可执行定义，与 objective 同属参照。\
                             变更理由请写成提案 note（note_manage，tag: reference-proposal），\
                             由治理环在其周期裁决；或经用户确认先解除托管：\
                             loop_graph(action='unlink', from_id='{owner}', \
                             to_id='goal:{session}', edge='owns_reference')。"
                        )));
                    }
                    // Empty string clears the per-goal gate; anything else sets it
                    // — after the same boundary check `set` applies, so an
                    // unrunnable gate can never be installed by either door.
                    let next = if cmd.trim().is_empty() {
                        None
                    } else {
                        validate_gate_command(&cmd)?;
                        Some(cmd)
                    };
                    goal = goal.with_gate_command(next);
                }
                if args.note.is_some() {
                    // BT-B-R4-01: same cap as `set`; an unbounded update
                    // note re-renders through StandingGoalLayer on every
                    // turn just as a long objective would.
                    if let Some(ref n) = args.note {
                        if n.len() > MAX_GOAL_STRING {
                            return Err(AlephError::tool(format!(
                                "note is {} bytes; max {MAX_GOAL_STRING}",
                                n.len()
                            )));
                        }
                    }
                    goal = goal.with_note(args.note.clone(), now);
                } else if matches!(args.status, Some(GoalStatus::Active))
                    && prev_status != GoalStatus::Active
                {
                    // Re-activating a paused/blocked goal: the existing note
                    // explained why it stopped (cap / deadline / error) and is now
                    // stale. Clear it so the prompt does not surface a
                    // contradictory "blocked" reason on a freshly-active goal. An
                    // explicit note (handled above) always wins over this reset.
                    goal = goal.with_note(None, now);
                }
                if let Some(lesson) = args.lesson.clone() {
                    // BT-B-R4-01: same cap for the lesson field.
                    if lesson.len() > MAX_GOAL_STRING {
                        return Err(AlephError::tool(format!(
                            "lesson is {} bytes; max {MAX_GOAL_STRING}",
                            lesson.len()
                        )));
                    }
                    goal = goal.with_lesson_appended(lesson, now);
                }
                // Validated here, not at the top of the arm: `goal` now
                // carries this update's status and pursuit changes, so one
                // call may legitimately arm autonomous pursuit and park it,
                // and a park on a goal this call moves out of `Active` is
                // correctly refused. See `validate_wait_args`.
                validate_wait_args(&args, &goal)?;
                // Wait barrier (model self-park, R7): the continuation hook
                // turns a deadline barrier into an exact timer wake and a
                // task barrier into an event-driven wake. The wake run costs
                // one iteration like any autonomous run.
                if let Some(minutes) = args.wait_minutes {
                    goal = goal.with_wait_until(
                        now.saturating_add((minutes as u64).saturating_mul(60_000)),
                        args.note.clone(),
                        now,
                    );
                }
                if let Some(task_id) = args.wait_for_task.clone() {
                    goal = goal.with_wait_on_task(task_id, args.note.clone(), now);
                }
                // Honest report on a resume that cannot actually take — checked
                // BEFORE persisting. Re-activating a goal whose binding limit is
                // still spent leaves it Active for exactly one hook, which
                // re-Blocks it and pushes a bewildering "⏹ cap reached" to the
                // user's channel — the exact outcome this branch exists to prevent.
                // Committing first defeated that: the store then held an Active,
                // cap-spent goal the next `post_run` re-Blocked (and cleared its
                // welded plan). Reject before the write so nothing is persisted;
                // `continuations_used` is store-owned and preserved by
                // `commit_field_update`, so evaluating it here is check-equivalent
                // and only drops the spurious Active write. Names the parameter
                // that would fix it (loop's pre-commit-validation honesty parity).
                if let Some(blocker) = inert_resume_reason(&goal, now) {
                    return Ok(GoalOutput {
                        success: false,
                        message: format!("{blocker} {}", Self::render(&goal, now)),
                    });
                }
                // Atomic commit: re-reads the LIVE `pending_continuation_ms` under
                // the store lock and keeps it, so a tool update landing while a
                // claimed continuation fires cannot restore a stale marker (which
                // would stall the next claim until the 60s stale grace). The claim
                // pipeline stays the single owner of that field. The status CAS
                // (`prev_status`) keeps a concurrent block/pause/complete that won
                // the read→write gap from being overwritten by this snapshot's
                // stale lifecycle cluster. `Gone` = the goal was cleared between
                // this turn's read and the write.
                match self.store.commit_field_update(&goal, prev_status)? {
                    crate::goal::FieldUpdate::Gone => {
                        return Ok(GoalOutput {
                            success: false,
                            message: "The standing goal was cleared while this update ran — \
                                      nothing to update. Set a new goal if you still need one."
                                .to_string(),
                        });
                    }
                    crate::goal::FieldUpdate::StatusSuperseded(live_status) => {
                        return Ok(GoalOutput {
                            success: false,
                            message: format!(
                                "The goal's status changed to '{}' concurrently — that \
                                 transition won, so your status change was dropped (other \
                                 field updates were kept). Re-read with goal(action='get') \
                                 and retry if still needed.",
                                live_status.as_str()
                            ),
                        });
                    }
                    crate::goal::FieldUpdate::Committed => {}
                }
                // Supersede a stale timer marker left armed by a barrier this
                // update just cleared/replaced (see `pre_wait_until` above), so
                // the un-parking run's post_run claims immediately instead of
                // waiting out the original wake. No-op unless the pre-update
                // barrier was a claimed timer (marker == its wake instant).
                if let Some(armed) = pre_wait_until {
                    if barrier_touched {
                        if let Err(e) = self.store.supersede_wait_timer(&session, armed) {
                            info!(session = %session, error = %e,
                                "goal: failed to supersede stale wait timer on un-park (ignored)");
                        }
                    }
                }
                // Clear the welded plan on a tool-owned authoritative termination
                // so the stale plan does not bleed into later plain turns of this
                // reused session. `Blocked` is never re-arbitrated; a Passive
                // `Complete` is terminal here (the gateway continuation hook is a
                // no-op for Passive goals). An Active-pursuit `Complete` is
                // deliberately NOT cleared here — the gateway gate arbitration
                // owns it, so a gate veto can still reopen the goal WITH its plan
                // intact (and the hook clears it on the confirmed/gate-less end).
                let tool_owned_terminal = matches!(goal.status, GoalStatus::Blocked)
                    || (matches!(goal.status, GoalStatus::Complete)
                        && matches!(goal.pursuit, PursuitMode::Passive));
                if tool_owned_terminal {
                    self.clear_welded_strategy(&session);
                    // A Passive complete is a victory claim too — poke any
                    // paired loop_graph watchers, once per completion write
                    // (the same store CAS the gateway hook uses for the
                    // gate-less Active complete, so neither path can re-fire
                    // on later re-observations). Blocked is a halt, not a
                    // victory — no poke.
                    if matches!(goal.status, GoalStatus::Complete) {
                        match self.store.try_claim_settle_notify(&goal) {
                            Ok(true) => {
                                // Hand the one-shot claim back when nothing was
                                // actually poked — otherwise this completion's
                                // review is retired permanently (the stamp for a
                                // still-Complete goal never moves again).
                                if !crate::loop_graph::service::notify_goal_settled(&session).await
                                {
                                    if let Err(e) = self.store.release_settle_notify(&goal) {
                                        info!(session = %session, error = %e,
                                            "goal: settle-notify release failed (ignored)");
                                    }
                                }
                            }
                            Ok(false) => {}
                            Err(e) => info!(session = %session, error = %e,
                                "goal: settle-notify claim failed; watcher poke skipped (ignored)"),
                        }
                    }
                }
                Ok(GoalOutput {
                    success: true,
                    message: format!("Updated. {}", Self::render(&goal, now)),
                })
            }
            GoalAction::Clear => {
                let target = self.resolve_target(&session, args.session.as_deref(), "clear")?;
                let scope = Self::scope_suffix(&session, &target);
                let session = target;
                // Report what actually happened: an unconditional "Standing goal
                // cleared." on a session that never had one told the model it had
                // undone something, which is how a model ends up assuring the user
                // their (never-created) goal is gone.
                let existed = self.store.get(&session)?;
                // owns_reference write-protection: clearing a governed goal
                // deletes the very reference the governing loop owns.
                if existed.is_some() {
                    if let Some(owner) = governing_owner_or_refuse(&session)? {
                        return Err(AlephError::tool(format!(
                            "此会话的 goal 由 {owner} 治理（owns_reference edge），clear 会删除\
                             被治理的参照。请写提案 note（tag: reference-proposal）交治理环裁决，\
                             或经用户确认先 loop_graph(action='unlink', from_id='{owner}', \
                             to_id='goal:{session}', edge='owns_reference') 再 clear。"
                        )));
                    }
                }
                // Graduate the lessons BEFORE the row goes away — the nightly
                // `GoalLessonsPromoteStage` sweeps `list_all()`, so anything
                // learned since the last dream window is destroyed by this
                // DELETE otherwise. Best-effort: no indexer / a failed append
                // must never fail the user's clear.
                let promoted = match &existed {
                    Some(goal) => self.promote_lessons_before_loss(&session, goal).await,
                    None => 0,
                };
                self.store.delete(&session)?;
                // Clear the goal-welded Strategy in lockstep with the
                // authoritative goal deletion (spec §6 lifecycle). Best-effort:
                // a missing global / corrupt row is a no-op, never fails the
                // user's clear. The loop-keyed Strategy (if any) is untouched.
                self.clear_welded_strategy(&session);
                let Some(prev) = existed else {
                    return Ok(GoalOutput {
                        success: true,
                        message: match scope.is_empty() {
                            true => "No standing goal was set for this session — nothing to clear."
                                .to_string(),
                            false => format!("No standing goal was set{scope} — nothing to clear."),
                        },
                    });
                };
                // Say so when lessons survived the clear: the user asked for the
                // goal to go away, not for what it taught to go away, and the
                // note path is how they (or a later goal) get it back.
                let kept = match promoted {
                    0 => String::new(),
                    n => format!(
                        " — {n} lesson fact(s) kept in {}",
                        crate::memory::dreaming::stages::goal_lessons_promote::lessons_note_path(
                            &prev.id
                        )
                    ),
                };
                Ok(GoalOutput {
                    success: true,
                    message: format!("Standing goal cleared{scope}: {}{kept}", prev.objective),
                })
            }
            GoalAction::List => {
                // Cross-session enumeration (R6 one-core-many-shells): a goal set on one
                // channel is invisible to `get` on another, which keys by the
                // current session. Reuse the store's existing `list_all` (the
                // dream lessons-promote sweep already relies on it) so the model
                // can answer "what goals are running?" from anywhere. Corrupt
                // rows are skipped inside `list_all` (fail-safe).
                let mut goals = self.store.list_all()?;
                // Scope, don't deny — the loop sibling does the same. Every
                // other cross-session verb here is operator-gated, and
                // `get(session=…)` reveals strictly less than this line does
                // (which prints the session key and the objective text). A
                // chat-tier caller keeps a true answer about its own session,
                // plus an honest count so it never reads as "nothing running".
                let hidden = if Self::caller_is_operator() {
                    0
                } else {
                    let before = goals.len();
                    goals.retain(|g| g.session_id == session);
                    before - goals.len()
                };
                if goals.is_empty() {
                    return Ok(GoalOutput {
                        success: true,
                        message: match hidden {
                            0 => "No standing goals set in any session.".to_string(),
                            n => format!(
                                "No standing goal in this session. {n} goal(s) in other \
                                 sessions are not shown at this permission level."
                            ),
                        },
                    });
                }
                // Newest-updated first so the most relevant goals lead.
                let mut sorted = goals;
                sorted.sort_unstable_by_key(|g| std::cmp::Reverse(g.updated_at_ms));
                let mut message = format!("Standing goals ({}):\n", sorted.len());
                for goal in &sorted {
                    message.push_str(&Self::render_list_line(goal, &session, now));
                    message.push('\n');
                }
                if hidden > 0 {
                    message.push_str(&format!(
                        "({hidden} goal(s) in other sessions are not shown at this permission level.)\n"
                    ));
                }
                Ok(GoalOutput {
                    success: true,
                    message: message.trim_end().to_string(),
                })
            }
            GoalAction::PauseAll => {
                if !Self::caller_is_operator() {
                    return Err(AlephError::tool(
                        "pause_all requires operator authorization; this conversation \
                         may only manage its own goal"
                            .to_string(),
                    ));
                }
                let paused = self
                    .store
                    .pause_all_active("Paused by goal(action='pause_all').", now)?;
                if paused.is_empty() {
                    return Ok(GoalOutput {
                        success: true,
                        message: "No actively-pursued standing goals in any session.".to_string(),
                    });
                }
                Ok(GoalOutput {
                    success: true,
                    message: format!(
                        "Paused {} standing goal(s): {}. Objectives and lessons are \
                         kept — each session resumes its own with \
                         goal(action='update', status='active').",
                        paused.len(),
                        paused.join(", ")
                    ),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::GoalStore;
    use crate::sync_primitives::Arc;
    use tokio::sync::RwLock;

    fn tool_with_session(session: &str) -> (GoalTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new(session.to_string()));
        (
            GoalTool::new(store).with_session_key_handle(Some(handle)),
            dir,
        )
    }

    /// All-`None` args for `action`, so a test sets only the fields it exercises.
    fn args(action: GoalAction) -> GoalArgs {
        GoalArgs {
            action,
            objective: None,
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: None,
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        }
    }

    #[tokio::test]
    async fn set_then_get_returns_objective() {
        let (tool, _d) = tool_with_session("sess-A");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Ship the goal feature".into()),
            status: None,
            note: None,
            token_budget: Some(5000),
            pursuit_max_iterations: None,
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();

        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.contains("Ship the goal feature"));
    }

    /// Pins the P1/P2 scope stamp (`with_owner_scope` at the `set` creation
    /// arm): a goal created inside a room run's `scope::with_scope` nest
    /// lands `scope_id == "project:<id>"` — not just the unit test on
    /// `with_owner_scope` itself in `goal/types.rs`, but the actual tool
    /// call path the Kanban board's write side runs through.
    #[tokio::test]
    async fn a_goal_created_inside_a_room_run_lands_in_project_scope() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new("sess-room".to_string()));
        let tool = GoalTool::new(store.clone()).with_session_key_handle(Some(handle));

        let mut set = args(GoalAction::Set);
        set.objective = Some("ship the kanban board".into());
        let attr = crate::scope::ScopeAttribution {
            owner_user_id: "u-alice".to_string(),
            scope: crate::scope::ScopeId::Project("p-x".to_string()),
        };
        let out = crate::scope::with_scope(Some(attr), tool.call(set))
            .await
            .unwrap();
        assert!(out.success);

        let saved = store.get("sess-room").unwrap().unwrap();
        assert_eq!(saved.scope_id.as_deref(), Some("project:p-x"));
        assert_eq!(saved.owner_user_id.as_deref(), Some("u-alice"));
    }

    /// The room-outside twin: no ambient scope ⇒ current (pre-P2) behavior —
    /// unscoped, `owner_user_id`/`scope_id` both `None`.
    #[tokio::test]
    async fn a_goal_created_outside_any_room_stays_unscoped() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new("sess-personal".to_string()));
        let tool = GoalTool::new(store.clone()).with_session_key_handle(Some(handle));

        let mut set = args(GoalAction::Set);
        set.objective = Some("just for me".into());
        let out = tool.call(set).await.unwrap();
        assert!(out.success);

        let saved = store.get("sess-personal").unwrap().unwrap();
        assert_eq!(saved.scope_id, None);
        assert_eq!(saved.owner_user_id, None);
    }

    #[tokio::test]
    async fn update_complete_marks_status() {
        let (tool, _d) = tool_with_session("sess-B");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("x".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: None,
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Update,
                objective: None,
                status: Some(GoalStatus::Complete),
                note: Some("done".into()),
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("complete"));
    }

    /// Regression (W14): a Passive complete is a victory claim — the tool must
    /// consume the one-shot settle-notify claim for the completion write (it is
    /// the only site that can poke watchers for Passive goals; the gateway
    /// hook's gate-less arm requires Active pursuit). If the tool skipped the
    /// claim, this later claim on the live row would still return `true`.
    #[tokio::test]
    async fn passive_complete_consumes_the_settle_notify_claim() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new("sess-notify".to_string()));
        let tool = GoalTool::new(store.clone()).with_session_key_handle(Some(handle));

        let mut set = args(GoalAction::Set);
        set.objective = Some("x".into()); // no pursuit iterations → Passive
        tool.call(set).await.unwrap();

        let mut update = args(GoalAction::Update);
        update.status = Some(GoalStatus::Complete);
        tool.call(update).await.unwrap();

        let live = store.get("sess-notify").unwrap().unwrap();
        assert!(matches!(live.pursuit, PursuitMode::Passive));
        assert_eq!(live.status, GoalStatus::Complete);
        assert!(
            !store.try_claim_settle_notify(&live).unwrap(),
            "the Passive-complete update must have claimed the watcher poke"
        );
    }

    #[tokio::test]
    async fn update_refuses_an_objective_change_instead_of_dropping_it() {
        let (tool, _d) = tool_with_session("s-obj");
        let mut set = args(GoalAction::Set);
        set.objective = Some("original objective".into());
        tool.call(set).await.unwrap();

        let mut update = args(GoalAction::Update);
        update.objective = Some("a completely different objective".into());
        let err = tool
            .call(update)
            .await
            .expect_err("a smuggled objective must not be silently discarded");
        let msg = err.to_string();
        assert!(
            msg.contains("set"),
            "the error must point at the fix: {msg}"
        );

        // And the stored objective is untouched.
        let out = tool.call(args(GoalAction::Get)).await.unwrap();
        assert!(
            out.message.contains("original objective"),
            "got: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn pursuit_iterations_are_capped() {
        let (tool, _d) = tool_with_session("sess-cap");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("z".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: Some(1_000_000),
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        // Effective cap surfaced to the model, not the requested 1,000,000.
        assert!(out
            .message
            .contains(&format!("0/{MAX_PURSUIT_ITERATIONS} iterations used")));
    }

    #[tokio::test]
    async fn get_with_no_goal_is_graceful() {
        let (tool, _d) = tool_with_session("sess-empty");
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("no standing goal"));
    }

    #[tokio::test]
    async fn set_requires_objective() {
        let (tool, _d) = tool_with_session("sess-C");
        let err = tool
            .call(GoalArgs {
                action: GoalAction::Set,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await;
        assert!(err.is_err(), "set without objective must error");
    }

    #[tokio::test]
    async fn clear_removes_goal() {
        let (tool, _d) = tool_with_session("sess-D");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("y".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: None,
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        tool.call(GoalArgs {
            action: GoalAction::Clear,
            objective: None,
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: None,
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.message.to_lowercase().contains("no standing goal"));
    }

    /// `clear` DELETEs the goal row, and `GoalLessonsPromoteStage` only sweeps
    /// goals that still exist — so before this fix everything the pursuit
    /// learned since the last nightly dream window died with the row. The
    /// lessons must be graduated into `goal-lessons/<id>` on the way out.
    #[tokio::test]
    async fn clear_promotes_lessons_before_deleting_the_goal() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("notes");
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();

        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let goal = Goal::new("sess-lessons", "ship it", 0, 0)
            .with_lesson_appended("run migrations first".into(), 1);
        store.put(&goal).unwrap();

        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let tool = GoalTool::new(store.clone())
            .with_session_key_handle(Some(Arc::new(RwLock::new("sess-lessons".to_string()))))
            .with_lesson_indexer(Some(Arc::new(NoteIndexer::new(mem_dir.clone(), backend))));

        let out = tool.call(args(GoalAction::Clear)).await.unwrap();
        assert!(out.success);
        assert!(
            out.message.contains("lesson fact(s) kept"),
            "clear must report what it preserved: {}",
            out.message
        );

        // The session key is not a parseable `SessionKey`, so the promotion
        // falls back to the default agent's note tree.
        let note = mem_dir
            .join(crate::routing::DEFAULT_AGENT_ID)
            .join("goal-lessons")
            .join(format!("{}.md", goal.id));
        let body = tokio::fs::read_to_string(&note)
            .await
            .expect("lessons note must survive the clear");
        assert!(body.contains("run migrations first"), "{body}");

        // …and the goal itself is still gone.
        assert!(store.get("sess-lessons").unwrap().is_none());
    }

    /// `set` with a new objective OVERWRITES the session's row — the same
    /// destruction as `clear`, through a different verb. The replaced goal's
    /// lessons must graduate on the way out too.
    #[tokio::test]
    async fn set_promotes_the_replaced_goals_lessons() {
        let dir = tempfile::tempdir().unwrap();
        let mem_dir = dir.path().join("notes");
        tokio::fs::create_dir_all(&mem_dir).await.unwrap();

        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let old = Goal::new("sess-reset", "old objective", 0, 0)
            .with_lesson_appended("the deploy needs a fresh token".into(), 1);
        store.put(&old).unwrap();

        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());
        let tool = GoalTool::new(store.clone())
            .with_session_key_handle(Some(Arc::new(RwLock::new("sess-reset".to_string()))))
            .with_lesson_indexer(Some(Arc::new(NoteIndexer::new(mem_dir.clone(), backend))));

        tool.call(GoalArgs {
            objective: Some("brand new objective".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();

        let note = mem_dir
            .join(crate::routing::DEFAULT_AGENT_ID)
            .join("goal-lessons")
            .join(format!("{}.md", old.id));
        let body = tokio::fs::read_to_string(&note)
            .await
            .expect("replaced goal's lessons must survive the overwrite");
        assert!(body.contains("the deploy needs a fresh token"), "{body}");
    }

    /// No indexer wired (memory disabled / unit tests) → `clear` still clears.
    #[tokio::test]
    async fn clear_without_a_lesson_indexer_still_clears() {
        let (tool, _d) = tool_with_session("sess-no-indexer");
        tool.call(GoalArgs {
            objective: Some("y".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let out = tool.call(args(GoalAction::Clear)).await.unwrap();
        assert!(out.success, "{}", out.message);
        assert!(
            !out.message.contains("lesson fact(s) kept"),
            "nothing was promoted: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn set_with_gate_command_is_rendered() {
        let (tool, _d) = tool_with_session("sess-gate");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Ship X".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: Some(3),
            gate_command: Some("cargo test".into()),
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.message.contains("per-goal command set"));
    }

    #[tokio::test]
    async fn update_with_lesson_appends_and_renders() {
        let (tool, _d) = tool_with_session("sess-lesson");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Y".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: None,
            gate_command: None,
            lesson: None,
            timeout_minutes: None,
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Update,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: Some("don't skip lint".into()),
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.message.contains("lessons (1)"));
        assert!(out.message.contains("don't skip lint"));
    }

    #[tokio::test]
    async fn set_with_timeout_minutes_sets_deadline() {
        let (tool, _d) = tool_with_session("sess-timeout");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("bounded run".into()),
            status: None,
            note: None,
            token_budget: None,
            pursuit_max_iterations: Some(5),
            gate_command: None,
            lesson: None,
            timeout_minutes: Some(30),
            wait_minutes: None,
            wait_for_task: None,
            session: None,
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Get,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(
            out.message.contains("deadline in ~30m"),
            "got: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn render_status_uses_snake_case_vocabulary() {
        // The model types `status='active'`; the render it reads back must match
        // that vocabulary, not a `{:?}` Debug dump `status=Active`.
        let (tool, _d) = tool_with_session("sess-vocab");
        tool.call(GoalArgs {
            objective: Some("polish render".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let out = tool.call(args(GoalAction::Get)).await.unwrap();
        assert!(
            out.message.contains("status=active"),
            "got: {}",
            out.message
        );
        assert!(
            !out.message.contains("status=Active"),
            "must not Debug-dump a capitalized status: {}",
            out.message
        );
    }

    /// Set an Active goal, attach a lesson, then `update` the iteration cap in
    /// place. The new cap is surfaced and — crucially — the accumulated lesson
    /// survives (the bug this fixes: previously only clear+set could raise the
    /// cap, wiping progress).
    #[tokio::test]
    async fn update_raises_iteration_cap_without_losing_progress() {
        let (tool, _d) = tool_with_session("sess-resume");
        tool.call(GoalArgs {
            objective: Some("Long migration".into()),
            pursuit_max_iterations: Some(3),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        tool.call(GoalArgs {
            lesson: Some("checkpoint after each table".into()),
            ..args(GoalAction::Update)
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                status: Some(GoalStatus::Active),
                pursuit_max_iterations: Some(10),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            out.message.contains("0/10 iterations used"),
            "got: {}",
            out.message
        );
        assert!(
            out.message.contains("checkpoint after each table"),
            "lesson kept"
        );
    }

    #[tokio::test]
    async fn update_adjusts_budget_and_deadline_in_place() {
        let (tool, _d) = tool_with_session("sess-budget");
        tool.call(GoalArgs {
            objective: Some("Bounded work".into()),
            pursuit_max_iterations: Some(5),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                token_budget: Some(80_000),
                timeout_minutes: Some(45),
                wait_minutes: None,
                wait_for_task: None,
                session: None,
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            out.message.contains("token_budget=80000"),
            "got: {}",
            out.message
        );
        assert!(
            out.message.contains("deadline in ~45m"),
            "got: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn update_gate_command_sets_then_clears() {
        let (tool, _d) = tool_with_session("sess-gate-upd");
        tool.call(GoalArgs {
            objective: Some("Ship".into()),
            pursuit_max_iterations: Some(3),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let set = tool
            .call(GoalArgs {
                gate_command: Some("cargo test".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(set.message.contains("per-goal command set"));
        // Empty/whitespace string clears the gate.
        let cleared = tool
            .call(GoalArgs {
                gate_command: Some("   ".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            !cleared.message.contains("per-goal command set"),
            "gate cleared: {}",
            cleared.message
        );
    }

    #[tokio::test]
    async fn update_iterations_are_capped() {
        let (tool, _d) = tool_with_session("sess-upd-cap");
        tool.call(GoalArgs {
            objective: Some("x".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                pursuit_max_iterations: Some(1_000_000),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(out
            .message
            .contains(&format!("0/{MAX_PURSUIT_ITERATIONS} iterations used")));
    }

    /// Re-activating a blocked goal with no explicit note clears the stale
    /// "blocked" reason so the prompt does not contradict the active status.
    #[tokio::test]
    async fn update_to_active_clears_stale_block_note() {
        let (tool, _d) = tool_with_session("sess-note");
        tool.call(GoalArgs {
            objective: Some("y".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        tool.call(GoalArgs {
            status: Some(GoalStatus::Blocked),
            note: Some("reached iteration cap".into()),
            ..args(GoalAction::Update)
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                status: Some(GoalStatus::Active),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            !out.message.contains("reached iteration cap"),
            "stale note kept: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn wait_minutes_parks_and_status_active_unparks() {
        let (tool, _d) = tool_with_session("sess-wait-tool");
        tool.call(GoalArgs {
            objective: Some("long haul".into()),
            pursuit_max_iterations: Some(5),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let out = tool
            .call(GoalArgs {
                wait_minutes: Some(20),
                note: Some("provider cooldown".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(
            out.message.contains("waiting: parked"),
            "got: {}",
            out.message
        );
        assert!(
            out.message.contains("provider cooldown"),
            "got: {}",
            out.message
        );

        // Explicit re-activate un-parks.
        let out = tool
            .call(GoalArgs {
                status: Some(GoalStatus::Active),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            !out.message.contains("waiting: parked"),
            "got: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn wait_rejected_on_interactive_goal_and_on_set() {
        let (tool, _d) = tool_with_session("sess-wait-rej");
        // Born parked → rejected at set.
        let err = tool
            .call(GoalArgs {
                objective: Some("x".into()),
                wait_minutes: Some(5),
                ..args(GoalAction::Set)
            })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("update"));
        // Passive (interactive) goal cannot park.
        tool.call(GoalArgs {
            objective: Some("x".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let err = tool
            .call(GoalArgs {
                wait_minutes: Some(5),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("autonomous"), "got: {err}");
    }

    #[tokio::test]
    async fn wait_args_are_mutually_exclusive_and_nonzero() {
        let (tool, _d) = tool_with_session("sess-wait-both");
        tool.call(GoalArgs {
            objective: Some("x".into()),
            pursuit_max_iterations: Some(3),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let err = tool
            .call(GoalArgs {
                wait_minutes: Some(5),
                wait_for_task: Some("t1".into()),
                session: None,
                ..args(GoalAction::Update)
            })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("not both"), "got: {err}");
        let err = tool
            .call(GoalArgs {
                wait_minutes: Some(0),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("at least 1"), "got: {err}");
    }

    #[tokio::test]
    async fn wait_is_refused_on_a_goal_this_update_leaves_non_active() {
        let (tool, _d) = tool_with_session("s-wait-blocked");
        let mut set = args(GoalAction::Set);
        set.objective = Some("obj".into());
        set.pursuit_max_iterations = Some(5);
        tool.call(set).await.unwrap();
        // Self-report blocked, then try to park it. `wait_parked` requires an
        // Active goal, so the barrier would never wake and `get` would advertise
        // a park that can never end.
        let mut block = args(GoalAction::Update);
        block.status = Some(GoalStatus::Blocked);
        tool.call(block).await.unwrap();

        let mut park = args(GoalAction::Update);
        park.wait_minutes = Some(30);
        let err = tool
            .call(park)
            .await
            .expect_err("parking a non-active goal must be refused, not silently inert");
        assert!(err.to_string().contains("active"), "got: {err}");
    }

    #[tokio::test]
    async fn wait_is_allowed_when_the_same_update_makes_the_goal_autonomous() {
        let (tool, _d) = tool_with_session("s-wait-promote");
        let mut set = args(GoalAction::Set);
        set.objective = Some("obj".into());
        tool.call(set).await.unwrap(); // interactive (Passive) goal

        // One call that both arms autonomous pursuit and parks it. Validating
        // against the PRE-update goal rejected this as "not autonomous" even
        // though this very call makes it autonomous.
        let mut promote = args(GoalAction::Update);
        promote.pursuit_max_iterations = Some(5);
        promote.wait_minutes = Some(30);
        let out = tool.call(promote).await.unwrap();
        assert!(out.success, "got: {}", out.message);
        assert!(out.message.contains("parked"), "got: {}", out.message);
    }

    #[tokio::test]
    async fn with_planner_provider_builds_and_still_sets_goal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new("sess-planner".to_string()));
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(crate::providers::MockProvider::new("not json"));
        let tool = GoalTool::new(store)
            .with_session_key_handle(Some(handle))
            .with_planner_provider(Some(provider));
        // Provider present but unparseable → planner self-fails → goal Set still OK.
        let out = tool
            .call(GoalArgs {
                objective: Some("Provider-present goal".into()),
                ..args(GoalAction::Set)
            })
            .await
            .unwrap();
        assert!(out.success);
    }

    /// With a planner provider that returns a concrete Strategy, goal `set` mints
    /// and stores it under goal_key(session). A second `set` with a *changed*
    /// objective invalidates the stale row (F4) and the planner re-fires; the
    /// constant mock returns identical guardrails, so the re-planned row matches.
    /// (Pure fire-once skip on an *unchanged* objective — no re-plan — is covered
    /// by `set_with_same_objective_keeps_strategy`.)
    #[tokio::test]
    async fn goal_set_mints_strategy_and_replans_on_changed_objective() {
        use crate::strategy::{goal_key, StrategyStore};
        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(crate::sync_primitives::Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let handle = Arc::new(RwLock::new("sess-fire".to_string()));
        let json = r#"{"objective":"o","approach":"a","phases":["p"],
            "guardrails":["do not touch the cache layer"],"success_criteria":"done"}"#;
        let provider: Arc<dyn crate::providers::AiProvider> =
            Arc::new(crate::providers::MockProvider::new(json));
        let tool = GoalTool::new(store)
            .with_session_key_handle(Some(handle))
            .with_planner_provider(Some(provider));

        tool.call(GoalArgs {
            objective: Some("First obj".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let stored = crate::strategy::global()
            .unwrap()
            .get(&goal_key("sess-fire"))
            .unwrap()
            .expect("a Strategy was minted");
        assert_eq!(
            stored.guardrails,
            vec!["do not touch the cache layer".to_string()]
        );

        // Re-set with a CHANGED objective: F4 invalidates the stale row, then the
        // planner re-fires; the constant mock yields the same guardrails.
        tool.call(GoalArgs {
            objective: Some("Second obj".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let after = crate::strategy::global()
            .unwrap()
            .get(&goal_key("sess-fire"))
            .unwrap()
            .unwrap();
        assert_eq!(
            after.guardrails, stored.guardrails,
            "re-planned row carries the mock's guardrails"
        );
    }

    /// Provider = None → goal `set` still succeeds and stores NO Strategy.
    #[tokio::test]
    async fn goal_set_with_no_provider_succeeds_without_strategy() {
        use crate::strategy::{goal_key, StrategyStore};
        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(crate::sync_primitives::Arc::new(
            StrategyStore::open(&sdir.path().join("s2.db")).unwrap(),
        ));

        let (tool, _d) = tool_with_session("sess-noprov");
        let out = tool
            .call(GoalArgs {
                objective: Some("Plain goal".into()),
                ..args(GoalAction::Set)
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(
            crate::strategy::global()
                .unwrap()
                .get(&goal_key("sess-noprov"))
                .unwrap()
                .is_none(),
            "no provider => no Strategy"
        );
    }

    #[tokio::test]
    async fn clear_deletes_goal_keyed_strategy_but_not_loop_keyed() {
        use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        // set_global_for_test is OnceCell-once; another test in this binary may
        // have set the global first. Seed + assert through the ACTUAL global the
        // tool's clear operates on (the unique session key avoids collision).
        let sstore = crate::strategy::global().expect("strategy global set");

        let concrete = Strategy {
            objective: "o".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["do not touch unrelated code".into()],
            success_criteria: "ok".into(),
            goal_id: None,
        };
        sstore
            .put(&goal_key("sess-clear-strat"), &concrete)
            .unwrap();
        sstore
            .put(&loop_key("sess-clear-strat"), &concrete)
            .unwrap();

        let (tool, _d) = tool_with_session("sess-clear-strat");
        tool.call(GoalArgs {
            objective: Some("x".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        tool.call(GoalArgs {
            ..args(GoalAction::Clear)
        })
        .await
        .unwrap();

        // Goal Clear removes the goal-keyed strategy...
        assert!(sstore.get(&goal_key("sess-clear-strat")).unwrap().is_none());
        // ...but leaves a co-existing loop-keyed strategy untouched.
        assert!(sstore.get(&loop_key("sess-clear-strat")).unwrap().is_some());
    }

    /// Hand-seed a concrete goal-keyed strategy for `session` through the global
    /// store the tool operates on. Shared by the terminal-clear tests below.
    fn seed_goal_strategy(session: &str) {
        use crate::strategy::{goal_key, Strategy};
        let concrete = Strategy {
            objective: "o".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["stay in scope".into()],
            success_criteria: "ok".into(),
            goal_id: None,
        };
        crate::strategy::global()
            .expect("strategy global set")
            .put(&goal_key(session), &concrete)
            .unwrap();
    }

    #[tokio::test]
    async fn update_to_blocked_clears_welded_strategy() {
        use crate::strategy::{goal_key, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        let sstore = crate::strategy::global().expect("strategy global set");

        let (tool, _d) = tool_with_session("sess-upd-blocked");
        tool.call(GoalArgs {
            objective: Some("x".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        seed_goal_strategy("sess-upd-blocked");

        // Model self-reports Blocked → the tool owns this dormant end (the
        // continuation hook is a no-op for a self-Blocked goal), so it clears
        // the welded plan.
        tool.call(GoalArgs {
            status: Some(GoalStatus::Blocked),
            ..args(GoalAction::Update)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-upd-blocked")).unwrap().is_none(),
            "update->blocked must clear the welded strategy"
        );
    }

    #[tokio::test]
    async fn update_to_complete_passive_clears_welded_strategy() {
        use crate::strategy::{goal_key, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        let sstore = crate::strategy::global().expect("strategy global set");

        let (tool, _d) = tool_with_session("sess-upd-passive");
        // No pursuit_max_iterations → Passive pursuit (interactive goal).
        tool.call(GoalArgs {
            objective: Some("x".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        seed_goal_strategy("sess-upd-passive");

        // Passive Complete is terminal at the tool (the hook never acts on a
        // Passive goal), so the tool clears the weld.
        tool.call(GoalArgs {
            status: Some(GoalStatus::Complete),
            ..args(GoalAction::Update)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-upd-passive")).unwrap().is_none(),
            "passive update->complete must clear the welded strategy"
        );
    }

    #[tokio::test]
    async fn update_to_complete_active_keeps_welded_strategy_for_gate_arbitration() {
        use crate::strategy::{goal_key, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        let sstore = crate::strategy::global().expect("strategy global set");

        let (tool, _d) = tool_with_session("sess-upd-active");
        // Active pursuit → the model's Complete is a *claim* the gateway gate
        // arbitrates; the tool must NOT clear the weld, so a gate veto can
        // reopen the goal WITH its plan intact.
        tool.call(GoalArgs {
            objective: Some("x".into()),
            pursuit_max_iterations: Some(5),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        seed_goal_strategy("sess-upd-active");

        tool.call(GoalArgs {
            status: Some(GoalStatus::Complete),
            ..args(GoalAction::Update)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-upd-active")).unwrap().is_some(),
            "active-pursuit update->complete must keep the weld (gate arbitrates)"
        );
    }

    #[tokio::test]
    async fn set_with_changed_objective_invalidates_stale_strategy() {
        use crate::strategy::{goal_key, Strategy, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        // set_global_for_test is OnceCell-once; another test in this binary may
        // have set the global first. Seed + assert through the ACTUAL global the
        // tool's Set operates on (the unique session key avoids collision).
        let sstore = crate::strategy::global().expect("strategy global set");

        let (tool, _d) = tool_with_session("sess-objchg");
        // First Set — no planner provider, so no strategy is auto-minted.
        tool.call(GoalArgs {
            objective: Some("Migrate auth to v2".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        // Confirm nothing was seeded automatically.
        assert!(sstore.get(&goal_key("sess-objchg")).unwrap().is_none());
        // Retrieve the first goal's id, then hand-seed a strategy that
        // cross-references it (simulating a previously-welded plan).
        let first_goal_id = tool.store.get("sess-objchg").unwrap().unwrap().id.clone();
        let strat = Strategy {
            objective: "Migrate auth to v2".into(),
            approach: "incremental".into(),
            phases: vec![],
            guardrails: vec!["do not break existing sessions".into()],
            success_criteria: "tests green".into(),
            goal_id: Some(first_goal_id),
        };
        sstore.put(&goal_key("sess-objchg"), &strat).unwrap();

        // Re-set with a DIFFERENT objective -> new goal.id -> stale strategy gone.
        tool.call(GoalArgs {
            objective: Some("Rewrite the billing pipeline".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-objchg")).unwrap().is_none(),
            "changed objective must invalidate the stale welded strategy"
        );
    }

    #[tokio::test]
    async fn set_with_same_objective_keeps_strategy() {
        use crate::strategy::{goal_key, Strategy, StrategyStore};
        use crate::sync_primitives::Arc;

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        // set_global_for_test is OnceCell-once; another test in this binary may
        // have set the global first. Seed + assert through the ACTUAL global.
        let sstore = crate::strategy::global().expect("strategy global set");

        let (tool, _d) = tool_with_session("sess-same");
        tool.call(GoalArgs {
            objective: Some("Keep me".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        let gid = tool.store.get("sess-same").unwrap().unwrap().id.clone();
        let strat = Strategy {
            objective: "Keep me".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["one concrete guardrail".into()],
            success_criteria: "ok".into(),
            goal_id: Some(gid),
        };
        sstore.put(&goal_key("sess-same"), &strat).unwrap();

        // Same objective => same goal.id => strategy preserved.
        tool.call(GoalArgs {
            objective: Some("Keep me".into()),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
        assert!(
            sstore.get(&goal_key("sess-same")).unwrap().is_some(),
            "unchanged objective must keep the welded strategy"
        );
    }

    #[tokio::test]
    async fn list_with_no_goals_is_graceful() {
        let (tool, _d) = tool_with_session("sess-list-empty");
        let out = tool.call(args(GoalAction::List)).await.unwrap();
        assert!(out.success);
        assert!(
            out.message.to_lowercase().contains("no standing goals"),
            "got: {}",
            out.message
        );
    }

    /// Two sessions sharing one store: `list` from session A must enumerate B's
    /// goal too (cross-session R6 one-core-many-shells) and flag A's own as "(this session)".
    #[tokio::test]
    async fn list_enumerates_all_sessions_and_flags_current() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let tool_a = GoalTool::new(store.clone())
            .with_session_key_handle(Some(Arc::new(RwLock::new("sess-A".to_string()))));
        let tool_b = GoalTool::new(store.clone())
            .with_session_key_handle(Some(Arc::new(RwLock::new("sess-B".to_string()))));
        tool_a
            .call(GoalArgs {
                objective: Some("Goal in A".into()),
                ..args(GoalAction::Set)
            })
            .await
            .unwrap();
        tool_b
            .call(GoalArgs {
                objective: Some("Goal in B".into()),
                ..args(GoalAction::Set)
            })
            .await
            .unwrap();

        let out = tool_a.call(args(GoalAction::List)).await.unwrap();
        assert!(
            out.message.contains("Standing goals (2)"),
            "got: {}",
            out.message
        );
        assert!(out.message.contains("Goal in A"));
        assert!(
            out.message.contains("Goal in B"),
            "cross-session goal must be visible: {}",
            out.message
        );
        let a_line = out
            .message
            .lines()
            .find(|l| l.contains("Goal in A"))
            .unwrap();
        assert!(a_line.contains("(this session)"), "A flagged: {a_line}");
        let b_line = out
            .message
            .lines()
            .find(|l| l.contains("Goal in B"))
            .unwrap();
        assert!(
            !b_line.contains("(this session)"),
            "B not flagged: {b_line}"
        );
    }

    // ---- boundary validation + honest reporting ---------------------------

    #[tokio::test]
    async fn set_rejects_a_born_dead_iteration_cap() {
        let (tool, _d) = tool_with_session("s");
        let err = tool
            .call(GoalArgs {
                action: GoalAction::Set,
                objective: Some("do it".into()),
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: Some(0),
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .expect_err("a 0-iteration pursuit is exhausted before its first step");
        assert!(err.to_string().contains("at least 1"), "got: {err}");
        // …and nothing was persisted.
        assert!(tool.store.get("s").unwrap().is_none());
    }

    #[tokio::test]
    async fn set_rejects_a_zero_timeout() {
        let (tool, _d) = tool_with_session("s");
        let err = tool
            .call(GoalArgs {
                action: GoalAction::Set,
                objective: Some("do it".into()),
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: Some(5),
                gate_command: None,
                lesson: None,
                timeout_minutes: Some(0),
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .expect_err("a 0-minute deadline is already in the past");
        assert!(err.to_string().contains("at least 1"), "got: {err}");
    }

    #[tokio::test]
    async fn set_rejects_a_gate_command_the_gate_runner_cannot_execute() {
        let (tool, _d) = tool_with_session("s");
        let err = tool
            .call(GoalArgs {
                action: GoalAction::Set,
                objective: Some("ship it".into()),
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: Some(5),
                gate_command: Some("cargo build && cargo test".into()),
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .expect_err("shell metacharacters make the gate unrunnable");
        assert!(err.to_string().contains("metacharacters"), "got: {err}");
        assert!(
            tool.store.get("s").unwrap().is_none(),
            "a goal must not be created with a gate that can never render a verdict"
        );
    }

    #[tokio::test]
    async fn reactivating_a_gate_passed_goal_rearms_the_gate() {
        let (tool, _d) = tool_with_session("s");
        let g = Goal::new("s", "obj", 0, 1)
            .with_pursuit(PursuitMode::Active { max_iterations: 5 })
            .with_status(GoalStatus::Complete, 1)
            .with_gate_outcome(crate::goal::GateOutcome::Passed, 1);
        tool.store.put(&g).unwrap();

        let out = tool
            .call(GoalArgs {
                action: GoalAction::Update,
                objective: None,
                status: Some(GoalStatus::Active),
                note: None,
                token_budget: None,
                pursuit_max_iterations: Some(10),
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let stored = tool.store.get("s").unwrap().unwrap();
        assert_eq!(stored.status, GoalStatus::Active);
        assert_eq!(
            stored.gate_outcome,
            crate::goal::GateOutcome::Unchecked,
            "a resumed goal's next completion claim must face the gate again"
        );
    }

    #[tokio::test]
    async fn reactivating_without_lifting_the_binding_cap_is_reported_honestly() {
        let (tool, _d) = tool_with_session("s");
        let mut g = Goal::new("s", "obj", 0, 1)
            .with_pursuit(PursuitMode::Active { max_iterations: 3 })
            .with_status(GoalStatus::Blocked, 1);
        g.continuations_used = 3;
        tool.store.put(&g).unwrap();

        let out = tool
            .call(GoalArgs {
                action: GoalAction::Update,
                objective: None,
                status: Some(GoalStatus::Active),
                note: None,
                token_budget: None,
                pursuit_max_iterations: None, // the half-resume
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(
            !out.success,
            "the hook would re-block this goal on the very next run: {}",
            out.message
        );
        assert!(
            out.message.contains("pursuit_max_iterations"),
            "{}",
            out.message
        );

        // Lifting the cap in the same call is the real resume, and it succeeds.
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Update,
                objective: None,
                status: Some(GoalStatus::Active),
                note: None,
                token_budget: None,
                pursuit_max_iterations: Some(8),
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let stored = tool.store.get("s").unwrap().unwrap();
        assert_eq!(stored.continuations_used, 3, "progress is kept");
    }

    #[tokio::test]
    async fn clear_says_so_when_there_was_nothing_to_clear() {
        let (tool, _d) = tool_with_session("s");
        let out = tool
            .call(GoalArgs {
                action: GoalAction::Clear,
                objective: None,
                status: None,
                note: None,
                token_budget: None,
                pursuit_max_iterations: None,
                gate_command: None,
                lesson: None,
                timeout_minutes: None,
                wait_minutes: None,
                wait_for_task: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.message.contains("nothing to clear"), "{}", out.message);
    }

    // ---- cross-session control (loop parity) --------------------------------

    /// Two session-bound tools over ONE store — the cross-session shape the
    /// single-session helper above cannot express.
    fn two_sessions(a: &str, b: &str) -> (GoalTool, GoalTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(GoalStore::open(&dir.path().join("g.db")).unwrap());
        let bind = |s: &str| {
            GoalTool::new(store.clone())
                .with_session_key_handle(Some(Arc::new(RwLock::new(s.to_string()))))
        };
        (bind(a), bind(b), dir)
    }

    async fn set_autonomous(tool: &GoalTool, objective: &str) {
        tool.call(GoalArgs {
            objective: Some(objective.into()),
            pursuit_max_iterations: Some(5),
            ..args(GoalAction::Set)
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_prints_the_session_key_that_remote_verbs_take() {
        let (here, there, _d) = two_sessions("here", "remote-sess");
        set_autonomous(&there, "watch the fleet").await;
        let out = here.call(args(GoalAction::List)).await.unwrap();
        assert!(
            out.message.contains("(session 'remote-sess')"),
            "a goal listed without an addressable key stays unstoppable: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn cross_session_update_pauses_and_clear_removes() {
        let (here, there, _d) = two_sessions("here", "remote-sess");
        set_autonomous(&there, "watch the fleet").await;

        let out = here
            .call(GoalArgs {
                status: Some(GoalStatus::Paused),
                session: Some("remote-sess".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        let remote = there.call(args(GoalAction::Get)).await.unwrap();
        assert!(
            remote.message.contains("status=paused"),
            "{}",
            remote.message
        );
        // Pausing an already-paused goal reports the truth rather than lying.
        let out = here
            .call(GoalArgs {
                status: Some(GoalStatus::Paused),
                session: Some("remote-sess".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            !out.success && out.message.contains("status=paused"),
            "{}",
            out.message
        );

        // `get` and `clear` reach across too, and say where they acted.
        let out = here
            .call(GoalArgs {
                session: Some("remote-sess".into()),
                ..args(GoalAction::Get)
            })
            .await
            .unwrap();
        assert!(out.message.starts_with("remote-sess:"), "{}", out.message);
        let out = here
            .call(GoalArgs {
                session: Some("remote-sess".into()),
                ..args(GoalAction::Clear)
            })
            .await
            .unwrap();
        assert!(
            out.success && out.message.contains("in session 'remote-sess'"),
            "{}",
            out.message
        );
        assert!(there
            .call(args(GoalAction::Get))
            .await
            .unwrap()
            .message
            .contains("No standing goal"));
    }

    #[tokio::test]
    async fn cross_session_update_is_quiet_only() {
        let (here, there, _d) = two_sessions("here", "remote-sess");
        set_autonomous(&there, "watch the fleet").await;

        // Resuming remotely would leave an Active goal nothing can drive.
        let err = here
            .call(GoalArgs {
                status: Some(GoalStatus::Active),
                session: Some("remote-sess".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("may only pause"), "{err}");

        // A field smuggled alongside the pause is refused, not silently dropped.
        let err = here
            .call(GoalArgs {
                status: Some(GoalStatus::Paused),
                pursuit_max_iterations: Some(40),
                session: Some("remote-sess".into()),
                ..args(GoalAction::Update)
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("pursuit_max_iterations"), "{err}");

        // `set` never reaches across at all.
        let err = here
            .call(GoalArgs {
                objective: Some("hijack".into()),
                session: Some("remote-sess".into()),
                ..args(GoalAction::Set)
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("own session"), "{err}");

        // …and nothing moved.
        assert!(there
            .call(args(GoalAction::Get))
            .await
            .unwrap()
            .message
            .contains("status=active"));
    }

    #[tokio::test]
    async fn cross_session_control_requires_operator() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let (here, there, _d) = two_sessions("here", "remote-sess");
        set_autonomous(&there, "watch the fleet").await;
        let guest = TurnContext {
            session_key: SessionKey::main("a"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "c".to_string(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        };

        let remote_clear = GoalArgs {
            session: Some("remote-sess".into()),
            ..args(GoalAction::Clear)
        };
        let err = TURN_CONTEXT
            .scope(guest.clone(), here.call(remote_clear))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("operator"), "{err}");
        let err = TURN_CONTEXT
            .scope(guest, here.call(args(GoalAction::PauseAll)))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("operator"), "{err}");
        assert!(there
            .call(args(GoalAction::Get))
            .await
            .unwrap()
            .message
            .contains("status=active"));
    }

    #[tokio::test]
    async fn pause_all_holds_every_pursuit_without_destroying_it() {
        let (a, b, _d) = two_sessions("sess-a", "sess-b");
        set_autonomous(&a, "objective A").await;
        set_autonomous(&b, "objective B").await;

        let out = a.call(args(GoalAction::PauseAll)).await.unwrap();
        assert!(out.success && out.message.contains('2'), "{}", out.message);
        for tool in [&a, &b] {
            let got = tool.call(args(GoalAction::Get)).await.unwrap();
            assert!(got.message.contains("status=paused"), "{}", got.message);
            assert!(
                got.message.contains("objective "),
                "the objective must survive a pause: {}",
                got.message
            );
        }
        // Each owner session resumes its own.
        a.call(GoalArgs {
            status: Some(GoalStatus::Active),
            ..args(GoalAction::Update)
        })
        .await
        .unwrap();
        assert!(a
            .call(args(GoalAction::Get))
            .await
            .unwrap()
            .message
            .contains("status=active"));
        // Nothing left actively pursued in b alone → the second sweep is a no-op
        // for a but still catches b.
        let out = b.call(args(GoalAction::PauseAll)).await.unwrap();
        assert!(out.message.contains('1'), "{}", out.message);
    }
}
