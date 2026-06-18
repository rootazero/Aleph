//! `loop` builtin tool (R8): the LLM starts/stops/paces an in-session timer
//! loop in natural language. The clock-gated sibling of `goal`.
//!
//! Registered under the name `loop`, so `/loop ...` resolves to it via the
//! command parser. The actual re-firing happens in the execution engine's
//! continuation hook (see `gateway::execution_engine::execute`).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

use crate::error::{AlephError, Result};
use crate::looping::{Cadence, LoopRegistry, LoopState, LoopStatus};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopAction {
    /// Begin a timer loop in this session.
    Start,
    /// Stop the session's loop (the only way it ends, absent a safety cap).
    Stop,
    /// Read the current loop: cadence, ticks used, caps, next wake.
    Status,
    /// Re-pace a model-paced loop (`next_wake`) or adjust caps.
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LoopArgs {
    pub action: LoopAction,
    /// Fixed cadence, human form: "30s" / "5m" / "2h". Omit on `start` to use
    /// model-paced cadence (you set the next wake each tick via `update`).
    pub interval: Option<String>,
    /// The prompt re-run each tick — required for `start`.
    pub prompt: Option<String>,
    /// Optional safety cap: stop after this many ticks.
    pub max_iterations: Option<u32>,
    /// Optional safety cap: wall-clock minutes from now.
    pub timeout_minutes: Option<u32>,
    /// Optional soft token budget.
    pub token_budget: Option<u64>,
    /// For `update` on a model-paced loop: when to wake next, human form
    /// ("8m"). Stored as an absolute deadline (now + delta).
    pub next_wake: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopOutput {
    pub success: bool,
    pub message: String,
}

/// Default model-paced fallback when the model never sets `next_wake`.
const MODEL_PACED_FALLBACK_MS: u64 = 600_000; // 10 min

/// Default safety cap applied when a loop is started with no explicit
/// max_iterations AND no timeout — prevents an unattended uncapped loop from
/// running forever on the 24/7 daemon. Generous (the model/user can raise it),
/// but never truly unbounded by default.
pub const DEFAULT_SOFT_MAX_ITERATIONS: u32 = 500;

/// Parse a human duration ("30s","5m","2h","500ms") into ms. Rejects garbage
/// and sub-second values (a sub-second loop would hammer the engine). No new
/// dependency — small hand parser (R3 core minimalism).
pub fn parse_interval_ms(s: &str) -> std::result::Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty interval".to_string());
    }
    let (num, unit_ms): (&str, u64) = if let Some(n) = s.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        return Err(format!("unrecognized interval '{s}' (use 30s/5m/2h)"));
    };
    let value: u64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid interval number in '{s}'"))?;
    let ms = value.saturating_mul(unit_ms);
    if ms < 1_000 {
        return Err(format!("interval too short: '{s}' is below the 1s minimum"));
    }
    Ok(ms)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Light env summary for the planner (OS + cwd), never failing.
fn planner_env_summary() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    format!("os={} cwd={}", std::env::consts::OS, cwd)
}

#[derive(Clone)]
pub struct LoopTool {
    registry: Arc<LoopRegistry>,
    session_key: Option<Arc<RwLock<String>>>,
    /// Tool-free planner provider; `None` → no Strategy on `start`.
    planner_provider: Option<Arc<dyn AiProvider>>,
    #[cfg(test)]
    test_session: Option<String>,
}

