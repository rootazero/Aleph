//! `strategy` builtin tool (R8): the LLM revises or reads the welded Strategy
//! for a long task. Sibling of `goal`/`loop` — but unlike them it does NOT
//! create or schedule anything; the Strategy is minted by the planner node
//! above the loop. This tool is the rare escape-hatch: a DUMB schema-validated
//! overwrite (`revise`) and a read (`show`).
//!
//! "High-friction" lives entirely in the DESCRIPTION discourse (R9: intelligence
//! in the prompt), NEVER as a Rust gate / counter / similarity score / classifier
//! (spec §8 non-goal: this tool must not evaluate the legitimacy of a revision).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::error::{AlephError, Result};
use crate::strategy::{goal_key, loop_key, session_key, Strategy, StrategyStore};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StrategyAction {
    /// Overwrite the in-force Strategy for this task. Reserve for genuine
    /// environment shock that invalidates the high-level approach.
    Revise,
    /// Read the current Strategy for this task.
    Show,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StrategyArgs {
    pub action: StrategyAction,
    /// Why the revision is warranted — required (non-empty) for `revise`.
    pub reason: Option<String>,
    /// The full replacement Strategy — required for `revise`. Must carry at
    /// least one concrete guardrail (a blank-guardrail object is rejected).
    pub new_strategy: Option<Strategy>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyOutput {
    pub success: bool,
    pub message: String,
}

#[derive(Clone)]
pub struct StrategyTool {
    store: Arc<StrategyStore>,
    session_key: Option<Arc<RwLock<String>>>,
    #[cfg(test)]
    test_session: Option<String>,
}

impl StrategyTool {
    #[must_use]
    pub fn new(store: Arc<StrategyStore>) -> Self {
        Self {
            store,
            session_key: None,
            #[cfg(test)]
            test_session: None,
        }
    }

    #[must_use]
    pub fn with_session_key_handle(mut self, handle: Option<Arc<RwLock<String>>>) -> Self {
        self.session_key = handle;
        self
    }

    #[cfg(test)]
    #[must_use]
    pub fn with_session_for_test(mut self, sess: &str) -> Self {
        self.test_session = Some(sess.to_string());
        self
    }

    async fn session(&self) -> String {
        #[cfg(test)]
        if let Some(s) = &self.test_session {
            return s.clone();
        }
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

    /// Resolve which composite key holds the in-force Strategy for this session.
    /// Goal precedence (mirrors `active_strategy`): a goal-keyed Strategy wins
    /// over a co-existing loop-keyed one. Returns the key to read/overwrite, or
    /// `None` if neither exists.
    fn resolve_key(&self, session: &str) -> std::result::Result<Option<String>, String> {
        let gk = goal_key(session);
        if self.store.get(&gk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(gk));
        }
        let lk = loop_key(session);
        if self.store.get(&lk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(lk));
        }
        // Naked-loop (plain interactive chat) strategy — lowest precedence so a
        // /goal or /loop strategy in a reused session always wins. Lets
        // `strategy show`/`revise` operate in a naked-loop session.
        let sk = session_key(session);
        if self.store.get(&sk).map_err(|e| e.to_string())?.is_some() {
            return Ok(Some(sk));
        }
        Ok(None)
    }

    /// Core dispatch — public so tests call it directly without the trait.
    pub async fn run(&self, args: StrategyArgs) -> std::result::Result<StrategyOutput, String> {
        let session = self.session().await;
        info!(session = %session, action = ?args.action, "strategy operation");
        match args.action {
            StrategyAction::Revise => self.revise(&session, args),
            StrategyAction::Show => self.show(&session),
        }
    }

    fn revise(
        &self,
        session: &str,
        args: StrategyArgs,
    ) -> std::result::Result<StrategyOutput, String> {
        // DUMB WRITE: schema validation only.
        let reason = args
            .reason
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| "revise requires a non-empty reason".to_string())?;
        let new_strategy = args
            .new_strategy
            .ok_or_else(|| "revise requires a new_strategy".to_string())?;
        // Reject a non-strategy (no concrete guardrail) — mirrors the planner's
        // self-gate so the welded prefix never carries noise.
        if new_strategy.is_empty() {
            return Err("new_strategy must carry at least one concrete guardrail".to_string());
        }
        // Overwrite the in-force Strategy. If none exists yet (a revise before
        // the planner ran), default to the goal key — the dominant flow.
        let key = self
            .resolve_key(session)?
            .unwrap_or_else(|| goal_key(session));
        self.store
            .put(&key, &new_strategy)
            .map_err(|e| e.to_string())?;
        info!(session = %session, reason = %reason, "strategy revised");
        Ok(StrategyOutput {
            success: true,
            message: "Strategy revised. The new high-level plan is welded into \
                 every following turn of this task."
                .to_string(),
        })
    }

    fn show(&self, session: &str) -> std::result::Result<StrategyOutput, String> {
        let Some(key) = self.resolve_key(session)? else {
            return Ok(StrategyOutput {
                success: false,
                message: "No strategy set for this task.".to_string(),
            });
        };
        match self.store.get(&key).map_err(|e| e.to_string())? {
            Some(s) => Ok(StrategyOutput {
                success: true,
                message: render_for_show(&s),
            }),
            None => Ok(StrategyOutput {
                success: false,
                message: "No strategy set for this task.".to_string(),
            }),
        }
    }
}

