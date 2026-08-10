//! Free functions that compute per-turn runtime/context prompt fragments
//! (execution plan, standing goal, tool health) for the context layers.

use crate::sync_primitives::Arc;

/// Look up the session's active scratchpad execution list and render a
/// compact, judgment-free progress snapshot for `ExecutionPlanLayer` to
/// inject into the per-turn system prompt. Returns `None` (→ the layer
/// emits nothing) when the session has no bound scratchpad, the file is
/// missing/unreadable, or every plan item is already done — the same
/// `has_pending_work()` gate the stop-verifier uses, so an empty or
/// finished plan never adds noise.
///
/// **Bounded on purpose.** `ExecutionPlanLayer` only passes its input through
/// and is listed in `prompt_contract::CONDITIONALLY_SILENT`, so the per-layer
/// byte ratchet measures it as 0 B no matter how large this gets — a layer that
/// merely forwards content owes its bound in the producer, and this is the
/// producer. `render_progress_bounded` is byte-identical to `render_progress`
/// for an ordinary plan; it only bites on a pathological one.
///
/// Free async function so it can be unit-tested without a full
/// `AgentHarnessRunner`, mirroring `compute_runtime_state_blocks`.
/// Fail-soft on any I/O error: a transient scratchpad read must never
/// wedge prompt assembly.
pub async fn active_execution_plan(session_key: &str) -> Option<String> {
    let snapshot = crate::builtin_tools::scratchpad::session_plan(session_key).await?;
    snapshot
        .has_pending_work()
        .then(|| snapshot.render_progress_bounded())
}

/// Fetch the session's active standing goal as a compact, judgment-free
/// summary for `StandingGoalLayer`. Returns `None` (→ layer emits nothing)
/// when the goal subsystem is uninitialized, the session has no goal, or the
/// goal is not `Active`. Fail-soft on store error. Mirrors `active_execution_plan`.
pub async fn active_standing_goal(session_key: &str) -> Option<String> {
    let store = crate::goal::global()?;
    let goal = store.get(session_key).ok().flatten()?;
    if !goal.is_active() {
        return None;
    }
    Some(render_goal_summary(&goal))
}

