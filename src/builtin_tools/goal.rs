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
    /// Optional soft token budget — for `set`.
    pub token_budget: Option<u64>,
    /// If present on `set`, enables autonomous continuation (opt-in,
    /// default-off) bounded by this many Think→Act iterations.
    pub pursuit_max_iterations: Option<u32>,
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
}

impl GoalTool {
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self { store, session_key: None }
    }

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
        let mut s = format!("Standing goal: {}\nstatus={:?}", goal.objective, goal.status);
        if let Some(b) = goal.token_budget {
            s.push_str(&format!(", token_budget={b}"));
        }
        if let PursuitMode::Active { max_iterations } = goal.pursuit {
            s.push_str(&format!(", pursuit=active(max_iterations={max_iterations})"));
        }
        if let Some(note) = goal.note.as_deref() {
            if !note.is_empty() {
                s.push_str(&format!("\nnote: {note}"));
            }
        }
        s
    }
}

/// Hard ceiling on autonomous continuations a single goal may request,
/// regardless of what the caller asks for (R5 不打扰 backstop).
const MAX_PURSUIT_ITERATIONS: u32 = 50;

/// Wall-clock milliseconds since the Unix epoch; 0 if the clock is before
/// the epoch (never in practice).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[async_trait]
impl AlephTool for GoalTool {
    const NAME: &'static str = "goal";
    const DESCRIPTION: &'static str =
        "Manage a STANDING GOAL — a persistent objective you keep pursuing \
         across turns until it is achieved. Create one with action='set' ONLY \
         when the user explicitly asks you to pursue a standing objective \
         (optionally with a token_budget, and pursuit_max_iterations to let \
         the system continue autonomously). Read it with action='get'. When \
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
            "goal(action='get')".into(),
            "goal(action='update', status='complete', note='all endpoints migrated and tests green')".into(),
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
                // per-session token counter at call time, so the budget is a
                // soft guardrail surfaced to the model, not measured from the
                // real turn baseline. Do NOT "fix" this to a nonzero value
                // without threading the live token count in — the structural
                // backstop for autonomous runs is pursuit_max_iterations.
                let mut goal = Goal::new(&session, objective, 0, now)
                    .with_budget(args.token_budget)
                    .with_note(args.note.clone(), now);
                if let Some(requested) = args.pursuit_max_iterations {
                    // Hard cap autonomous continuations (R5 不打扰): an
                    // unbounded value would let a single session self-run for
                    // days. The clamped value is surfaced to the model via
                    // `render` so it sees the effective cap.
                    let max_iterations = requested.min(MAX_PURSUIT_ITERATIONS);
                    goal = goal.with_pursuit(PursuitMode::Active { max_iterations });
                }
                self.store.put(&goal)?;
                Ok(GoalOutput { success: true, message: format!("Set. {}", Self::render(&goal)) })
            }
            GoalAction::Get => match self.store.get(&session)? {
                Some(goal) => Ok(GoalOutput { success: true, message: Self::render(&goal) }),
                None => Ok(GoalOutput {
                    success: true,
                    message: "No standing goal set for this session.".to_string(),
                }),
            },
            GoalAction::Update => {
                let mut goal = self.store.get(&session)?.ok_or_else(|| {
                    AlephError::tool("no standing goal to update".to_string())
                })?;
                if let Some(status) = args.status {
                    goal = goal.with_status(status, now);
                }
                if args.note.is_some() {
                    goal = goal.with_note(args.note.clone(), now);
                }
                self.store.put(&goal)?;
                Ok(GoalOutput {
                    success: true,
                    message: format!("Updated. {}", Self::render(&goal)),
                })
            }
            GoalAction::Clear => {
                self.store.delete(&session)?;
                Ok(GoalOutput { success: true, message: "Standing goal cleared.".to_string() })
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
        (GoalTool::new(store).with_session_key_handle(Some(handle)), dir)
    }

    #[tokio::test]
    async fn set_then_get_returns_objective() {
        let (tool, _d) = tool_with_session("sess-A");
        tool.call(GoalArgs {
            action: GoalAction::Set,
            objective: Some("Ship the goal feature".into()),
            status: None, note: None, token_budget: Some(5000),
            pursuit_max_iterations: None,
        }).await.unwrap();

        let out = tool.call(GoalArgs {
            action: GoalAction::Get, objective: None, status: None, note: None,
            token_budget: None, pursuit_max_iterations: None,
        }).await.unwrap();
        assert!(out.success);
        assert!(out.message.contains("Ship the goal feature"));
    }

    #[tokio::test]
    async fn update_complete_marks_status() {
        let (tool, _d) = tool_with_session("sess-B");
        tool.call(GoalArgs { action: GoalAction::Set, objective: Some("x".into()),
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        let out = tool.call(GoalArgs { action: GoalAction::Update, objective: None,
            status: Some(GoalStatus::Complete), note: Some("done".into()),
            token_budget: None, pursuit_max_iterations: None }).await.unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("complete"));
    }

    #[tokio::test]
    async fn pursuit_iterations_are_capped() {
        let (tool, _d) = tool_with_session("sess-cap");
        tool.call(GoalArgs { action: GoalAction::Set, objective: Some("z".into()),
            status: None, note: None, token_budget: None,
            pursuit_max_iterations: Some(1_000_000) }).await.unwrap();
        let out = tool.call(GoalArgs { action: GoalAction::Get, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        // Effective cap surfaced to the model, not the requested 1,000,000.
        assert!(out.message.contains(&format!("max_iterations={MAX_PURSUIT_ITERATIONS}")));
    }

    #[tokio::test]
    async fn get_with_no_goal_is_graceful() {
        let (tool, _d) = tool_with_session("sess-empty");
        let out = tool.call(GoalArgs { action: GoalAction::Get, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        assert!(out.success);
        assert!(out.message.to_lowercase().contains("no standing goal"));
    }

    #[tokio::test]
    async fn set_requires_objective() {
        let (tool, _d) = tool_with_session("sess-C");
        let err = tool.call(GoalArgs { action: GoalAction::Set, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await;
        assert!(err.is_err(), "set without objective must error");
    }

    #[tokio::test]
    async fn clear_removes_goal() {
        let (tool, _d) = tool_with_session("sess-D");
        tool.call(GoalArgs { action: GoalAction::Set, objective: Some("y".into()),
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        tool.call(GoalArgs { action: GoalAction::Clear, objective: None, status: None,
            note: None, token_budget: None, pursuit_max_iterations: None }).await.unwrap();
        let out = tool.call(GoalArgs { action: GoalAction::Get, objective: None,
            status: None, note: None, token_budget: None, pursuit_max_iterations: None })
            .await.unwrap();
        assert!(out.message.to_lowercase().contains("no standing goal"));
    }
}
