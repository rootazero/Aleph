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

#[derive(Clone)]
pub struct LoopTool {
    registry: Arc<LoopRegistry>,
    session_key: Option<Arc<RwLock<String>>>,
    #[cfg(test)]
    test_session: Option<String>,
}

impl LoopTool {
    #[must_use]
    pub fn new(registry: Arc<LoopRegistry>) -> Self {
        Self {
            registry,
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
            LoopAction::Start => self.start(&session, args),
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
            Some(state) => {
                self.registry.put(state.with_status(LoopStatus::Stopped));
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
                message: format!(
                    "Loop: status={:?}, ticks_used={}, cadence={:?}, \
                     max_iterations={:?}, deadline_ms={:?}, token_budget={:?}",
                    s.status,
                    s.iterations_used,
                    s.cadence,
                    s.max_iterations,
                    s.deadline_ms,
                    s.token_budget
                ),
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
        if let Some(nw) = &args.next_wake {
            let delta = parse_interval_ms(nw)?;
            state = state.with_next_wake_ms(Some(now_ms().saturating_add(delta)));
        }
        if args.max_iterations.is_some() {
            state = state.with_max_iterations(args.max_iterations);
        }
        self.registry.put(state);
        Ok(LoopOutput {
            success: true,
            message: "Loop updated.".to_string(),
        })
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
         tick to set the next delay). Optional safety caps: max_iterations, \
         timeout_minutes. Use for watch/poll duties (e.g. 'every 5 minutes check \
         the deploy and tell me if it changed').";

    type Args = LoopArgs;
    type Output = LoopOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "loop(action='start', interval='5m', prompt='Check the deploy status; tell me if it changed')".into(),
            "loop(action='start', prompt='Triage the PR queue', max_iterations=20)".into(),
            "loop(action='update', next_wake='8m')".into(),
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
        assert_eq!(parse_interval_ms("500ms").unwrap(), 500);
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
        assert!(matches!(st.cadence, crate::looping::Cadence::Fixed { interval_ms: 300_000 }));
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
        assert!(matches!(st.cadence, crate::looping::Cadence::ModelPaced { .. }));
        assert_eq!(st.max_iterations, Some(20));
    }

    #[tokio::test]
    async fn stop_marks_loop_stopped() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s", "p", crate::looping::Cadence::Fixed { interval_ms: 1000 }, 0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Stop,
            interval: None, prompt: None, max_iterations: None,
            timeout_minutes: None, token_budget: None, next_wake: None,
        })
        .await
        .unwrap();
        assert!(!reg.get("s").unwrap().is_active());
    }

    #[tokio::test]
    async fn update_sets_next_wake_for_model_paced() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "s", "p", crate::looping::Cadence::ModelPaced { fallback_ms: 600_000 }, 0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Update,
            interval: None, prompt: None, max_iterations: None,
            timeout_minutes: None, token_budget: None,
            next_wake: Some("8m".to_string()),
        })
        .await
        .unwrap();
        // next_wake stored as an absolute epoch-ms; just assert it is now set.
        assert!(reg.get("s").unwrap().next_wake_ms.is_some());
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
        assert_eq!(reg.get("s").unwrap().max_iterations, Some(DEFAULT_SOFT_MAX_ITERATIONS));
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
}