/// Human-readable single-object dump for `show`. Deterministic — no timestamps,
/// no HashMap iteration (fields are `Vec`/`String`).
fn render_for_show(s: &Strategy) -> String {
    let mut out = format!("objective: {}\napproach: {}", s.objective, s.approach);
    if !s.phases.is_empty() {
        out.push_str(&format!("\nphases: {}", s.phases.join(" -> ")));
    }
    if !s.guardrails.is_empty() {
        out.push_str("\nguardrails:");
        for g in &s.guardrails {
            out.push_str(&format!("\n  - {g}"));
        }
    }
    if !s.success_criteria.is_empty() {
        out.push_str(&format!("\nsuccess_criteria: {}", s.success_criteria));
    }
    out
}

#[async_trait]
impl AlephTool for StrategyTool {
    const NAME: &'static str = "strategy";
    const DESCRIPTION: &'static str =
        "Read or REVISE the high-level Strategy welded into this long task. A \
         Strategy is the map you drew before starting — objective, approach, \
         coarse phases, and a small set of concrete guardrails — and it rides \
         in your system prompt every turn so you do not drift. \
         action='show' reads it. \
         action='revise' OVERWRITES it (reason + new_strategy required) and is \
         HIGH-FRICTION BY DESIGN: default to HOLDING the Strategy. Revise ONLY \
         on a genuine ENVIRONMENT SHOCK that invalidates the high-level \
         approach itself (the chosen tool/library is gone, the objective was \
         misread, a hard external constraint appeared). Do NOT revise for \
         ordinary tactical changes — a different file to edit, a reordered \
         step, a new sub-task — those belong in your scratchpad, not here. A \
         revise costs a prompt-cache miss and resets the map the whole task \
         leans on; keep it rare. The new_strategy must carry at least one \
         concrete, observable guardrail or it is rejected.";