impl LoopTool {
    #[must_use]
    pub fn new(registry: Arc<LoopRegistry>) -> Self {
        Self {
            registry,
            session_key: None,
            planner_provider: None,
            #[cfg(test)]
            test_session: None,
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
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    /// Core dispatch — public so tests call it directly without the trait.
    pub async fn run(&self, args: LoopArgs) -> std::result::Result<LoopOutput, String> {
        let session = self.session().await;
        info!(session = %session, action = ?args.action, "loop operation");
        match args.action {
            LoopAction::Start => {
                // Capture the watch prompt before `start` consumes `args` so the
                // planner can plan over the loop's objective.
                let objective = args.prompt.clone().unwrap_or_default();
                let out = self.start(&session, args)?;
                if out.success {
                    self.maybe_plan_strategy(&session, &objective).await;
                }
                Ok(out)
            }
            LoopAction::Stop => self.stop(&session),
            LoopAction::Status => self.status(&session),
            LoopAction::Update => self.update(&session, args),
        }
    }

    fn start(&self, session: &str, args: LoopArgs) -> std::result::Result<LoopOutput, String> {
        let prompt = args
            .prompt
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| "start requires a non-empty prompt".to_string())?;
        let cadence = match &args.interval {
            Some(i) => Cadence::Fixed {
                interval_ms: parse_interval_ms(i)?,
            },
            None => Cadence::ModelPaced {
                fallback_ms: MODEL_PACED_FALLBACK_MS,
            },
        };
        let now = now_ms();
        let deadline = args
            .timeout_minutes
            .map(|m| now.saturating_add(u64::from(m).saturating_mul(60_000)));
        // Safety net: a loop with no user-supplied bound at all gets a soft
        // iteration cap so unattended pursuit cannot run unbounded forever.
        let effective_max = match (args.max_iterations, deadline) {
            (Some(m), _) => Some(m),
            (None, Some(_)) => None, // a deadline is itself a bound
            (None, None) => Some(DEFAULT_SOFT_MAX_ITERATIONS),
        };
        let state = LoopState::new(session, &prompt, cadence, now)
            .with_max_iterations(effective_max)
            .with_deadline_ms(deadline)
            .with_token_budget(args.token_budget);
        self.registry.put(state);
        Ok(LoopOutput {
            success: true,
            message: "Loop started in this session. It will re-run every tick and \
                 will not self-stop — call loop(action='stop') to end it."
                .to_string(),
        })
    }

    fn stop(&self, session: &str) -> std::result::Result<LoopOutput, String> {
        match self.registry.get(session) {
            // Already stopped → report honestly rather than claiming a fresh
            // stop. Surfaces the prior stop reason so the user understands why.
            Some(state) if !state.is_active() => Ok(LoopOutput {
                success: false,
                message: match &state.stop_reason {
                    Some(r) => format!("Loop was already stopped ({r})."),
                    None => "Loop was already stopped.".to_string(),
                },
            }),
            Some(state) => {
                self.registry.put(
                    state
                        .with_status(LoopStatus::Stopped)
                        .with_stop_reason(Some("Stopped by user request.".to_string())),
                );
                Ok(LoopOutput {
                    success: true,
                    message: "Loop stopped.".to_string(),
                })
            }
            None => Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            }),
        }
    }

    fn status(&self, session: &str) -> std::result::Result<LoopOutput, String> {
        match self.registry.get(session) {
            Some(s) => Ok(LoopOutput {
                success: true,
                message: s.human_summary(now_ms()),
            }),
            None => Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            }),
        }
    }

    fn update(&self, session: &str, args: LoopArgs) -> std::result::Result<LoopOutput, String> {
        let Some(mut state) = self.registry.get(session) else {
            return Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            });
        };
        // A stopped loop cannot be re-paced in place — `update` is for live
        // loops. Resurrecting it silently would lie ("Loop updated") while the
        // continuation hook (which only fires for Active loops) never re-runs
        // it. Tell the user to start a fresh loop instead.
        if !state.is_active() {
            return Ok(LoopOutput {
                success: false,
                message: match &state.stop_reason {
                    Some(r) => format!(
                        "Loop is stopped ({r}); update only re-paces a running loop. \
                         Call loop(action='start') to begin a new one."
                    ),
                    None => {
                        "Loop is stopped; call loop(action='start') to begin a new one.".to_string()
                    }
                },
            });
        }
        // Re-pace a Fixed loop (or convert model-paced → fixed) without a
        // stop/start cycle. `with_cadence` clears any stale next_wake.
        if let Some(i) = &args.interval {
            state = state.with_cadence(Cadence::Fixed {
                interval_ms: parse_interval_ms(i)?,
            });
        }
        if let Some(nw) = &args.next_wake {
            let delta = parse_interval_ms(nw)?;
            state = state.with_next_wake_ms(Some(now_ms().saturating_add(delta)));
        }
        if args.max_iterations.is_some() {
            state = state.with_max_iterations(args.max_iterations);
        }
        // Reset the wall-clock deadline relative to now (a fresh watch window).
        if let Some(m) = args.timeout_minutes {
            let deadline = now_ms().saturating_add(u64::from(m).saturating_mul(60_000));
            state = state.with_deadline_ms(Some(deadline));
        }
        if args.token_budget.is_some() {
            state = state.with_token_budget(args.token_budget);
        }
        // Re-target the watch prompt (ignore empty, which would blank the loop).
        if let Some(p) = args.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
            state = state.with_prompt(p);
        }
        self.registry.put(state);
        Ok(LoopOutput {
            success: true,
            message: "Loop updated.".to_string(),
        })
    }

    /// Fire the tool-free planner ONCE for this session's loop, fail-soft.
    /// No-op when no provider is injected, no global StrategyStore exists, a
    /// Strategy already exists for the key, or the planner self-gates/errs.
    async fn maybe_plan_strategy(&self, session: &str, objective: &str) {
        let Some(provider) = &self.planner_provider else {
            return;
        };
        let Some(store) = crate::strategy::global() else {
            return;
        };
        let key = crate::strategy::loop_key(session);
        // Fire-exactly-once: plan only when the slot is provably empty (Ok(None));
        // an existing row (Ok(Some)) or a get failure (Err) both skip (P7).
        if !matches!(store.get(&key), Ok(None)) {
            return;
        }
        let ctx = crate::strategy::planner::PlannerContext {
            tool_descriptions: Vec::new(),
            env_summary: planner_env_summary(),
            lessons: Vec::new(),
        };
        if let Some(strategy) =
            crate::strategy::planner::plan_strategy(provider, objective, &ctx, None).await
        {
            let _ = store.put(&key, &strategy);
        }
    }
}

