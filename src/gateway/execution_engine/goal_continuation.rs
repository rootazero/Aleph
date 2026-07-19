//! Goal continuation — the driver behind a standing goal's autonomous pursuit.
//!
//! Extracted from `execute.rs` (which now just calls [`post_run`]): the whole
//! post-run decision is ONE atomic store call
//! ([`GoalStore::try_claim_continuation`]), so this module only ever *acts* on a
//! decision — spawn, arbitrate the objective gate, or report a structural stop.
//! It holds no judgment of its own (R7) and lives outside `src/harness/` (R10).
//!
//! Entry points:
//! - [`post_run`] — after every completed run of a session, claim the next
//!   continuation (the clock-gated sibling is the loop's `try_claim_tick`).
//! - [`block_goal_on_failure`] — route a failed continuation run to a `Blocked`
//!   goal + an origin-channel notice.
//! - [`rearm_goal_after_busy`] — a continuation that lost the session run-slot
//!   retries the SAME claimed step instead of burning it.
//!
//! Crash recovery is deliberately NOT here: `ResumeCoordinator` already
//! re-triggers a run interrupted mid-flight at boot, and that run's completion
//! re-enters [`post_run`], so the pursuit chain re-establishes itself. A second
//! boot driver would double-drive the same session.

use std::collections::HashMap;
use std::path::Path;

use tracing::{info, warn};

use super::execute::{notify_origin, spawn_continuation_run, ContinuationKind, OriginRoute};
use super::{ContinuationDeps, ExecutionError};
use crate::gateway::agent_instance::AgentInstance;
use crate::goal::{ContinuationDecision, RearmDecision};
use crate::routing::session_key::SessionKey;
use crate::sync_primitives::Arc;
use crate::verification::stop_hooks::{execute_stop_hooks_arc, StopHookContext};

type SessionManager = Option<Arc<dyn crate::gateway::session_store::SessionStore>>;

/// Wall-clock now (Unix epoch ms); 0 only if the clock predates the epoch.
pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The goal's live cumulative token total — read ONLY when the goal carries a
/// budget, so the common no-budget path touches no session state. `None` → the
/// budget goes unenforced this round (the iteration/deadline caps still bind).
/// With enrolled delegation members (tree budget v1) this is the TREE total:
/// own session plus each member's spend since it joined.
async fn live_tokens(
    session_manager: &SessionManager,
    session_key: &SessionKey,
    goal: &crate::goal::Goal,
) -> Option<u64> {
    goal.token_budget?; // no budget → touch no session state (common path).
    let sm = session_manager.as_ref()?;
    crate::gateway::goal_budget::tree_tokens(sm, goal, session_key).await
}

/// Resolve the session's bound origin channel for a stop/halt notice (R5).
async fn origin_of(agent: &Arc<AgentInstance>, session_key: &SessionKey) -> Option<OriginRoute> {
    let reg = crate::gateway::event_emitter::origin_fanout::channel_registry()?;
    agent
        .origin_route(session_key)
        .await
        .map(|(ch, conv)| (reg, ch, conv))
}

