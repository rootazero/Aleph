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
}

impl GoalTool {
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self {
            store,
            session_key: None,
            planner_provider: None,
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

    async fn session(&self) -> String {
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    fn render(goal: &Goal) -> String {
        let mut s = format!(
            "Standing goal: {}\nstatus={:?}",
            goal.objective, goal.status
        );
        if let Some(b) = goal.token_budget {
            s.push_str(&format!(", token_budget={b}"));
        }
        if let PursuitMode::Active { max_iterations } = goal.pursuit {
            s.push_str(&format!(
                ", pursuit=active({}/{max_iterations} iterations used)",
                goal.continuations_used
            ));
        }
        if goal.deadline_ms.is_some() {
            s.push_str(", deadline set");
        }
        if let Some(note) = goal.note.as_deref() {
            if !note.is_empty() {
                s.push_str(&format!("\nnote: {note}"));
            }
        }
        if goal.gate_command.is_some() {
            s.push_str("\ngate: per-goal command set");
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
}

/// Hard ceiling on autonomous continuations a single goal may request,
/// regardless of what the caller asks for (R5 不打扰 backstop).
const MAX_PURSUIT_ITERATIONS: u32 = 50;

/// Clamp a requested autonomous-iteration cap to the hard ceiling
/// (`MAX_PURSUIT_ITERATIONS`, R5 不打扰 backstop). Shared by `set` and the
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
         Read it with action='get'. When \
         you have achieved the objective, self-report with action='update', \
         status='complete'; if you are stuck and need the user, use \
         status='blocked'. Use status='paused'/'active' only when the user \
         explicitly asks to pause or resume. Remove it with action='clear'. \
         The goal is re-surfaced into your prompt every turn while active.";

    type Args = GoalArgs;
    type Output = GoalOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "goal(action='set', objective='Migrate the auth module to the new API', token_budget=50000)".into(),
            "goal(action='set', objective='Triage failing CI', pursuit_max_iterations=10, timeout_minutes=30)".into(),
            "goal(action='get')".into(),
            "goal(action='update', status='complete', note='all endpoints migrated and tests green')".into(),
            "goal(action='update', lesson='remember to run db migrations before tests')".into(),
            "goal(action='update', status='active', pursuit_max_iterations=15)".into(),
            "goal(action='clear')".into(),
        ])
    }

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
                let objective = args.objective.as_deref().ok_or_else(|| {
                    AlephError::tool("goal 'set' requires 'objective'".to_string())
                })?;
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
                    .with_note(args.note.clone(), now);
                if let Some(requested) = args.pursuit_max_iterations {
                    // Hard cap autonomous continuations (R5 不打扰): an
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
                self.store.put(&goal)?;
                Ok(GoalOutput {
                    success: true,
                    message: format!("Set. {}", Self::render(&goal)),
                })
            }
            GoalAction::Get => match self.store.get(&session)? {
                Some(goal) => Ok(GoalOutput {
                    success: true,
                    message: Self::render(&goal),
                }),
                None => Ok(GoalOutput {
                    success: true,
                    message: "No standing goal set for this session.".to_string(),
                }),
            },
            GoalAction::Update => {
                let mut goal = self
                    .store
                    .get(&session)?
                    .ok_or_else(|| AlephError::tool("no standing goal to update".to_string()))?;
                let prev_status = goal.status;
                if let Some(status) = args.status {
                    goal = goal.with_status(status, now);
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
                    // Empty string clears the per-goal gate; anything else sets it.
                    let next = if cmd.trim().is_empty() {
                        None
                    } else {
                        Some(cmd)
                    };
                    goal = goal.with_gate_command(next);
                }
                if args.note.is_some() {
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
                    goal = goal.with_lesson_appended(lesson, now);
                }
                self.store.put(&goal)?;
                Ok(GoalOutput {
                    success: true,
                    message: format!("Updated. {}", Self::render(&goal)),
                })
            }
            GoalAction::Clear => {
                self.store.delete(&session)?;
                Ok(GoalOutput {
                    success: true,
                    message: "Standing goal cleared.".to_string(),
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
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.contains("Ship the goal feature"));
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
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("complete"));
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
            })
            .await
            .unwrap();
        assert!(out.message.to_lowercase().contains("no standing goal"));
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
            })
            .await
            .unwrap();
        assert!(out.message.contains("deadline set"));
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
                ..args(GoalAction::Update)
            })
            .await
            .unwrap();
        assert!(
            out.message.contains("token_budget=80000"),
            "got: {}",
            out.message
        );
        assert!(out.message.contains("deadline set"), "got: {}", out.message);
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
}