#[async_trait]
impl AlephTool for LoopTool {
    const NAME: &'static str = "loop";
    const DESCRIPTION: &'static str =
        "Start a timer loop that re-runs a prompt on a schedule in THIS session. \
         Unlike `goal` (which stops when a condition is met), a loop runs to a \
         clock and never self-stops — end it with action='stop'. Use \
         action='start' with `interval` (e.g. '5m') for a fixed cadence, or omit \
         `interval` for model-paced (call action='update' with `next_wake` each \
         tick to set the next delay). action='update' also re-paces a running \
         loop in place — pass `interval` to change a fixed cadence, or \
         `prompt`/`timeout_minutes`/`max_iterations` to re-target or re-bound it \
         without stop/start. Optional safety caps: max_iterations, \
         timeout_minutes. Use for watch/poll duties (e.g. 'every 5 minutes check \
         the deploy and tell me if it changed').";

    type Args = LoopArgs;
    type Output = LoopOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "loop(action='start', interval='5m', prompt='Check the deploy status; tell me if it changed')".into(),
            "loop(action='start', prompt='Triage the PR queue', max_iterations=20)".into(),
            "loop(action='update', next_wake='8m')".into(),
            "loop(action='update', interval='10m')".into(),
            "loop(action='status')".into(),
            "loop(action='stop')".into(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let session = self.session().await;
        if session.is_empty() {
            return Err(AlephError::tool(
                "loop tool has no active session binding".to_string(),
            ));
        }
        self.run(args).await.map_err(AlephError::tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_interval_handles_units() {
        assert_eq!(parse_interval_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_interval_ms("5m").unwrap(), 300_000);
        assert_eq!(parse_interval_ms("2h").unwrap(), 7_200_000);
        assert_eq!(parse_interval_ms("1500ms").unwrap(), 1500);
    }

    #[test]
    fn parse_interval_rejects_garbage() {
        assert!(parse_interval_ms("soon").is_err());
        assert!(parse_interval_ms("").is_err());
        assert!(parse_interval_ms("5x").is_err());
    }

    #[test]
    fn parse_interval_rejects_sub_second_fixed() {
        // sub-second intervals would hammer the loop — reject below 1000ms
        // (mirrors cron_manage every_ms < 1000 guard).
        assert!(parse_interval_ms("100ms").is_err());
    }

    #[tokio::test]
    async fn start_with_interval_registers_active_fixed_loop() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("sess-x");
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Start,
                interval: Some("5m".to_string()),
                prompt: Some("check deploy".to_string()),
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        let st = reg.get("sess-x").unwrap();
        assert!(st.is_active());
        assert_eq!(st.prompt, "check deploy");
        assert!(matches!(
            st.cadence,
            crate::looping::Cadence::Fixed {
                interval_ms: 300_000
            }
        ));
    }

    #[tokio::test]
    async fn start_without_interval_is_model_paced() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: None,
            prompt: Some("watch CI".to_string()),
            max_iterations: Some(20),
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        let st = reg.get("s").unwrap();
        assert!(matches!(
            st.cadence,
            crate::looping::Cadence::ModelPaced { .. }
        ));
        assert_eq!(st.max_iterations, Some(20));
    }