    type Args = StrategyArgs;
    type Output = StrategyOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session = self.session().await;
        if session.is_empty() {
            return Err(AlephError::tool(
                "strategy tool has no active session binding".to_string(),
            ));
        }
        self.run(args).await.map_err(AlephError::tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{goal_key, loop_key, session_key, Strategy, StrategyStore};
    use crate::sync_primitives::Arc;
    use tokio::sync::RwLock;

    fn concrete_strategy(objective: &str) -> Strategy {
        Strategy {
            objective: objective.to_string(),
            approach: "incremental, verify each step".to_string(),
            phases: vec!["understand".to_string(), "implement".to_string()],
            guardrails: vec!["do not refactor unrelated modules".to_string()],
            success_criteria: "cargo test green".to_string(),
            goal_id: None,
        }
    }

    fn tool_with_session(session: &str) -> (StrategyTool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("s.db")).unwrap());
        let handle = Arc::new(RwLock::new(session.to_string()));
        (
            StrategyTool::new(store).with_session_key_handle(Some(handle)),
            dir,
        )
    }

    #[tokio::test]
    async fn revise_rejects_empty_reason() {
        let (tool, _d) = tool_with_session("sess-empty-reason");
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Revise,
                reason: Some("   ".to_string()),
                new_strategy: Some(concrete_strategy("ship the feature")),
            })
            .await;
        assert!(out.is_err(), "empty/whitespace reason must be rejected");
    }

    #[tokio::test]
    async fn revise_rejects_empty_guardrails() {
        let (tool, _d) = tool_with_session("sess-empty-guards");
        let mut s = concrete_strategy("ship the feature");
        s.guardrails = vec!["   ".to_string()]; // no concrete guardrail => is_empty() true
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Revise,
                reason: Some("environment changed".to_string()),
                new_strategy: Some(s),
            })
            .await;
        assert!(
            out.is_err(),
            "a strategy with no concrete guardrail must be rejected"
        );
    }

    #[tokio::test]
    async fn revise_overwrites_in_force_strategy() {
        let (tool, _d) = tool_with_session("sess-overwrite");
        // Seed a goal-keyed strategy directly in the store.
        let store = tool.store.clone();
        store
            .put(
                &goal_key("sess-overwrite"),
                &concrete_strategy("old objective"),
            )
            .unwrap();
        let mut revised = concrete_strategy("new objective after shock");
        revised.approach = "pivot to the new approach".to_string();
        tool.run(StrategyArgs {
            action: StrategyAction::Revise,
            reason: Some("the API we relied on was removed".to_string()),
            new_strategy: Some(revised.clone()),
        })
        .await
        .unwrap();
        // The goal-keyed strategy is overwritten (revise resolves to it via precedence).
        let stored = store.get(&goal_key("sess-overwrite")).unwrap().unwrap();
        assert_eq!(stored.objective, "new objective after shock");
        assert_eq!(stored.approach, "pivot to the new approach");
    }

    #[tokio::test]
    async fn revise_writes_loop_key_when_only_loop_strategy_exists() {
        let (tool, _d) = tool_with_session("sess-loop-only");
        let store = tool.store.clone();
        store
            .put(
                &loop_key("sess-loop-only"),
                &concrete_strategy("loop objective"),
            )
            .unwrap();
        let mut revised = concrete_strategy("loop objective revised");
        revised.guardrails = vec!["stay on the watch target".to_string()];
        tool.run(StrategyArgs {
            action: StrategyAction::Revise,
            reason: Some("the watch target moved".to_string()),
            new_strategy: Some(revised),
        })
        .await
        .unwrap();
        // No goal-keyed strategy exists -> revise falls back to the loop key.
        assert!(store.get(&goal_key("sess-loop-only")).unwrap().is_none());
        let stored = store.get(&loop_key("sess-loop-only")).unwrap().unwrap();
        assert_eq!(stored.objective, "loop objective revised");
    }

    #[tokio::test]
    async fn show_returns_current_strategy() {
        let (tool, _d) = tool_with_session("sess-show");
        let store = tool.store.clone();
        store
            .put(&goal_key("sess-show"), &concrete_strategy("show me"))
            .unwrap();
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Show,
                reason: None,
                new_strategy: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.contains("show me"), "got: {}", out.message);
    }

    #[tokio::test]
    async fn show_with_no_strategy_is_graceful() {
        let (tool, _d) = tool_with_session("sess-none");
        let out = tool
            .run(StrategyArgs {
                action: StrategyAction::Show,
                reason: None,
                new_strategy: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(
            out.message.to_lowercase().contains("no strategy"),
            "got: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn resolve_key_returns_session_when_only_session_exists() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("s.db")).unwrap());
        let strat = Strategy {
            objective: "obj".into(),
            approach: "appr".into(),
            phases: vec![],
            guardrails: vec!["avoid X".into()],
            success_criteria: "done when Y".into(),
            goal_id: None,
        };
        store.put(&session_key("sess-1"), &strat).unwrap();
        let tool = StrategyTool::new(store).with_session_for_test("sess-1");
        let key = tool.resolve_key("sess-1").unwrap();
        assert_eq!(key.as_deref(), Some("session:sess-1"));
    }

    #[tokio::test]
    async fn resolve_key_goal_beats_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(StrategyStore::open(&dir.path().join("s.db")).unwrap());
        let strat = Strategy {
            objective: "obj".into(),
            approach: "appr".into(),
            phases: vec![],
            guardrails: vec!["avoid X".into()],
            success_criteria: "done when Y".into(),
            goal_id: None,
        };
        store.put(&session_key("sess-1"), &strat).unwrap();
        store.put(&goal_key("sess-1"), &strat).unwrap();
        let tool = StrategyTool::new(store).with_session_for_test("sess-1");
        let key = tool.resolve_key("sess-1").unwrap();
        assert_eq!(key.as_deref(), Some("goal:sess-1"));
    }
}