/// The wall-clock half of the goal + loop state: the live countdowns, rendered
/// for the **transient trailing user message** rather than the system prompt.
///
/// This exists because of how prompt caching is keyed. The system prompt is the
/// prefix of every message-level cache breakpoint, so a byte that changes on
/// every run — a `Utc::now()`-derived countdown — re-keys the whole conversation
/// prefix each turn: the growing history gets re-WRITTEN to cache (1.25× input)
/// instead of READ (0.1×), a ~12× swing on the prompt-input component of every
/// single turn a loop or standing goal is alive. `active_standing_goal` and
/// `active_timer_loop` used to stamp `Utc::now()` and render `time left: 5m12s`
/// / `next wake: in 43s` / `deadline in ~7m` straight into that prefix.
///
/// The information is not lost — it moves to the far side of the breakpoint,
/// where changing bytes cost only themselves. The model still gets a live
/// countdown every turn. Returns `None` when nothing time-varying is live.
pub async fn live_deadline_status(session_key: &str) -> Option<String> {
    // One clock stamp for both readers, so the goal's and the loop's countdowns
    // describe the same instant (and match the deadline check the autonomous
    // loop's `should_continue` performs).
    let now_ms = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
    let mut lines = Vec::new();

    if let Some(goal) = crate::goal::global()
        .and_then(|store| store.get(session_key).ok().flatten())
        .filter(crate::goal::Goal::is_active)
    {
        let mut parts = Vec::new();
        if let Some(deadline) = goal.deadline_ms {
            parts.push(render_deadline(deadline, now_ms));
        }
        // The parked countdown rides here for the same reason the deadline
        // countdown does: it changes every run, so it must stay on the far
        // side of the prompt-cache breakpoint.
        if let Some(until) = goal.waiting_until_ms {
            parts.push(render_park_wait(until, now_ms));
        }
        if !parts.is_empty() {
            lines.push(format!("standing goal: {}", parts.join(", ")));
        }
    }

    if let Some(state) = crate::looping::global().and_then(|reg| reg.get_active(session_key)) {
        let live = state.live_status(now_ms);
        if !live.is_empty() {
            lines.push(format!("timer loop: {}", live.join(", ")));
        }
    }

    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

/// Fetch the session's active timer loop as a compact summary (watch
/// prompt + status) for `TimerLoopLayer`. The clock-gated sibling of
/// `active_standing_goal`: between ticks the model had no idea a watch was
/// running in this session (the tick reminder exists only inside tick
/// turns), so on an ordinary user turn it could neither report on the loop
/// nor avoid starting a duplicate. Returns `None` (→ layer emits nothing)
/// when the loop subsystem is uninitialized or the session has no Active
/// loop. `async` for uniformity with its `tokio::join!` siblings.
pub async fn active_timer_loop(session_key: &str) -> Option<String> {
    let reg = crate::looping::global()?;
    let state = reg.get_active(session_key)?;
    // `stable_summary`, not `human_summary`: this lands in the system prompt,
    // which must not carry wall-clock-derived bytes. The live countdown rides
    // the transient tail instead — see `live_deadline_status`.
    Some(format!("{} ({})", state.prompt, state.stable_summary()))
}

/// Fetch the session's welded Strategy for the prompt weld. Returns the
/// `Strategy` struct (the caller renders both the `<strategy>` body and the
/// guardrail echo from it). Thin async wrapper over `resolve_active_strategy`,
/// which resolves the StraTA composite key with precedence
/// goal → loop → team → session (see its doc for the tier rules). Returns
/// `None` (→ both Strategy layers emit nothing) when the strategy subsystem is
/// uninitialized or no Strategy is stored for any tier. Fail-soft on store
/// error. Mirrors `active_standing_goal`.
pub async fn active_strategy(session_key: &str) -> Option<crate::strategy::Strategy> {
    let store = crate::strategy::global()?;
    resolve_active_strategy(
        &store,
        session_key,
        goal_tier_live(session_key),
        loop_tier_live(session_key),
    )
}

/// Is the goal-keyed Strategy tier live for this session?
///
/// The goal-welded plan is minted at `goal set` and deleted at every *terminal*
/// transition — but `Paused` is not terminal, so a paused goal's plan kept
/// winning the top strategy tier and steering every unrelated turn, while the
/// goal itself vanished from the prompt (`active_standing_goal` surfaces only
/// `Active`). The user saw an agent silently executing a plan for work they had
/// explicitly paused. Gating the tier on the goal's own status keeps the plan on
/// disk — a resume gets it back, unlike deleting it — while it stops steering.
/// No goal row at all → the tier is live (a plan welded without a goal, or one
/// awaiting the gate, behaves exactly as before).
fn goal_tier_live(session_key: &str) -> bool {
    let Some(store) = crate::goal::global() else {
        return true;
    };
    match store.get(session_key) {
        Ok(Some(goal)) => goal.status != crate::goal::GoalStatus::Paused,
        // Missing row / unreadable → do not change the pre-existing behaviour.
        Ok(None) | Err(_) => true,
    }
}

/// Is the loop-keyed Strategy tier live for this session? The loop sibling of
/// [`goal_tier_live`] (goal hardening ⑦, never mirrored until round 4).
///
/// The loop-welded plan is persistent SQLite, but the loop registry is process
/// memory — a daemon restart clears every loop *by design* while the weld row
/// survives with no owner and no remaining lifecycle site that could ever
/// delete it (every clear is gated on live registry state or a tool verb that
/// finds one). Without this gate the orphan plan silently steers every later
/// turn of the resumed session. Gating on registry *presence* (any status —
/// the tool `stop` and both gateway stop paths already delete the weld, so a
/// present-but-stopped loop briefly keeping the tier live is harmless) keeps
/// the row on disk while it stops steering.
///
/// `Paused` is the one status that must be excluded explicitly, exactly as
/// [`goal_tier_live`] excludes `GoalStatus::Paused`. A pause deliberately does
/// NOT delete the weld — the plan is still the plan when the loop resumes — and
/// unlike the momentary stopped-but-present window, a pause can last hours. Left
/// live, the held loop's plan would keep steering every ordinary turn of a
/// session whose watch the user explicitly put down. Registry uninitialized (host
/// tests, early boot) → fail open, matching `goal_tier_live`.
fn loop_tier_live(session_key: &str) -> bool {
    match crate::looping::global() {
        Some(reg) => reg.get(session_key).is_some_and(|l| !l.is_paused()),
        None => true,
    }
}

/// Resolve the welded Strategy for a session key against an explicit store
/// (sync; the global accessor lives in `active_strategy`, keeping this
/// unit-testable). Precedence: goal → loop → **team** → session. The team tier
/// fires only for a `team_chat` Task key — it recovers the (already-normalized)
/// team id and reads the leader-minted team-wide row, so every member welds the
/// same plan (strategy round 2). Above `session_key` so a member's own
/// `/goal`/`/loop` still wins; below `loop_key` for the same reason.
fn resolve_active_strategy(
    store: &crate::strategy::StrategyStore,
    session_key: &str,
    goal_tier_live: bool,
    loop_tier_live: bool,
) -> Option<crate::strategy::Strategy> {
    if goal_tier_live {
        if let Some(s) = store
            .get(&crate::strategy::goal_key(session_key))
            .ok()
            .flatten()
        {
            return Some(s);
        }
    }
    if loop_tier_live {
        if let Some(s) = store
            .get(&crate::strategy::loop_key(session_key))
            .ok()
            .flatten()
        {
            return Some(s);
        }
    }
    if let Some(crate::routing::session_key::SessionKey::Task {
        task_type, task_id, ..
    }) = crate::routing::session_key::SessionKey::parse(session_key)
    {
        if task_type == crate::teams::run_mode::TEAM_CHAT_TASK_TYPE {
            if let Some(s) = store
                .get(&crate::strategy::team_key(&task_id))
                .ok()
                .flatten()
            {
                return Some(s);
            }
        }
    }
    store
        .get(&crate::strategy::session_key(session_key))
        .ok()
        .flatten()
}

/// Format the active-goal summary line injected as `<standing_goal>`. Pure and
/// **clock-free**: every byte is a function of the goal alone, so two prompt
/// builds a minute apart are byte-identical and the conversation's cache prefix
/// survives (see [`live_deadline_status`] for why that matters, and for where
/// the countdown went).
///
/// Surfaces the objective plus the structural backstops the autonomous loop
/// enforces — token budget, iteration pace, and *whether* a deadline exists — so
/// the model can pace itself against each one (R9 — intelligence in the prompt).
/// The remaining *time* is the one thing that cannot live here; it is delivered
/// every turn on the transient tail instead, so nothing is hidden from the model.
///
/// The counters (`continuations_used`) do change between runs, but only when the
/// goal actually advances — a real state change that should re-key the prefix.
pub(crate) fn render_goal_summary(goal: &crate::goal::Goal) -> String {
    let budget = match goal.token_budget {
        Some(b) => format!(", budget={b}"),
        None => String::new(),
    };
    let deadline = match goal.deadline_ms {
        Some(_) => ", deadline set",
        None => "",
    };
    let pursuit = match goal.pursuit {
        crate::goal::PursuitMode::Active { max_iterations } => {
            format!(
                ", autonomous iteration {}/{}",
                goal.continuations_used, max_iterations
            )
        }
        crate::goal::PursuitMode::Passive => String::new(),
    };
    // The parked FACT (stable for the whole park) belongs here; the parked
    // COUNTDOWN is clock-derived and rides `live_deadline_status` on the far
    // side of the cache breakpoint. Without this the model reads a parked
    // pursuit as actively running every single turn — the tool's own `get`
    // and `list` renders both say it, and this is the one injected per turn.
    let parked = match (
        goal.waiting_on_task.as_deref(),
        goal.waiting_until_ms.is_some(),
    ) {
        (Some(task_id), _) => format!(", parked (waiting on task '{task_id}')"),
        (None, true) => ", parked (waiting)".to_string(),
        (None, false) => String::new(),
    };
    format!(
        "{} (status=active{budget}{deadline}{pursuit}{parked})",
        goal.objective
    )
}

/// Render a wall-clock deadline (absolute Unix epoch ms) as a compact
/// remaining-time phrase relative to `now_ms`. `now_ms == 0` (no clock
/// available, mirroring the loop's clock-less convention) degrades to a bare
/// "deadline set" rather than a misleading countdown; an already-passed
/// deadline reads "deadline passed" (the loop Blocks the goal on its next hook).
pub(crate) fn render_deadline(deadline_ms: u64, now_ms: u64) -> String {
    if now_ms == 0 {
        return "deadline set".to_string();
    }
    if now_ms >= deadline_ms {
        return "deadline passed".to_string();
    }
    let remaining_s = (deadline_ms - now_ms) / 1000;
    if remaining_s < 60 {
        format!("deadline in ~{remaining_s}s")
    } else if remaining_s < 3600 {
        format!("deadline in ~{}m", remaining_s / 60)
    } else {
        format!(
            "deadline in ~{}h{}m",
            remaining_s / 3600,
            (remaining_s % 3600) / 60
        )
    }
}

/// Render a wait barrier's remaining park as a compact phrase relative to
/// `now_ms`. Sibling of [`render_deadline`], same conventions: `now_ms == 0`
/// (no clock) degrades rather than lying, and an elapsed wake reads as due.
pub(crate) fn render_park_wait(until_ms: u64, now_ms: u64) -> String {
    if now_ms == 0 {
        return "parked, wake time unknown".to_string();
    }
    if now_ms >= until_ms {
        return "parked, wake due".to_string();
    }
    let remaining_s = (until_ms - now_ms) / 1000;
    if remaining_s < 60 {
        format!("parked, ~{remaining_s}s left")
    } else if remaining_s < 3600 {
        format!("parked, ~{}m left", remaining_s / 60)
    } else {
        format!(
            "parked, ~{}h{}m left",
            remaining_s / 3600,
            (remaining_s % 3600) / 60
        )
    }
}

/// Fraction of the in-loop compaction point (`token_budget * warning_threshold`)
/// at which the context-pressure reminder starts firing. 0.85 gives the model
/// lead time to checkpoint before the sensor summarizes older turns away.
pub(crate) const CONTEXT_PRESSURE_REMINDER_LEAD: f64 = 0.85;

/// Render the context-window pressure line for the transient trailing message:
/// how full the model's context is, as a heads-up to wrap up or checkpoint
/// before the in-loop sensor compacts older turns away. Pure and clock-free —
/// like [`render_deadline`] it belongs on the far side of the prompt-cache
/// breakpoint (see [`live_deadline_status`] for why), so it never re-keys the
/// conversation prefix. R9: the model self-paces on the number; the harness
/// still owns the actual compaction. The producer
/// (`AgentHarnessRunner::context_pressure_reminder`) computes the token figures
/// and only calls this once the reminder threshold is crossed.
pub(crate) fn render_context_pressure(used_tokens: u64, window_tokens: u64) -> String {
    let pct = used_tokens
        .saturating_mul(100)
        .checked_div(window_tokens)
        .unwrap_or(0)
        .min(100);
    format!(
        "context window: ~{used_tokens} of ~{window_tokens} tokens in use (~{pct}%). \
         Approaching this model's context limit — older turns will be summarized \
         away soon. Finish, or checkpoint anything important now, so it is not lost."
    )
}

/// Snapshot the tool catalog's `ToolHealthCache` and convert every
/// currently-cached `Unhealthy` entry into a `RuntimeStateFragment` for
/// `ToolRuntimeStateLayer` to render. Returns `vec![]` when
/// `tool_catalog` is `None` (test / early-boot).
///
/// Free function so unit tests can exercise the conversion without
/// constructing a full `AgentHarnessRunner`.
#[must_use]
pub fn compute_runtime_state_blocks(
    tool_catalog: Option<&Arc<crate::tool_metadata::ToolCatalog>>,
) -> Vec<crate::tools::runtime_state::RuntimeStateFragment> {
    let Some(registry) = tool_catalog else {
        return Vec::new();
    };
    let snapshot = registry.health().snapshot();
    // Coalesce unhealthy tools by reason: a single downed dependency — an MCP
    // server exposing many tools, or the whole `browser_*` family when no
    // browser runtime exists — collapses to ONE hint instead of flooding the
    // prompt with a near-identical line per tool. Groups are keyed by the
    // reason's short label (server-id-qualified for MCP, capability-specific
    // for generation, so genuinely distinct dependencies stay separate) and
    // sorted for deterministic output. A single-tool group keeps its exact
    // name, so existing one-tool-per-reason behaviour is byte-identical.
    let mut by_reason: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (name, reason) in snapshot.unhealthy_iter() {
        by_reason
            .entry(reason.short_label())
            .or_default()
            .push(name);
    }
    by_reason
        .into_iter()
        .map(|(reason, mut tools)| {
            tools.sort_unstable();
            let label = match tools.as_slice() {
                [] => String::new(),
                [single] => (*single).to_string(),
                many => format!("{} (+{} more)", many[0], many.len() - 1),
            };
            crate::tools::runtime_state::RuntimeStateFragment::unavailable(label, reason)
        })
        .collect()
}

#[cfg(test)]
mod active_strategy_tests {
    use super::*;

    #[tokio::test]
    async fn returns_none_when_store_uninitialized() {
        let out = active_strategy("session-with-no-store").await;
        assert!(out.is_none());
    }

    fn mk_strategy(objective: &str) -> crate::strategy::Strategy {
        crate::strategy::Strategy {
            objective: objective.into(),
            approach: "a".into(),
            phases: vec![],
            guardrails: vec!["g".into()],
            success_criteria: "s".into(),
            goal_id: None,
        }
    }

    #[test]
    fn resolve_active_strategy_team_tier_and_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::strategy::StrategyStore::open(&dir.path().join("s.db")).unwrap();
        // A team-chat member session key: agent:alice:team_chat:squad
        let sk = crate::routing::session_key::SessionKey::task("alice", "team_chat", "squad")
            .to_key_string();

        // team tier resolves the team-wide row
        store
            .put(
                &crate::strategy::team_key("squad"),
                &mk_strategy("team-obj"),
            )
            .unwrap();
        assert_eq!(
            resolve_active_strategy(&store, &sk, true, true).map(|s| s.objective),
            Some("team-obj".to_string())
        );

        // a member's own /goal still wins over the team frame
        store
            .put(&crate::strategy::goal_key(&sk), &mk_strategy("goal-obj"))
            .unwrap();
        assert_eq!(
            resolve_active_strategy(&store, &sk, true, true).map(|s| s.objective),
            Some("goal-obj".to_string())
        );

        // …but a PAUSED goal takes its tier out of the running: the plan stays on
        // disk (a resume gets it back) yet stops steering unrelated turns. The
        // team frame below it wins instead.
        assert_eq!(
            resolve_active_strategy(&store, &sk, false, true).map(|s| s.objective),
            Some("team-obj".to_string())
        );

        // a non-team session never hits the team tier
        assert!(resolve_active_strategy(&store, "agent:bob:main", true, true).is_none());
    }

    #[test]
    fn dead_loop_tier_stops_steering_but_keeps_the_row() {
        // LOOP-01: the loop weld is persistent SQLite while the loop registry is
        // process memory — after a daemon restart the weld has no owner. A dead
        // loop tier must fall through (here to the session tier), exactly like
        // the paused-goal gate above, without deleting the row.
        let dir = tempfile::tempdir().unwrap();
        let store = crate::strategy::StrategyStore::open(&dir.path().join("s.db")).unwrap();
        let sk = "agent:alice:main";
        store
            .put(&crate::strategy::loop_key(sk), &mk_strategy("loop-obj"))
            .unwrap();
        store
            .put(&crate::strategy::session_key(sk), &mk_strategy("sess-obj"))
            .unwrap();

        // Live loop → loop tier wins.
        assert_eq!(
            resolve_active_strategy(&store, sk, true, true).map(|s| s.objective),
            Some("loop-obj".to_string())
        );
        // Dead loop (registry empty after restart) → tier skipped, row intact.
        assert_eq!(
            resolve_active_strategy(&store, sk, true, false).map(|s| s.objective),
            Some("sess-obj".to_string())
        );
        assert!(
            store.get(&crate::strategy::loop_key(sk)).unwrap().is_some(),
            "gating must not delete the welded row"
        );
    }
}

#[cfg(test)]
mod context_pressure_tests {
    use super::render_context_pressure;

    #[test]
    fn reports_used_window_and_percentage() {
        let s = render_context_pressure(85_000, 100_000);
        assert!(s.contains("~85000 of ~100000"), "{s}");
        assert!(s.contains("~85%"), "{s}");
        assert!(s.contains("checkpoint"), "{s}");
    }

    #[test]
    fn clamps_over_full_to_100_and_guards_zero_window() {
        // Usage past the window never reads over 100%.
        assert!(render_context_pressure(120, 100).contains("~100%"));
        // A zero window (the caller guards against it) must not divide by zero.
        assert!(render_context_pressure(50, 0).contains("~0%"));
    }
}