    #[tokio::test]
    async fn stop_marks_loop_stopped() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s",
            "p",
            crate::looping::Cadence::Fixed { interval_ms: 1000 },
            0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Stop,
            interval: None,
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        assert!(!reg.get("s").unwrap().is_active());
    }

    #[tokio::test]
    async fn update_sets_next_wake_for_model_paced() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s",
            "p",
            crate::looping::Cadence::ModelPaced {
                fallback_ms: 600_000,
            },
            0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Update,
            interval: None,
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: Some("8m".to_string()),
        })
        .await
        .unwrap();
        // next_wake stored as an absolute epoch-ms; just assert it is now set.
        assert!(reg.get("s").unwrap().next_wake_ms.is_some());
    }

    #[tokio::test]
    async fn update_repaces_fixed_interval_in_place() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s",
            "p",
            crate::looping::Cadence::Fixed {
                interval_ms: 300_000,
            },
            0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Update,
            interval: Some("10m".to_string()),
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        assert!(matches!(
            reg.get("s").unwrap().cadence,
            crate::looping::Cadence::Fixed {
                interval_ms: 600_000
            }
        ));
    }

    #[tokio::test]
    async fn update_retargets_prompt_and_resets_deadline() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s",
            "old",
            crate::looping::Cadence::Fixed { interval_ms: 1000 },
            0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Update,
            interval: None,
            prompt: Some("watch staging".to_string()),
            max_iterations: None,
            timeout_minutes: Some(30),
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        let st = reg.get("s").unwrap();
        assert_eq!(st.prompt, "watch staging");
        assert!(
            st.deadline_ms.is_some(),
            "deadline set from timeout_minutes"
        );
    }

    #[tokio::test]
    async fn start_without_any_cap_gets_default_soft_max() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some("p".to_string()),
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        // No user cap → a default soft cap is applied so unattended loops
        // cannot run unbounded forever.
        assert_eq!(
            reg.get("s").unwrap().max_iterations,
            Some(DEFAULT_SOFT_MAX_ITERATIONS)
        );
    }

    #[tokio::test]
    async fn status_is_human_readable_and_shows_stop_reason() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(
            crate::looping::LoopState::new(
                "s",
                "p",
                crate::looping::Cadence::Fixed {
                    interval_ms: 300_000,
                },
                0,
            )
            .with_status(LoopStatus::Stopped)
            .with_stop_reason(Some("reached the iteration cap (20 ticks).".to_string())),
        );
        let tool = LoopTool::new(reg).with_session_for_test("s");
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Status,
                interval: None,
                prompt: None,
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.contains("Loop stopped"), "{}", out.message);
        assert!(out.message.contains("every 5m"), "{}", out.message);
        assert!(out.message.contains("reason:"), "{}", out.message);
        // No raw Debug enum leakage.
        assert!(!out.message.contains("Fixed {"), "{}", out.message);
    }

    #[tokio::test]
    async fn update_on_stopped_loop_reports_honestly_without_mutating() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(
            crate::looping::LoopState::new(
                "s",
                "old",
                crate::looping::Cadence::Fixed {
                    interval_ms: 300_000,
                },
                0,
            )
            .with_status(LoopStatus::Stopped),
        );
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Update,
                interval: Some("10m".to_string()),
                prompt: Some("new prompt".to_string()),
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(
            !out.success,
            "updating a stopped loop must not claim success"
        );
        assert!(out.message.contains("start"), "{}", out.message);
        // The loop must be untouched: still stopped, prompt unchanged.
        let st = reg.get("s").unwrap();
        assert!(!st.is_active());
        assert_eq!(st.prompt, "old", "stopped loop must not be mutated");
    }

    #[tokio::test]
    async fn stop_on_already_stopped_loop_reports_not_active() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(
            crate::looping::LoopState::new(
                "s",
                "p",
                crate::looping::Cadence::Fixed { interval_ms: 1000 },
                0,
            )
            .with_status(LoopStatus::Stopped)
            .with_stop_reason(Some("reached its time limit.".to_string())),
        );
        let tool = LoopTool::new(reg).with_session_for_test("s");
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Stop,
                interval: None,
                prompt: None,
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(out.message.contains("already stopped"), "{}", out.message);
    }

    #[tokio::test]
    async fn explicit_cap_is_respected_over_default() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some("p".to_string()),
            max_iterations: Some(1000),
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
        })
        .await
        .unwrap();
        assert_eq!(reg.get("s").unwrap().max_iterations, Some(1000));
    }

    #[tokio::test]
    async fn with_planner_provider_builds_and_still_starts_loop() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let provider: crate::sync_primitives::Arc<dyn crate::providers::AiProvider> =
            crate::sync_primitives::Arc::new(crate::providers::MockProvider::new("not json"));
        let tool = LoopTool::new(reg.clone())
            .with_session_for_test("sess-lp")
            .with_planner_provider(Some(provider));
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Start,
                interval: Some("5m".to_string()),
                prompt: Some("watch".to_string()),
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(out.success);
    }

    /// Provider = None → loop `start` still succeeds and stores NO Strategy.
    #[tokio::test]
    async fn loop_start_with_no_provider_succeeds_without_strategy() {
        use crate::strategy::{loop_key, StrategyStore};
        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        );
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg).with_session_for_test("sess-lp-noprov");
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Start,
                interval: Some("5m".to_string()),
                prompt: Some("watch".to_string()),
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(
            crate::strategy::global()
                .unwrap()
                .get(&loop_key("sess-lp-noprov"))
                .unwrap()
                .is_none(),
            "no provider => no Strategy"
        );
    }
}
