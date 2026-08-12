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
    /// Begin a timer loop in this session. Replaces any loop already here,
    /// INCLUDING a paused one (whose tick count and caps are then lost) —
    /// check `status` first if a held watch might exist.
    Start,
    /// Stop the session's loop (the only way it ends, absent a safety cap).
    Stop,
    /// Read the current loop: cadence, ticks used, caps, next wake.
    Status,
    /// Re-pace a model-paced loop (`next_wake`) or adjust caps.
    Update,
    /// List every timer loop across ALL sessions (not just this one), so the
    /// model can answer "what loops are running?" from any channel — `status`
    /// only sees the current session (R6 one-core-many-shells / R8).
    List,
    /// Suspend the loop without losing it: caps, tick count, prompt and cadence
    /// all survive, and no tick fires until `resume`. Use when the user wants
    /// the watch held rather than ended.
    Pause,
    /// Resume a paused loop in THIS session. The next tick is claimed when this
    /// turn completes.
    Resume,
    /// Kill switch: stop EVERY timer loop in every session at once. For "stop
    /// all the loops" / incident response. Operator only.
    StopAll,
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
    /// Target ANOTHER session's loop — the session key exactly as
    /// `action='list'` prints it. Omit for this session (the normal case).
    ///
    /// Honored by the read verb (`status`) and the QUIETING verbs (`stop`,
    /// `pause`) only. `start` / `resume` / `update` refuse it: arming a loop in
    /// a session that is not currently running a turn would leave it `Active`
    /// with nothing to claim its next tick — a dormant loop that `status`
    /// misreports as running. Requires operator authorization.
    pub session: Option<String>,
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

/// Reject a zero safety cap at the system boundary (P7). A `max_iterations` or
/// `timeout_minutes` of 0 would create a loop that the continuation hook marks
/// exhausted on its very first check — "born dead" — and reports a confusing
/// "reached the cap (0 ticks)". Omit the field for "no cap"; never pass 0.
fn reject_zero_cap(value: Option<u32>, field: &str) -> std::result::Result<(), String> {
    if value == Some(0) {
        return Err(format!(
            "{field} must be at least 1 (omit it for no cap, do not pass 0)"
        ));
    }
    Ok(())
}