/// Post-run continuation hook for the session's standing goal. Fires after every
/// completed run (user turns included); a session with no goal, a passive goal or
/// a terminal one costs exactly one indexed `SELECT`.
///
/// `policy_meta` is `execute::carry_policy_metadata` of the completing run: the
/// continuation this hook spawns must be no more privileged than the
/// conversation it continues. `workspace` is that run's project root, which is
/// persisted nowhere (`goal::types` does not carry it) — pass it down or the
/// continuation silently falls back to the agent workspace.
pub(super) async fn post_run(
    deps: &ContinuationDeps,
    session_manager: &SessionManager,
    session_key: &SessionKey,
    agent: &Arc<AgentInstance>,
    policy_meta: &HashMap<String, String>,
    workspace: Option<&Path>,
) {
    let Some(store) = crate::goal::global() else {
        return;
    };
    let session = session_key.to_key_string();

    // Cheap pre-read purely to decide whether the (awaited) token read is worth
    // it and whether a gate is even configured for this goal. The authoritative
    // read-decide-write happens inside `try_claim_continuation`.
    let peek = match store.get(&session) {
        Ok(Some(g)) => g,
        Ok(None) => return, // no goal for this session — the common path.
        Err(e) => {
            warn!(error = %e, session = %session, "goal pursuit: goal store lookup failed");
            return;
        }
    };
    let gate_configured = deps.gate.is_some() || peek.gate_command.is_some();
    let tokens = live_tokens(session_manager, session_key, &peek).await;
    let now = now_ms();

    let decision = match store.try_claim_continuation(&session, tokens, now, gate_configured) {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, session = %session,
                "goal pursuit: continuation claim failed; skipping this round");
            return;
        }
    };

    match decision {
        ContinuationDecision::Idle => {
            // A gate-less autonomous completion is an authoritative terminal
            // end the gate arbitration never sees (`awaiting_gate` requires a
            // configured gate, so the claim returns `Idle` for it): clear the
            // welded plan here, or it silently steers every later plain turn
            // of this reused session (the goal tier resolves FIRST). Idempotent
            // — repeated Idle rounds on a Complete goal just re-delete a
            // missing row. The tool side deliberately defers this exact case
            // to the hook (a gate veto must be able to reopen WITH the plan).
            if crate::goal::pursuit::gateless_terminal_complete(&peek, gate_configured) {
                clear_goal_welded_strategy(&session);
                // Victory claimed with no gate to verify it — exactly the
                // moment a paired watcher should look for the cheap win.
                crate::loop_graph::service::notify_goal_settled(&session).await;
            }
        }
        ContinuationDecision::Fire {
            delay_ms,
            wake_ms,
            prompt,
        } => {
            spawn_continuation_run(
                deps.registry.clone(),
                deps.adapter.clone(),
                session_key.clone(),
                session.clone(),
                prompt,
                policy_meta.clone(),
                workspace.map(Path::to_path_buf),
                deps.event_bus.clone(),
                Some(delay_ms),
                ContinuationKind::Goal { wake_ms },
            );
            info!(session = %session, "goal pursuit: enqueued autonomous continuation");
        }
        ContinuationDecision::Exhausted { note } => {
            // Authoritative dormant end — drop the welded plan so it does not
            // bleed into later plain turns of this reused session.
            clear_goal_welded_strategy(&session);
            // A Blocked goal is invisible in the prompt (`active_standing_goal`
            // only surfaces Active), so the most common autonomous ending —
            // hitting a cap — would otherwise be entirely silent (R5).
            notify_origin(
                origin_of(agent, session_key).await.as_ref(),
                format!("⏹ {note}"),
            )
            .await;
            info!(session = %session, note = %note,
                "goal pursuit: cap reached, goal blocked for user guidance");
        }
        ContinuationDecision::AwaitingGate(goal) => {
            // `tokens` is only a real count once the baseline was captured; the
            // Goal type enforces that invariant inside `over_budget`, so passing
            // the raw live total here is safe.
            arbitrate_gate(
                deps,
                session_key,
                &session,
                agent,
                &goal,
                tokens.unwrap_or(0),
                now,
                policy_meta,
                workspace,
            )
            .await;
        }
    }
}

/// Read the gate's aggregate as a completion verdict — FAIL CLOSED.
///
/// `Some(reason)` = the completion claim is rejected. Only a gate that actually
/// ran and returned a clean pass (exit 0 everywhere) confirms completion.
/// Previously the caller only looked at `halt_reason()`/`blocking_reason()`, so
/// every OTHER verdict — [`StopHookVerdict::Error`], which is what a non-0/2/3
/// exit code, a timeout, a signal, a spawn failure, and a shell-metacharacter
/// rejection all produce — read as "no veto" and CONFIRMED the claim. A checker
/// that could not check silently passed the thing it was there to check: a
/// `gate_command` with a `&&` in it (rejected by `shell_safe`) made every goal
/// "verified complete" without a single test ever running.
///
/// The trade-off is deliberate: a flaky gate now costs one more autonomous
/// iteration (the goal reopens with the error as its feedback, and the model can
/// fix or clear the gate) instead of silently certifying unfinished work.
fn gate_veto(result: &crate::verification::stop_hooks::StopHookAggregateResult) -> Option<String> {
    if let Some(r) = result.halt_reason().or_else(|| result.blocking_reason()) {
        return Some(r.to_string());
    }
    let errors = result.errors();
    if errors.is_empty() {
        return None; // clean pass — the only way to confirm completion.
    }
    Some(format!(
        "the objective gate could not be evaluated, so completion cannot be \
         confirmed — {}",
        errors
            .iter()
            .map(|(hook, msg)| format!("{hook}: {msg}"))
            .collect::<Vec<_>>()
            .join("; ")
    ))
}

/// The model self-reported `complete` on an autonomous goal and an objective gate
/// is configured: run it (structured exit codes, zero LLM calls — R7) before
/// accepting completion. Pass → the pursuit truly ends. Veto → the goal reopens
/// with the failure as a lesson and one more continuation carries it (the
/// "Ralph Wiggum rescue"), or, with no runway left, blocks for the user.
///
/// The gate is a shell command (`cargo test`-class), so this awaits for tens of
/// seconds with the session's run slot FREE — the user can chat, and can clear or
/// replace the goal, while it runs. Every write below is therefore a
/// compare-and-swap against the goal we gated (`commit_gate_pass` /
/// `claim_after_gate_veto`, keyed on the goal id + its still-awaiting-gate
/// state). Blindly writing back the pre-gate snapshot used to resurrect a goal
/// the user had just cleared — and spawn an unattended run to pursue it.
#[allow(clippy::too_many_arguments)]
async fn arbitrate_gate(
    deps: &ContinuationDeps,
    session_key: &SessionKey,
    session: &str,
    agent: &Arc<AgentInstance>,
    goal: &crate::goal::Goal,
    tokens_now: u64,
    claim_now_ms: u64,
    policy_meta: &HashMap<String, String>,
    workspace: Option<&Path>,
) {
    let Some(store) = crate::goal::global() else {
        return;
    };
    let Some(gate) = crate::verification::stop_hooks::effective_gate(
        deps.gate.as_ref(),
        goal.gate_command.as_deref(),
    ) else {
        // Unreachable: `awaiting_gate` is only true when one of the two exists.
        warn!(session = %session, "goal pursuit: gate configured but resolved to none; skipping");
        return;
    };
    let hctx = StopHookContext {
        final_text: Some(goal.objective.clone()),
        iterations: goal.continuations_used as usize,
        tool_calls_made: 0,
        stop_reason: "goal_complete_claim".to_string(),
    };
    let result =
        execute_stop_hooks_arc(&gate, &hctx, &tokio_util::sync::CancellationToken::new()).await;
    // The gate ran for a while; re-stamp the clock so deadline checks below see
    // the time it actually consumed (the claim's `now` is now stale).
    let now = if claim_now_ms == 0 { 0 } else { now_ms() };

    let Some(reason) = gate_veto(&result) else {
        // Gate passed → completion confirmed, the pursuit truly ends.
        let confirmed = crate::goal::pursuit::confirm_complete(goal, now);
        match store.commit_gate_pass(goal, &confirmed) {
            Ok(true) => {
                info!(session = %session,
                    "goal pursuit: objective gate passed, goal verified complete");
                clear_goal_welded_strategy(session);
                crate::loop_graph::service::notify_goal_settled(session).await;
            }
            Ok(false) => info!(session = %session,
                "goal pursuit: goal changed while the gate ran; discarding the gate pass"),
            Err(e) => warn!(error = %e, session = %session,
                "goal pursuit: failed to persist gate confirmation"),
        }
        return;
    };

    // Vetoed. Reopen with the failure as feedback — unless a structural cap
    // (iterations / deadline / token budget) is already spent, in which case the
    // reopen path blocks the goal instead of handing it runway it does not have.
    let reopened = crate::goal::pursuit::reopen_after_gate_failure(goal, &reason, tokens_now, now);
    if !reopened.is_active() {
        match store.commit_gate_pass(goal, &reopened) {
            Ok(true) => {
                clear_goal_welded_strategy(session);
                let note = reopened
                    .note
                    .clone()
                    .unwrap_or_else(|| "Autonomous pursuit blocked.".to_string());
                // As silent as any other cap stop without this (R5).
                notify_origin(
                    origin_of(agent, session_key).await.as_ref(),
                    format!("⏹ {note}"),
                )
                .await;
                info!(session = %session,
                    "goal pursuit: objective gate vetoed with no runway left, goal blocked");
            }
            Ok(false) => info!(session = %session,
                "goal pursuit: goal changed while the gate ran; discarding the gate veto"),
            Err(e) => warn!(error = %e, session = %session,
                "goal pursuit: failed to persist gate veto"),
        }
        return;
    }

    // Reopened with runway left → claim one more continuation carrying the gate's
    // objective failure signal (R9: the evidence goes into the prompt).
    let prompt = crate::goal::pursuit::gate_failure_prompt(goal, &reason);
    match store.claim_after_gate_veto(goal, &reopened, now) {
        Ok(Some(wake_ms)) => {
            info!(session = %session,
                "goal pursuit: objective gate vetoed completion, re-running with feedback");
            spawn_continuation_run(
                deps.registry.clone(),
                deps.adapter.clone(),
                session_key.clone(),
                session.to_string(),
                prompt,
                policy_meta.clone(),
                workspace.map(Path::to_path_buf),
                deps.event_bus.clone(),
                Some(0),
                ContinuationKind::Goal { wake_ms },
            );
        }
        Ok(None) => info!(session = %session,
            "goal pursuit: goal changed while the gate ran; discarding the gate veto"),
        Err(e) => warn!(error = %e, session = %session,
            "goal pursuit: failed to claim continuation after gate veto"),
    }
}