/// u64 sibling of [`reject_zero_cap`] for `token_budget` — a 0 budget survives
/// exactly one tick (the baseline seed) before stopping with a baffling
/// "reached its token budget (0 tokens)".
fn reject_zero_budget(value: Option<u64>) -> std::result::Result<(), String> {
    if value == Some(0) {
        return Err(
            "token_budget must be at least 1 (omit it for no budget, do not pass 0)".to_string(),
        );
    }
    Ok(())
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
        // Per-run truth first: the shared registry handle is process-global
        // and rewritten at every run start, so a concurrent run of another
        // agent can overwrite it mid-turn and `start` would bind the loop to
        // that run's session. The task-local is scoped per tool call by the
        // dispatch chokepoint and cannot race.
        if let Some(sk) = crate::tools::turn_context::current_session_key() {
            return sk;
        }
        match &self.session_key {
            Some(h) => h.read().await.clone(),
            None => String::new(),
        }
    }

    /// Is this turn allowed to reach across session boundaries?
    ///
    /// Loops are per-session, so before cross-session targeting existed the
    /// blast radius of the `loop` tool was exactly the caller's own
    /// conversation — which is why `loop` is deliberately NOT in
    /// `method_authz::OPERATOR_TOOLS` (a chat-tier Telegram user pacing their
    /// own watch is harmless). `session=` and `stop_all` widen that radius
    /// across the trust boundary, so they carry their own operator gate here,
    /// exactly as `select_model` gates its own cross-cutting arm. Absent role =
    /// trusted local/internal run.
    fn caller_is_operator() -> bool {
        crate::tools::turn_context::current_turn_context()
            .is_none_or(|ctx| ctx.caller_is_operator())
    }

    /// Resolve which session a verb acts on. `None` → this session. `Some(key)`
    /// → that session, once the operator gate passes and the key names a loop
    /// the registry actually knows (a typo must not silently act on nothing, or
    /// worse, read as "no loop is running").
    fn resolve_target(
        &self,
        session: &str,
        requested: Option<&str>,
        verb: &str,
    ) -> std::result::Result<String, String> {
        let Some(target) = requested.map(str::trim).filter(|s| !s.is_empty()) else {
            return Ok(session.to_string());
        };
        if target == session {
            return Ok(session.to_string());
        }
        if !Self::caller_is_operator() {
            return Err(format!(
                "{verb} on another session requires operator authorization; \
                 this conversation may only manage its own loop"
            ));
        }
        if self.registry.get(target).is_none() {
            return Err(format!(
                "no timer loop is registered for session '{target}' — call \
                 loop(action='list') and pass a session key exactly as it prints"
            ));
        }
        Ok(target.to_string())
    }

    /// Reject `session=` on the verbs that ARM a loop (`start` / `resume` /
    /// `update` with a reschedule). A loop only ever gets its next tick from the
    /// completion hook of a run in its own session, so arming one remotely
    /// produces an `Active` loop with nothing to claim a tick — dormant until
    /// that session's user happens to speak again, while `status` and `list`
    /// both report it as running. Refusing is the honest answer; the quieting
    /// verbs (`stop` / `pause`) need no scheduling and are allowed.
    fn reject_remote(
        session: &str,
        args: &LoopArgs,
        verb: &str,
    ) -> std::result::Result<(), String> {
        match args.session.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() && t != session => Err(format!(
                "{verb} only works on the current session: a loop is re-fired by \
                 its own session's completion hook, so one armed from elsewhere \
                 would never tick. Run {verb} from that session, or use \
                 loop(action='stop', session='…') / loop(action='pause', session='…') \
                 to quiet it from here"
            )),
            _ => Ok(()),
        }
    }

    /// Core dispatch — public so tests call it directly without the trait.
    pub async fn run(&self, args: LoopArgs) -> std::result::Result<LoopOutput, String> {
        let session = self.session().await;
        info!(session = %session, action = ?args.action, "loop operation");
        match args.action {
            LoopAction::Start => {
                Self::reject_remote(&session, &args, "start")?;
                // Capture the watch prompt before `start` consumes `args` so the
                // planner can plan over the loop's objective.
                let objective = args.prompt.clone().unwrap_or_default();
                let out = self.start(&session, args)?;
                if out.success {
                    self.maybe_plan_strategy(&session, &objective).await;
                }
                Ok(out)
            }
            LoopAction::Stop => {
                let target = self.resolve_target(&session, args.session.as_deref(), "stop")?;
                self.stop(&session, &target)
            }
            LoopAction::Pause => {
                let target = self.resolve_target(&session, args.session.as_deref(), "pause")?;
                self.pause(&session, &target)
            }
            LoopAction::Resume => {
                Self::reject_remote(&session, &args, "resume")?;
                self.resume(&session)
            }
            LoopAction::StopAll => self.stop_all(),
            LoopAction::Status => {
                let target = self.resolve_target(&session, args.session.as_deref(), "status")?;
                self.status(&session, &target)
            }
            LoopAction::Update => {
                Self::reject_remote(&session, &args, "update")?;
                self.update(&session, args)
            }
            LoopAction::List => self.list(&session),
        }
    }

    fn start(&self, session: &str, args: LoopArgs) -> std::result::Result<LoopOutput, String> {
        let prompt = args
            .prompt
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| "start requires a non-empty prompt".to_string())?;
        // Reject "born dead" zero caps at the boundary before building state.
        reject_zero_cap(args.max_iterations, "max_iterations")?;
        reject_zero_cap(args.timeout_minutes, "timeout_minutes")?;
        reject_zero_budget(args.token_budget)?;
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
        let mut state = LoopState::new(session, &prompt, cadence, now)
            .with_max_iterations(effective_max)
            .with_deadline_ms(deadline)
            .with_token_budget(args.token_budget)
            // P1 data isolation: stamp the owning user/scope from the ambient
            // attribution of THIS creating run — mirrors goal.rs's stamp.
            .with_owner_scope(crate::scope::current_scope().as_ref());
        // Honor next_wake on a model-paced start so the model's chosen pace
        // applies from tick 1 instead of the 10-minute fallback; on a fixed
        // cadence it is contradictory — reject with the same guidance as
        // `update` rather than silently ignoring an accepted argument.
        if let Some(nw) = &args.next_wake {
            if matches!(state.cadence, Cadence::ModelPaced { .. }) {
                let delta = parse_interval_ms(nw)?;
                state = state.with_next_wake_ms(Some(now.saturating_add(delta)));
            } else {
                return Err("next_wake only paces a model-paced loop; omit `interval` \
                     on start for model-paced cadence, or drop `next_wake`"
                    .to_string());
            }
        }
        // `start` claims a fresh plan slot: clear any leftover loop-keyed
        // Strategy (e.g. from an overwritten still-active loop) so
        // maybe_plan_strategy plans for THIS objective instead of silently
        // welding the old one. Best-effort, same as the stop-side cleanup.
        // "Still exists" is `status != Stopped`, the same predicate the
        // collision count below uses — NOT `is_active()`. A Paused loop is
        // exactly the state pause exists to preserve (tick count, caps, prompt,
        // cadence), and `put` overwrites it unconditionally; reporting the
        // neutral "Loop started in this session." there destroyed a held watch
        // and said nothing.
        let replaced = self
            .registry
            .get(session)
            .map(|p| p.status)
            .filter(|s| *s != LoopStatus::Stopped);
        Self::clear_welded_strategy(session, "start fresh-plan claim");
        let cadence_desc = state.cadence.describe();
        self.registry.put(state);
        // Honest confirmation: say what was actually registered — the parsed
        // cadence, whether a previous loop was replaced, and the silently
        // applied default cap (which the model can raise or replace).
        let lead = match replaced {
            Some(LoopStatus::Paused) => {
                "Loop started, replacing the loop that was PAUSED in this session — \
                 its tick count, caps, prompt and cadence are gone."
            }
            Some(_) => "Loop started, replacing this session's previous loop.",
            None => "Loop started in this session.",
        };
        let cap_note = match (args.max_iterations, deadline) {
            (None, None) => format!(
                " A default safety cap of {DEFAULT_SOFT_MAX_ITERATIONS} ticks applies \
                 (pass max_iterations or timeout_minutes to change it)."
            ),
            _ => String::new(),
        };
        // Collision disclosure: loops are per-session, so a watch started here
        // knows nothing about one already watching the same thing from another
        // channel — `<timer_loop>` only ever projects THIS session's loop. Two
        // loops polling the same target double the token burn and can act on
        // each other's half-finished work. State the count and stop there: what
        // to do about it is the model's call, not the registry's (R7).
        let elsewhere = self
            .registry
            .list_all()
            .iter()
            .filter(|l| l.session_id != session && l.status != LoopStatus::Stopped)
            .count();
        let collision_note = match elsewhere {
            0 => String::new(),
            n => format!(
                " Note: {n} other timer loop(s) are live in other sessions — \
                 loop(action='list') shows them if this might duplicate one."
            ),
        };
        Ok(LoopOutput {
            success: true,
            message: format!(
                "{lead} It will re-run ({cadence_desc}) and will not self-stop — \
                 call loop(action='stop') to end it.{cap_note}{collision_note}"
            ),
        })
    }

    /// Clear the loop-welded Strategy for a session (best-effort). Tool-side
    /// single source for `stop` and `start`'s fresh-plan-slot claim; the
    /// gateway endings (cap / cap-trip / failure stops) clear the same key via
    /// `execution_engine::execute::clear_loop_welded_strategy`. The goal-keyed
    /// Strategy (if any) is never touched. Mirrors `GoalTool`'s tool-side
    /// clear (the continuation siblings keep parity).
    fn clear_welded_strategy(session: &str, context: &str) {
        if let Some(strat) = crate::strategy::global() {
            if let Err(e) = strat.delete(&crate::strategy::loop_key(session)) {
                info!(session = %session, error = %e, context,
                    "loop: failed to delete welded strategy (ignored)");
            }
        }
    }

    /// " in session 'x'" when the verb reached across sessions, "" when it acted
    /// here. Every cross-session reply carries it so the user can never mistake
    /// a remote effect for a local one.
    fn scope_suffix(session: &str, target: &str) -> String {
        if target == session {
            String::new()
        } else {
            format!(" in session '{target}'")
        }
    }

    /// The stored "why it is not ticking" note. A remote quiet says so, because
    /// that session's own `status` is the only place its user will ever see it —
    /// there is no channel plumbing from here to another session's origin.
    fn quiet_note(session: &str, target: &str, verb: &str) -> String {
        if target == session {
            format!("{verb} by user request.")
        } else {
            format!("{verb} by user request from another session.")
        }
    }

    fn stop(&self, session: &str, target: &str) -> std::result::Result<LoopOutput, String> {
        let scope = Self::scope_suffix(session, target);
        match self.registry.transition(
            target,
            LoopStatus::Stopped,
            Some(Self::quiet_note(session, target, "Stopped")),
        ) {
            crate::looping::TransitionOutcome::Applied { .. } => {
                // Clear the loop-welded Strategy in lockstep with the
                // authoritative loop stop (spec §6 lifecycle). Best-effort; the
                // goal-keyed Strategy (if any) is untouched.
                Self::clear_welded_strategy(target, "tool stop");
                Ok(LoopOutput {
                    success: true,
                    message: format!("Loop stopped{scope}."),
                })
            }
            // Only an already-`Stopped` loop can refuse a stop — report that
            // honestly rather than claiming a fresh stop, and surface the prior
            // reason so the user understands why it had already ended.
            crate::looping::TransitionOutcome::Refused { .. } => Ok(LoopOutput {
                success: false,
                message: match self.registry.get(target).and_then(|s| s.stop_reason) {
                    Some(r) => format!("Loop{scope} was already stopped ({r})."),
                    None => format!("Loop{scope} was already stopped."),
                },
            }),
            crate::looping::TransitionOutcome::Missing => {
                // No live loop — but a welded plan may have outlived one (the
                // registry is process memory, the weld is persistent SQLite; a
                // daemon restart orphans it). An explicit stop is the user's
                // escape hatch: tidy the orphan row while reporting honestly.
                Self::clear_welded_strategy(target, "tool stop (no live loop)");
                Ok(LoopOutput {
                    success: false,
                    message: format!("No loop{scope}."),
                })
            }
        }
    }

    /// Suspend without losing the loop. The registry retires the in-flight tick
    /// as part of the transition, so `resume` takes effect at once instead of
    /// waiting out the wake the pause interrupted.
    fn pause(&self, session: &str, target: &str) -> std::result::Result<LoopOutput, String> {
        let scope = Self::scope_suffix(session, target);
        match self.registry.transition(
            target,
            LoopStatus::Paused,
            Some(Self::quiet_note(session, target, "Paused")),
        ) {
            crate::looping::TransitionOutcome::Applied { .. } => Ok(LoopOutput {
                success: true,
                message: format!(
                    "Loop paused{scope}; no ticks fire until loop(action='resume') \
                     runs in that session. Tick count, caps, prompt and cadence are \
                     preserved. A wall-clock `timeout_minutes` deadline keeps \
                     running while paused — re-set it on resume if the pause was long."
                ),
            }),
            crate::looping::TransitionOutcome::Refused { current } => Ok(LoopOutput {
                success: false,
                message: match current {
                    LoopStatus::Paused => format!("Loop{scope} is already paused."),
                    // Terminal: "pausing" it would imply a resume that cannot happen.
                    _ => format!(
                        "Loop{scope} is stopped, not running — nothing to pause. \
                         Call loop(action='start') to begin a new one."
                    ),
                },
            }),
            crate::looping::TransitionOutcome::Missing => Ok(LoopOutput {
                success: false,
                message: format!("No loop{scope}."),
            }),
        }
    }

    /// Put a paused loop back to work. Local-only by construction (see
    /// [`Self::reject_remote`]): the next tick is claimed by THIS turn's
    /// completion hook, which only runs for the session that is executing.
    fn resume(&self, session: &str) -> std::result::Result<LoopOutput, String> {
        // A wall-clock deadline keeps running while paused (the pause receipt
        // says so). Resuming a loop whose bound already elapsed used to answer
        // "the next tick is scheduled from now" and then, seconds later on the
        // same channel, "⏹ Loop stopped: reached its time limit" — the very
        // next post-run claim takes the Exhausted path. Refuse honestly up
        // front, reusing the same two pure functions the claim uses so there
        // is no second opinion about what "exhausted" means. Tokens are 0 here,
        // matching `rearm_after_busy`'s claim-side-only budget convention.
        if let Some(state) = self.registry.get(session).filter(LoopState::is_paused) {
            let now = now_ms();
            if crate::looping::pursuit::exhausted(&state, 0, now) {
                let reason = crate::looping::pursuit::stop_reason_note(&state, 0, now);
                return Ok(LoopOutput {
                    success: false,
                    message: format!(
                        "Loop cannot resume — it hit its bound while paused ({reason}). \
                         Widen it in place with loop(action='update', timeout_minutes=… \
                         / max_iterations=…) and resume, or loop(action='start') for a \
                         new one."
                    ),
                });
            }
        }
        match self.registry.transition(session, LoopStatus::Active, None) {
            crate::looping::TransitionOutcome::Applied { .. } => Ok(LoopOutput {
                success: true,
                message: "Loop resumed; the next tick is scheduled from now.".to_string(),
            }),
            crate::looping::TransitionOutcome::Refused { current } => Ok(LoopOutput {
                success: false,
                message: match current {
                    LoopStatus::Active => "Loop is already running.".to_string(),
                    // `Stopped` is terminal — resuming would resurrect a loop
                    // whose caps may already be spent.
                    _ => match self.registry.get(session).and_then(|s| s.stop_reason) {
                        Some(r) => format!(
                            "Loop is stopped ({r}); resume only restarts a PAUSED loop. \
                             Call loop(action='start') to begin a new one."
                        ),
                        None => "Loop is stopped; call loop(action='start') to begin a new one."
                            .to_string(),
                    },
                },
            }),
            crate::looping::TransitionOutcome::Missing => Ok(LoopOutput {
                success: false,
                message: "No loop in this session.".to_string(),
            }),
        }
    }

    /// Kill switch (operator only): quiet every loop everywhere in one call.
    /// The incident-response counterpart to `list` — before this, a loop was
    /// visible from any channel but stoppable only from its own session.
    fn stop_all(&self) -> std::result::Result<LoopOutput, String> {
        if !Self::caller_is_operator() {
            return Err(
                "stop_all requires operator authorization; this conversation \
                 may only manage its own loop"
                    .to_string(),
            );
        }
        let stopped = self
            .registry
            .stop_all("Stopped by loop(action='stop_all').");
        if stopped.is_empty() {
            return Ok(LoopOutput {
                success: true,
                message: "No running or paused timer loops in any session.".to_string(),
            });
        }
        for session in &stopped {
            Self::clear_welded_strategy(session, "stop_all");
        }
        Ok(LoopOutput {
            success: true,
            message: format!(
                "Stopped {} timer loop(s): {}.",
                stopped.len(),
                stopped.join(", ")
            ),
        })
    }

    fn status(&self, session: &str, target: &str) -> std::result::Result<LoopOutput, String> {
        let scope = Self::scope_suffix(session, target);
        match self.registry.get(target) {
            Some(s) => Ok(LoopOutput {
                success: true,
                message: if scope.is_empty() {
                    s.human_summary(now_ms())
                } else {
                    format!("{target}: {}", s.human_summary(now_ms()))
                },
            }),
            None => Ok(LoopOutput {
                success: false,
                message: format!("No loop{scope}."),
            }),
        }
    }

    /// Cross-session enumeration (R6 one-core-many-shells / R8 conversation-as-panel): a loop
    /// started on one channel is invisible to `status`, which keys by the
    /// current session. Reuse the registry's in-memory map so the model can
    /// answer "what timer loops are running?" from anywhere. Mirrors
    /// `GoalTool`'s `list`; process memory only, so the answer is exactly the
    /// loops alive now — no orphan rows to reconcile.
    fn list(&self, session: &str) -> std::result::Result<LoopOutput, String> {
        let mut loops = self.registry.list_all();
        // Scope, don't deny. Every OTHER cross-session verb here carries an
        // operator gate (`resolve_target`, `stop_all`) — and `status(session=…)`
        // reveals strictly LESS than this does, since `human_summary` has no
        // prompt in it while `render_list_line` prints the session key and the
        // first 60 chars of the watch prompt. A chat-tier channel asking "what
        // loops are running?" still gets a true answer about its own session,
        // plus an honest count so it never reads as "nothing is running".
        let hidden = if Self::caller_is_operator() {
            0
        } else {
            let before = loops.len();
            loops.retain(|l| l.session_id == session);
            before - loops.len()
        };
        if loops.is_empty() {
            return Ok(LoopOutput {
                success: true,
                message: match hidden {
                    0 => "No timer loops in any session.".to_string(),
                    n => format!(
                        "No timer loops in this session. {n} loop(s) in other \
                         sessions are not shown at this permission level."
                    ),
                },
            });
        }
        // Newest-started first so the most recent watch leads. A loop has no
        // `updated_at`, so `created_at_ms` is its only ordering key (goal sorts
        // by `updated_at_ms`; the intent — most-relevant-first — is the same).
        loops.sort_unstable_by_key(|l| std::cmp::Reverse(l.created_at_ms));
        let now = now_ms();
        let mut message = format!("Timer loops ({}):\n", loops.len());
        for state in &loops {
            message.push_str(&Self::render_list_line(state, session, now));
            message.push('\n');
        }
        if hidden > 0 {
            message.push_str(&format!(
                "({hidden} loop(s) in other sessions are not shown at this permission level.)\n"
            ));
        }
        Ok(LoopOutput {
            success: true,
            message: message.trim_end().to_string(),
        })
    }

    /// One compact line per loop for `list` — mirrors `GoalTool::render_list_line`:
    /// status, the watch prompt (truncated), a `(this session)` flag, cadence,
    /// ticks/cap, and the stop reason when stopped. UTF-8-safe truncation (P7).
    ///
    /// The session key leads every line for loops that are NOT the current one.
    /// It is the handle `action='stop'`/`'pause'` take in `session=`: listing a
    /// loop the caller then has no way to name is how "visible but unstoppable"
    /// happened in the first place. Omitted for the current session, which
    /// needs no key and where the `(this session)` flag already says so.
    fn render_list_line(state: &LoopState, current_session: &str, now_ms: u64) -> String {
        let here = if state.session_id == current_session {
            " (this session)".to_string()
        } else {
            format!(" (session '{}')", state.session_id)
        };
        let status = state.status.as_str();
        // A watch prompt can be a paragraph — keep the list line compact.
        let prompt: String = state.prompt.chars().take(60).collect();
        let ellipsis = if state.prompt.chars().count() > 60 {
            "…"
        } else {
            ""
        };
        let mut s = format!(
            "- [{status}] {prompt}{ellipsis}{here} | {}",
            state.cadence.describe()
        );
        match state.max_iterations {
            Some(max) => s.push_str(&format!(" | ticks {}/{max}", state.iterations_used)),
            None => s.push_str(&format!(" | ticks {}", state.iterations_used)),
        }
        if let Some(deadline) = state.deadline_ms {
            if now_ms != 0 && deadline > now_ms {
                s.push_str(&format!(
                    " | time left {}",
                    crate::looping::types::fmt_duration_ms(deadline - now_ms)
                ));
            }
        }
        // Explain a stopped loop; the status tag already says "[stopped]", so
        // the reason adds the "why" without repeating the "what".
        if !state.is_active() {
            if let Some(reason) = &state.stop_reason {
                s.push_str(&format!(" | {reason}"));
            }
        }
        s
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
        // it. Tell the user to start a fresh loop instead. A PAUSED loop is
        // updatable: adjusting caps or cadence while the watch is held is what
        // pause is for, and `resume` picks the new values up.
        if !state.is_adjustable() {
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
        let paused = state.is_paused();
        // Reject "born dead" zero caps at the boundary (same guard as `start`).
        reject_zero_cap(args.max_iterations, "max_iterations")?;
        reject_zero_cap(args.timeout_minutes, "timeout_minutes")?;
        reject_zero_budget(args.token_budget)?;
        // Re-pace a Fixed loop (or convert model-paced → fixed) without a
        // stop/start cycle. `with_cadence` clears any stale next_wake.
        if let Some(i) = &args.interval {
            state = state.with_cadence(Cadence::Fixed {
                interval_ms: parse_interval_ms(i)?,
            });
        }
        if let Some(nw) = &args.next_wake {
            // `next_wake` only paces a model-paced loop — `tick_delay_ms` ignores
            // it for Fixed cadence. Storing it on a Fixed loop would silently do
            // nothing while reporting "Loop updated" (the misleading no-op the
            // 2026-06-17 honesty pass set out to kill). Reject and guide instead.
            if !matches!(state.cadence, Cadence::ModelPaced { .. }) {
                return Ok(LoopOutput {
                    success: false,
                    message: "next_wake only re-paces a model-paced loop; this loop \
                         runs on a fixed cadence. Pass `interval` (e.g. '10m') to \
                         change a fixed loop's pace, or start a model-paced loop \
                         (omit `interval` on start)."
                        .to_string(),
                });
            }
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
        // A pacing or target change supersedes the tick already in flight:
        // its prompt and delay were captured at claim time, so without this
        // the change would silently wait out the OLD wake (hours, for a slow
        // loop) before taking effect. The in-flight tick's pending marker is
        // cleared so its confirm_fire mismatches (it skips), and this very
        // run's completion re-claims a fresh tick from the updated state.
        // Cap-only updates keep the in-flight tick and its schedule — except
        // `timeout_minutes`, which re-bases the watch WINDOW: the in-flight
        // tick's wake was projected against the OLD deadline, and only a fresh
        // claim re-runs `pursuit::fires_out_of_bounds` against the new one. So
        // a shortened window would otherwise still let the already-claimed tick
        // execute past it.
        let reschedule = args.interval.is_some()
            || args.next_wake.is_some()
            || args.timeout_minutes.is_some()
            || args.prompt.as_deref().is_some_and(|p| !p.trim().is_empty());
        // Commit atomically: `state` was built from a `get` snapshot taken
        // above, but a concurrent tick fire/re-arm may have moved the LIVE
        // pending marker since. `commit_field_update` re-reads it under the
        // registry lock and preserves it (or clears it when `reschedule`),
        // so a cap-only update never resurrects a stale pending and stalls
        // the loop — the tick pipeline stays the sole owner of that field.
        if !self.registry.commit_field_update(state, reschedule) {
            // The loop was stopped between our read and this write (a
            // concurrent stop / cap-exhaustion won the race). Report honestly
            // rather than claiming an update that did not land.
            return Ok(LoopOutput {
                success: false,
                message: "Loop is no longer active (it was stopped while updating); \
                     call loop(action='start') to begin a new one."
                    .to_string(),
            });
        }
        Ok(LoopOutput {
            success: true,
            message: match (paused, reschedule) {
                // A paused loop has no in-flight tick to reschedule; the new
                // values simply apply when it resumes. Saying "rescheduled from
                // now" there would promise a tick that cannot come.
                (true, _) => "Loop updated; it stays paused until \
                     loop(action='resume')."
                    .to_string(),
                (false, true) => "Loop updated; the next tick is rescheduled from now.".to_string(),
                (false, false) => "Loop updated.".to_string(),
            },
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
            env_summary: crate::strategy::planner::env_summary(),
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
         tick to set the next delay). The FIRST tick fires one cadence AFTER the \
         turn that starts the loop, never immediately — if the user wants a \
         check right away, do it in this same turn. action='update' also re-paces a running \
         loop in place — pass `interval` to change a fixed cadence, or \
         `prompt`/`timeout_minutes`/`max_iterations` to re-target or re-bound it \
         without stop/start. action='pause' holds the watch without losing it \
         (tick count, caps and cadence survive; nothing fires until \
         action='resume' in that session) — prefer it over stop when the user \
         says 'hold off'/'pause', since stop is terminal. action='status' \
         reports THIS session's loop; action='list' shows every timer loop \
         across ALL sessions (use it to answer 'what loops are running?', since \
         status only sees the current session). To act on a loop `list` shows in \
         ANOTHER session, pass session='<key as list prints it>' — accepted by \
         status/stop/pause only, and only for operators; start/resume/update \
         must run in the loop's own session. action='stop_all' is the kill \
         switch for 'stop every loop' (operator only). Optional safety caps: \
         max_iterations, timeout_minutes. If a \
         tick finds it is blocked on slow external work (a rate-limit cooldown, a \
         long build), defer the next check instead of busy-ticking or stopping — \
         on a fixed loop update to a longer `interval`, on a model-paced loop set \
         a larger `next_wake` (or hand the wait to `goal`, which can park until an \
         exact event). Use for watch/poll duties (e.g. 'every 5 minutes check the \
         deploy and tell me if it changed').";

    type Args = LoopArgs;
    type Output = LoopOutput;

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
                session: None,
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
            session: None,
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
    async fn start_model_paced_honors_next_wake() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            action: LoopAction::Start,
            interval: None,
            prompt: Some("watch CI".to_string()),
            max_iterations: Some(5),
            timeout_minutes: None,
            token_budget: None,
            next_wake: Some("2m".to_string()),
            session: None,
        })
        .await
        .unwrap();
        let st = reg.get("s").unwrap();
        assert!(
            st.next_wake_ms.is_some(),
            "first wake seeded from next_wake instead of the 10m fallback"
        );
    }

    #[tokio::test]
    async fn start_next_wake_on_fixed_cadence_is_rejected() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        let res = tool
            .run(LoopArgs {
                action: LoopAction::Start,
                interval: Some("5m".to_string()),
                prompt: Some("p".to_string()),
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: Some("2m".to_string()),
                session: None,
            })
            .await;
        assert!(res.is_err(), "contradictory next_wake+interval must reject");
        assert!(reg.get("s").is_none(), "no loop registered on rejection");
    }

    #[tokio::test]
    async fn start_confirmation_is_honest_about_cadence_cap_and_replacement() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        let args = |prompt: &str| LoopArgs {
            action: LoopAction::Start,
            interval: Some("5m".to_string()),
            prompt: Some(prompt.to_string()),
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
            session: None,
        };
        let out = tool.run(args("watch deploy")).await.unwrap();
        assert!(out.message.contains("every 5m"), "{}", out.message);
        assert!(
            out.message
                .contains(&DEFAULT_SOFT_MAX_ITERATIONS.to_string()),
            "silently applied default cap must be disclosed: {}",
            out.message
        );
        assert!(!out.message.contains("replacing"), "{}", out.message);
        // Starting again over the still-active loop must say so.
        let out2 = tool.run(args("watch staging")).await.unwrap();
        assert!(out2.message.contains("replacing"), "{}", out2.message);
        assert_eq!(reg.get("s").unwrap().prompt, "watch staging");
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
            session: None,
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
            session: None,
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
            session: None,
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
    async fn update_pacing_supersedes_in_flight_tick_but_cap_update_keeps_it() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(
            crate::looping::LoopState::new(
                "s",
                "p",
                crate::looping::Cadence::Fixed {
                    interval_ms: 3_600_000,
                },
                0,
            )
            .with_pending_tick(Some(99_999)),
        );
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        // Cap-only update: the scheduled tick keeps its slot.
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Update,
                interval: None,
                prompt: None,
                max_iterations: Some(9),
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(!out.message.contains("rescheduled"), "{}", out.message);
        assert_eq!(reg.get("s").unwrap().pending_tick_wake_ms, Some(99_999));
        // Re-pacing supersedes the in-flight tick (it would otherwise sleep
        // out the old 1h wake before the new cadence applied).
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Update,
                interval: Some("2m".to_string()),
                prompt: None,
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: None,
                session: None,
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(out.message.contains("rescheduled"), "{}", out.message);
        assert!(reg.get("s").unwrap().pending_tick_wake_ms.is_none());
    }

    #[tokio::test]
    async fn update_next_wake_on_fixed_loop_is_rejected_honestly() {
        // Setting next_wake on a Fixed-cadence loop is a silent no-op
        // (tick_delay_ms ignores it). The tool must refuse rather than claim
        // success and store dead state.
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
        let out = tool
            .run(LoopArgs {
                action: LoopAction::Update,
                interval: None,
                prompt: None,
                max_iterations: None,
                timeout_minutes: None,
                token_budget: None,
                next_wake: Some("8m".to_string()),
                session: None,
            })
            .await
            .unwrap();
        assert!(!out.success, "next_wake on a fixed loop must not succeed");
        assert!(out.message.contains("interval"), "{}", out.message);
        // State untouched: no next_wake stored.
        assert!(reg.get("s").unwrap().next_wake_ms.is_none());
    }

    #[tokio::test]
    async fn start_rejects_zero_caps() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        for (max_it, timeout) in [(Some(0), None), (None, Some(0))] {
            let res = tool
                .run(LoopArgs {
                    action: LoopAction::Start,
                    interval: Some("5m".to_string()),
                    prompt: Some("p".to_string()),
                    max_iterations: max_it,
                    timeout_minutes: timeout,
                    token_budget: None,
                    next_wake: None,
                    session: None,
                })
                .await;
            assert!(res.is_err(), "zero cap must be rejected");
        }
        // No "born dead" loop was registered.
        assert!(reg.get("s").is_none());
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
            session: None,
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
            session: None,
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
                session: None,
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
                session: None,
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
                session: None,
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
            session: None,
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
                session: None,
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
        crate::strategy::set_global_for_test(crate::sync_primitives::Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
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
                session: None,
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

    #[tokio::test]
    async fn stop_deletes_loop_keyed_strategy_but_not_goal_keyed() {
        use crate::strategy::{goal_key, loop_key, Strategy, StrategyStore};

        let sdir = tempfile::tempdir().unwrap();
        crate::strategy::set_global_for_test(std::sync::Arc::new(
            StrategyStore::open(&sdir.path().join("s.db")).unwrap(),
        ));
        // set_global_for_test is OnceCell-once; another test in this binary may
        // have set the global first. Seed + assert through the ACTUAL global the
        // tool's stop operates on (the unique session key avoids collision).
        let sstore = crate::strategy::global().expect("strategy global set");

        let concrete = Strategy {
            objective: "o".into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["stay on the watch target".into()],
            success_criteria: "ok".into(),
            goal_id: None,
        };
        sstore.put(&loop_key("sess-loop-stop"), &concrete).unwrap();
        sstore.put(&goal_key("sess-loop-stop"), &concrete).unwrap();

        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(crate::looping::LoopState::new(
            "sess-loop-stop",
            "p",
            crate::looping::Cadence::Fixed { interval_ms: 1000 },
            0,
        ));
        let tool = LoopTool::new(reg.clone()).with_session_for_test("sess-loop-stop");
        tool.run(LoopArgs {
            action: LoopAction::Stop,
            interval: None,
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
            session: None,
        })
        .await
        .unwrap();

        // Loop stop removes the loop-keyed strategy...
        assert!(sstore.get(&loop_key("sess-loop-stop")).unwrap().is_none());
        // ...but leaves a co-existing goal-keyed strategy untouched.
        assert!(sstore.get(&goal_key("sess-loop-stop")).unwrap().is_some());
    }

    /// Regression: the shared registry handle is process-global and rewritten
    /// at every run start, so a concurrent run of another agent can overwrite
    /// it mid-turn. The turn's `TURN_CONTEXT` task-local must win so
    /// `loop(start)` binds to the run that actually made the call; the handle
    /// stays as the fallback for non-scoped paths.
    #[tokio::test]
    async fn session_prefers_turn_context_over_shared_handle() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let handle = Arc::new(RwLock::new("concurrent-run-session".to_string()));
        let tool = LoopTool::new(reg).with_session_key_handle(Some(handle));

        let run_key = SessionKey::main("own-run");
        let turn = TurnContext {
            session_key: run_key.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };
        let bound = TURN_CONTEXT.scope(turn, tool.session()).await;
        assert_eq!(bound, run_key.to_key_string());

        // Outside a scoped turn the shared handle remains the fallback.
        assert_eq!(tool.session().await, "concurrent-run-session");
    }

    fn list_args() -> LoopArgs {
        LoopArgs {
            action: LoopAction::List,
            interval: None,
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
            session: None,
        }
    }

    #[tokio::test]
    async fn list_empty_reports_no_loops() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg).with_session_for_test("sess-x");
        let out = tool.run(list_args()).await.unwrap();
        assert!(out.success);
        assert!(out.message.contains("No timer loops"), "{}", out.message);
    }

    #[tokio::test]
    async fn list_enumerates_all_sessions_and_flags_current() {
        // The R6/R8 gap this closes: a loop set on another channel is invisible
        // to `status` (keyed by the current session) — `list` sees all of them.
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        reg.put(LoopState::new(
            "sess-here",
            "watch the deploy",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            1_000,
        ));
        reg.put(
            LoopState::new(
                "sess-elsewhere",
                "triage the PR queue",
                Cadence::ModelPaced {
                    fallback_ms: 600_000,
                },
                2_000,
            )
            .with_status(LoopStatus::Stopped)
            .with_stop_reason(Some(
                "Loop stopped: reached the iteration cap (20 ticks).".into(),
            )),
        );
        let tool = LoopTool::new(reg).with_session_for_test("sess-here");
        let out = tool.run(list_args()).await.unwrap();
        assert!(out.success);
        assert!(out.message.contains("Timer loops (2)"), "{}", out.message);
        // The current session's loop is flagged; both loops are visible.
        assert!(out.message.contains("watch the deploy (this session)"));
        assert!(out.message.contains("triage the PR queue"));
        // Status tags + the stop reason for the stopped one.
        assert!(out.message.contains("[active]"));
        assert!(out.message.contains("[stopped]"));
        assert!(out.message.contains("iteration cap"), "{}", out.message);
        // The other session's loop is NOT flagged as current.
        assert!(
            !out.message
                .lines()
                .any(|l| l.contains("triage the PR queue") && l.contains("(this session)")),
            "{}",
            out.message
        );
    }

    // ---- lifecycle (pause / resume) + cross-session control -----------------

    fn mk(action: LoopAction) -> LoopArgs {
        LoopArgs {
            action,
            interval: None,
            prompt: None,
            max_iterations: None,
            timeout_minutes: None,
            token_budget: None,
            next_wake: None,
            session: None,
        }
    }

    async fn started(
        reg: &std::sync::Arc<crate::looping::LoopRegistry>,
        session: &str,
    ) -> LoopTool {
        let tool = LoopTool::new(reg.clone()).with_session_for_test(session);
        let args = LoopArgs {
            interval: Some("5m".to_string()),
            prompt: Some("watch CI".to_string()),
            ..mk(LoopAction::Start)
        };
        tool.run(args).await.unwrap();
        tool
    }

    #[tokio::test]
    async fn pause_holds_the_watch_and_resume_puts_it_back() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = started(&reg, "s").await;

        let out = tool.run(mk(LoopAction::Pause)).await.unwrap();
        assert!(out.success, "{}", out.message);
        assert!(reg.get_active("s").is_none(), "a paused loop never fires");
        assert!(reg.get("s").unwrap().is_paused());
        // Pausing twice is a reported no-op, not a silent success.
        assert!(!tool.run(mk(LoopAction::Pause)).await.unwrap().success);
        // status still answers, and says paused.
        let st = tool.run(mk(LoopAction::Status)).await.unwrap();
        assert!(st.message.contains("Loop paused"), "{}", st.message);

        let out = tool.run(mk(LoopAction::Resume)).await.unwrap();
        assert!(out.success, "{}", out.message);
        assert!(reg.get_active("s").is_some(), "resume re-arms the watch");
        assert!(!tool.run(mk(LoopAction::Resume)).await.unwrap().success);
    }

    #[tokio::test]
    async fn paused_loop_is_updatable_but_stopped_one_is_not() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = started(&reg, "s").await;
        tool.run(mk(LoopAction::Pause)).await.unwrap();

        let out = tool
            .run(LoopArgs {
                interval: Some("30m".to_string()),
                ..mk(LoopAction::Update)
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        assert!(
            out.message.contains("stays paused"),
            "must not promise a tick it cannot fire: {}",
            out.message
        );
        assert!(reg.get("s").unwrap().is_paused(), "update kept it paused");

        // Stopped stays terminal for both update and resume.
        tool.run(mk(LoopAction::Stop)).await.unwrap();
        assert!(
            !tool
                .run(LoopArgs {
                    interval: Some("1m".to_string()),
                    ..mk(LoopAction::Update)
                })
                .await
                .unwrap()
                .success
        );
        let out = tool.run(mk(LoopAction::Resume)).await.unwrap();
        assert!(!out.success);
        assert!(out.message.contains("start"), "{}", out.message);
    }

    #[tokio::test]
    async fn cross_session_stop_closes_the_visible_but_unstoppable_gap() {
        // `list` has always been cross-session while `stop` was session-local:
        // a loop on another channel could be seen and not ended. It can now.
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let _remote = started(&reg, "other-session").await;
        let here = LoopTool::new(reg.clone()).with_session_for_test("here");

        // The list line must carry the key `stop` takes — otherwise the loop is
        // still unaddressable in conversation.
        let list = here.run(mk(LoopAction::List)).await.unwrap();
        assert!(
            list.message.contains("(session 'other-session')"),
            "{}",
            list.message
        );

        let out = here
            .run(LoopArgs {
                session: Some("other-session".to_string()),
                ..mk(LoopAction::Stop)
            })
            .await
            .unwrap();
        assert!(out.success, "{}", out.message);
        assert!(
            out.message.contains("in session 'other-session'"),
            "a remote effect must never read as a local one: {}",
            out.message
        );
        let remote = reg.get("other-session").unwrap();
        assert!(!remote.is_active());
        assert!(remote
            .stop_reason
            .as_deref()
            .unwrap_or_default()
            .contains("from another session"));
    }

    #[tokio::test]
    async fn unknown_session_key_is_refused_not_silently_a_no_op() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let here = LoopTool::new(reg).with_session_for_test("here");
        let err = here
            .run(LoopArgs {
                session: Some("typo-session".to_string()),
                ..mk(LoopAction::Stop)
            })
            .await
            .unwrap_err();
        assert!(err.contains("list"), "{err}");
    }

    #[tokio::test]
    async fn arming_verbs_refuse_a_remote_session() {
        // A loop only gets its next tick from its OWN session's completion
        // hook, so arming one remotely would leave a dormant Active that
        // status/list misreport as running.
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let _remote = started(&reg, "other-session").await;
        let here = LoopTool::new(reg.clone()).with_session_for_test("here");
        for action in [LoopAction::Resume, LoopAction::Update, LoopAction::Start] {
            let err = here
                .run(LoopArgs {
                    prompt: Some("p".to_string()),
                    interval: Some("5m".to_string()),
                    session: Some("other-session".to_string()),
                    ..mk(action)
                })
                .await
                .unwrap_err();
            assert!(err.contains("own session"), "{err}");
        }
        assert!(
            reg.get_active("other-session").is_some(),
            "a refused remote arm must leave the loop exactly as it was"
        );
    }

    #[tokio::test]
    async fn start_says_so_when_it_replaces_a_paused_loop() {
        // Pause preserves tick count / caps / prompt / cadence — and `start`
        // overwrites the entry unconditionally. Reporting the neutral "Loop
        // started in this session." there destroyed a held watch in silence.
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = started(&reg, "s").await;
        assert!(tool.run(mk(LoopAction::Pause)).await.unwrap().success);

        let out = tool
            .run(LoopArgs {
                interval: Some("10m".to_string()),
                prompt: Some("watch the deploy".to_string()),
                ..mk(LoopAction::Start)
            })
            .await
            .unwrap();
        assert!(out.success);
        assert!(
            out.message.contains("PAUSED"),
            "must name the paused loop it destroyed: {}",
            out.message
        );
    }

    #[tokio::test]
    async fn resume_refuses_a_loop_whose_bound_elapsed_while_paused() {
        // The pause receipt warns that a wall-clock deadline keeps running.
        // Resuming past it used to answer "the next tick is scheduled from now"
        // and then, seconds later, "⏹ Loop stopped: reached its time limit".
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(reg.clone()).with_session_for_test("s");
        tool.run(LoopArgs {
            interval: Some("5m".to_string()),
            prompt: Some("watch CI".to_string()),
            timeout_minutes: Some(1),
            ..mk(LoopAction::Start)
        })
        .await
        .unwrap();
        assert!(tool.run(mk(LoopAction::Pause)).await.unwrap().success);
        // Rewind the deadline into the past to simulate a long pause.
        let expired = reg.get("s").unwrap().with_deadline_ms(Some(1));
        assert!(reg.commit_field_update(expired, false));

        let out = tool.run(mk(LoopAction::Resume)).await.unwrap();
        assert!(!out.success, "{}", out.message);
        assert!(out.message.contains("time limit"), "{}", out.message);
        assert!(out.message.contains("update"), "must name the escape hatch");
        assert!(
            reg.get("s").unwrap().is_paused(),
            "a refused resume must not flip the status"
        );
    }

    #[tokio::test]
    async fn list_is_scoped_to_the_caller_session_below_operator() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let _elsewhere = started(&reg, "operator-session").await;
        let here = started(&reg, "here").await;
        let guest = TurnContext {
            session_key: SessionKey::main("a"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "c".to_string(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };

        let out = TURN_CONTEXT
            .scope(guest, here.run(mk(LoopAction::List)))
            .await
            .unwrap();
        assert!(
            !out.message.contains("operator-session"),
            "another session's key must not reach a chat-tier caller: {}",
            out.message
        );
        assert!(
            out.message.contains("not shown at this permission level"),
            "must not read as a false zero: {}",
            out.message
        );

        // An operator still sees everything (the R6/R8 answer is preserved).
        let all = here.run(mk(LoopAction::List)).await.unwrap();
        assert!(all.message.contains("operator-session"), "{}", all.message);
    }

    #[tokio::test]
    async fn cross_session_control_requires_operator() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let _remote = started(&reg, "other-session").await;
        let here = LoopTool::new(reg.clone()).with_session_for_test("here");
        let guest = TurnContext {
            session_key: SessionKey::main("a"),
            run_id: String::new(),
            channel_id: "telegram".to_string(),
            conversation_id: "c".to_string(),
            caller_role: Some("guest".to_string()),
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
        };

        let remote_stop = LoopArgs {
            session: Some("other-session".to_string()),
            ..mk(LoopAction::Stop)
        };
        let err = TURN_CONTEXT
            .scope(guest.clone(), here.run(remote_stop))
            .await
            .unwrap_err();
        assert!(err.contains("operator"), "{err}");
        let err = TURN_CONTEXT
            .scope(guest.clone(), here.run(mk(LoopAction::StopAll)))
            .await
            .unwrap_err();
        assert!(err.contains("operator"), "{err}");
        assert!(
            reg.get_active("other-session").is_some(),
            "a chat-tier caller must not reach across the trust boundary"
        );

        // The caller's OWN loop stays fully manageable at chat tier.
        let own = LoopTool::new(reg.clone()).with_session_for_test("other-session");
        assert!(
            TURN_CONTEXT
                .scope(guest, own.run(mk(LoopAction::Stop)))
                .await
                .unwrap()
                .success
        );
    }

    #[tokio::test]
    async fn stop_all_is_the_kill_switch() {
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let a = started(&reg, "a").await;
        let _b = started(&reg, "b").await;
        let _c = started(&reg, "c").await;
        a.run(mk(LoopAction::Pause)).await.unwrap();

        let out = a.run(mk(LoopAction::StopAll)).await.unwrap();
        assert!(out.success, "{}", out.message);
        assert!(out.message.contains('3'), "{}", out.message);
        for s in ["a", "b", "c"] {
            assert!(!reg.get(s).unwrap().is_active(), "{s} still running");
        }
        // Idempotent: a second sweep has nothing left to quiet.
        let out = a.run(mk(LoopAction::StopAll)).await.unwrap();
        assert!(
            out.success && out.message.contains("No running"),
            "{}",
            out.message
        );
    }

    #[tokio::test]
    async fn start_discloses_loops_live_in_other_sessions() {
        // Parallel-collision disclosure: `<timer_loop>` only ever projects THIS
        // session's loop, so without this the model cannot know it is about to
        // duplicate a watch running on another channel.
        let reg = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let _other = started(&reg, "other-session").await;
        let here = LoopTool::new(reg.clone()).with_session_for_test("here");
        let out = here
            .run(LoopArgs {
                interval: Some("5m".to_string()),
                prompt: Some("watch CI".to_string()),
                ..mk(LoopAction::Start)
            })
            .await
            .unwrap();
        assert!(
            out.message.contains("1 other timer loop(s) are live"),
            "{}",
            out.message
        );

        // A lone loop gets no noise.
        let solo = std::sync::Arc::new(crate::looping::LoopRegistry::default());
        let tool = LoopTool::new(solo).with_session_for_test("only");
        let out = tool
            .run(LoopArgs {
                interval: Some("5m".to_string()),
                prompt: Some("p".to_string()),
                ..mk(LoopAction::Start)
            })
            .await
            .unwrap();
        assert!(!out.message.contains("other timer loop"), "{}", out.message);
    }
}