/// Clear the goal-welded Strategy for a session (best-effort). Called at every
/// authoritative goal termination — gate-confirmed complete, cap/deadline/budget
/// exhaustion, gate-veto-at-cap, and failure block — so the stale plan neither
/// bleeds into later plain turns of the reused session (the goal tier resolves
/// FIRST in `resolve_active_strategy`) nor blocks a future same-objective
/// `goal set` from re-planning. The tool-side terminations (`goal clear`,
/// self-reported terminal `update`) clear the same key in `builtin_tools/goal.rs`;
/// only Passive-complete and Blocked are cleared there — Active-pursuit
/// completion is gate-arbitrated here so a gate veto can still reopen the goal
/// WITH its plan intact.
pub(super) fn clear_goal_welded_strategy(session: &str) {
    if let Some(strat) = crate::strategy::global() {
        if let Err(e) = strat.delete(&crate::strategy::goal_key(session)) {
            info!(session = %session, error = %e,
                "goal pursuit: failed to clear welded strategy on termination (ignored)");
        }
    }
}

/// A continuation run failed (non-cancellation, non-busy): transition the goal to
/// `Blocked` with the error and notify the origin channel. Without this a
/// transient failure leaves the goal a stuck `Active` with no in-flight run —
/// a silent stall — and `Blocked` goals are invisible in the prompt, so the
/// channel notice is the user's only signal that unattended pursuit halted.
pub(super) async fn block_goal_on_failure(
    session: &str,
    error: &ExecutionError,
    origin: Option<&OriginRoute>,
) {
    let reason: String = format!("{error}").chars().take(300).collect();
    if let Some(store) = crate::goal::global() {
        let note = format!(
            "Autonomous pursuit was halted by an error and blocked for your \
             guidance: {reason}. Review progress, then clear or re-set the \
             goal to continue."
        );
        // Atomic: only blocks a goal still being pursued — never clobbers one the
        // failed run had already marked complete/blocked, nor one the user
        // cleared while the failure landed.
        match store.block_if_active(session, &note, now_ms()) {
            Ok(true) => clear_goal_welded_strategy(session),
            Ok(false) => {} // nothing left to block.
            Err(e) => warn!(error = %e, session = %session,
                "goal pursuit: failed to persist failure block"),
        }
    }
    notify_origin(
        origin,
        format!("⚠️ Autonomous pursuit of your standing goal halted: {reason}"),
    )
    .await;
}

/// A claimed continuation lost the session run-slot (`AgentBusy`) — a scheduling
/// collision, not a pursuit failure. The iteration was already spent at claim
/// time, so re-arm the SAME continuation with a short retry delay instead of
/// dropping it (which burned the iteration for zero work, and stalled the pursuit
/// outright whenever the colliding run then failed — its own hook never runs).
pub(super) async fn rearm_goal_after_busy(
    session: &str,
    origin: Option<&OriginRoute>,
) -> Option<(u64, u64)> {
    let store = crate::goal::global()?;
    match store.rearm_after_busy(session, now_ms()) {
        Ok(RearmDecision::Retry { delay_ms, wake_ms }) => {
            info!(session = %session, delay_ms,
                "goal pursuit: agent busy at continuation fire; re-armed with a retry delay");
            Some((delay_ms, wake_ms))
        }
        Ok(RearmDecision::Exhausted { note }) => {
            clear_goal_welded_strategy(session);
            notify_origin(origin, format!("⏹ {note}")).await;
            info!(session = %session, note = %note,
                "goal pursuit: cap tripped during a busy collision; goal blocked");
            None
        }
        Ok(RearmDecision::Drop) => {
            info!(session = %session,
                "goal pursuit: agent busy at continuation fire; goal no longer active or re-claimed — dropped");
            None
        }
        Err(e) => {
            warn!(error = %e, session = %session, "goal pursuit: re-arm after busy failed");
            None
        }
    }
}
