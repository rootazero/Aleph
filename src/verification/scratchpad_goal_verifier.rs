//! `ScratchpadGoalVerifier` — the goal-loop hook.
//!
//! Closes the *within-turn* half of the gap against hermes-agent's
//! `goals.py`: after the LLM decomposes a request into an execution list
//! (via the `scratchpad` tool's `set_objective` + `set_plan`), *something*
//! must keep the loop running until those steps are worked through — "complete
//! each step, converge, until the user's goal is achieved". This verifier is that within-turn hook.
//!
//! The *cross-turn* half — a persistent standing goal that survives across
//! turns/sessions with lifecycle + budget, and an opt-in autonomous
//! continuation driver — lives in the `src/goal/` subsystem (`goal` tool +
//! `StandingGoalLayer` + `src/goal/pursuit.rs`), not here. See
//! docs/superpowers/specs/2026-06-08-standing-goal-design.md.
//!
//! ## Why this is a structural watchdog, not a `JudgeVerifier` (R7 / R10)
//!
//! [`crate::verification`] permanently prohibits cognitive
//! `JudgeVerifier`s — completion judgment belongs to the LLM, in the
//! prompt, never in Rust. This verifier does **not** judge whether the
//! objective is semantically achieved. It checks one purely mechanical
//! fact about the model's *own* bookkeeping: "the model is trying to
//! stop, yet its self-maintained checklist still has unchecked `- [ ]`
//! boxes." That is the same shape as `StopHookVerifier` reading a shell
//! hook's exit code — a structural signal the model fully controls.
//!
//! The model stays sovereign over completion: it decides what goes on the
//! list, checks boxes via `complete_item`, and ends the loop by either
//! finishing every box or calling `scratchpad(action='clear')`. The
//! per-model `steer_max` cap in the harness is the hard backstop, mirroring
//! goals.py's `max_turns`. Zero LLM calls; passes R10's Future-Proof Test.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::builtin_tools::scratchpad_registry;
use crate::memory::scratchpad::ScratchpadSnapshot;
use crate::memory::ScratchpadManager;
use crate::verification::turn_verifier::{TurnVerifier, TurnVerifyContext, VerifierVerdict};

/// Cap on how many pending items a single veto message enumerates.
const MAX_LISTED: usize = 8;

/// Vetoes a stop attempt while the session's active scratchpad has an
/// objective set and unchecked plan items. See module docs for the
/// R7/R10 structural-watchdog justification.
pub struct ScratchpadGoalVerifier;

impl ScratchpadGoalVerifier {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ScratchpadGoalVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TurnVerifier for ScratchpadGoalVerifier {
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        // Only fires when the model is about to STOP. Mid-turn (tool calls
        // still pending) pays zero cost — same short-circuit as StopHook.
        if ctx.stop_reason.is_none() {
            return VerifierVerdict::Continue;
        }
        // Need a session to locate the active execution list.
        let Some(session_key) = ctx.session_id else {
            return VerifierVerdict::Continue;
        };
        // Which project the session is writing its scratchpad to.
        let Some(project_id) = scratchpad_registry::active(session_key) else {
            return VerifierVerdict::Continue;
        };

        let manager = ScratchpadManager::new(&project_id, session_key);
        let snapshot = match tokio::select! {
            biased;
            _ = _cancel.cancelled() => return VerifierVerdict::Continue,
            res = manager.snapshot() => res,
        } {
            Ok(s) => s,
            // Fail-open: a watchdog must never wedge the loop on I/O error.
            Err(_) => return VerifierVerdict::Continue,
        };
        // Dormant unless an objective is set AND a box is still unchecked
        // (the user-chosen "activate only when there is an objective" gate).
        if !snapshot.has_pending_work() {
            return VerifierVerdict::Continue;
        }

        VerifierVerdict::Veto {
            reason: veto_reason(&snapshot),
        }
    }
}

/// The model-facing veto text.
///
/// Split out from [`ScratchpadGoalVerifier::verify`] so the rendering — the
/// only part that can be wrong — is testable without a live workspace on disk.
fn veto_reason(snapshot: &ScratchpadSnapshot) -> String {
    // Enumerate the FULL list, then filter — the index shown must be the one
    // `complete_item` takes.
    //
    // This used to list `snapshot.incomplete()`, a filtered sublist whose
    // positions have nothing to do with the item-index space, while the
    // instruction below names the index-addressed `complete_item`. On a plan
    // whose first step was already `[x]` the model read the second line of the
    // veto, called `complete_item(0)` — in range, so no bounds error and
    // `success: true` — and re-marked the step that was already done. The real
    // step stayed pending, `has_pending_work()` stayed true, the next stop
    // attempt was vetoed identically, and the run burned `steer_max` before
    // surfacing "⚠️ 目标未达成".
    //
    // Same principle `plan_carry.rs` was built on: "丢掉已完成项会让模型手里的每个
    // 下标都错位" (FEATURE_LOCATOR §3.13 ⑥). This was the one model-facing
    // surface still dropping them.
    let pending: Vec<(usize, &crate::memory::scratchpad::PlanItem)> = snapshot
        .items
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.is_done())
        .collect();
    let objective = snapshot.objective.as_deref().unwrap_or("");
    let listed = pending
        .iter()
        .take(MAX_LISTED)
        .map(|(idx, i)| {
            if i.is_in_progress() {
                format!("- [{idx}] {} (in progress)", i.text)
            } else {
                format!("- [{idx}] {}", i.text)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let overflow = pending.len().saturating_sub(MAX_LISTED);
    let more_note = if overflow > 0 {
        format!("\n…and {overflow} more.")
    } else {
        String::new()
    };
    format!(
        "Your execution list for the objective \"{objective}\" still has {n} \
         incomplete step(s) (the number in brackets is the item index):\
         \n{listed}{more_note}\n\nKeep working through them one at a time — \
         mark the step you are working on with \
         `scratchpad(action='start_item', item_index=N)` and each finished \
         step with `scratchpad(action='complete_item', item_index=N)`, using \
         the bracketed index. If the objective is already fully achieved, \
         call `scratchpad(action='clear', …)` to finish.",
        n = pending.len(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_stopping<'a>(session_id: Option<&'a str>) -> TurnVerifyContext<'a> {
        TurnVerifyContext {
            iterations: 1,
            tool_calls_made: 0,
            final_text: Some("done"),
            recent_tool_calls: &[],
            stop_reason: Some("end_turn"),
            session_id,
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        }
    }

    #[tokio::test]
    async fn continues_mid_turn_regardless_of_scratchpad() {
        let v = ScratchpadGoalVerifier::new();
        let cancel = CancellationToken::new();
        let ctx = TurnVerifyContext {
            iterations: 1,
            tool_calls_made: 1,
            final_text: None,
            recent_tool_calls: &[],
            stop_reason: None, // mid-turn
            session_id: Some("sess-midturn"),
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        };
        assert!(v.verify(&ctx, &cancel).await.is_continue());
    }

    #[tokio::test]
    async fn continues_without_session() {
        let v = ScratchpadGoalVerifier::new();
        let cancel = CancellationToken::new();
        assert!(v.verify(&ctx_stopping(None), &cancel).await.is_continue());
    }

    #[tokio::test]
    async fn continues_when_no_active_scratchpad() {
        let v = ScratchpadGoalVerifier::new();
        let cancel = CancellationToken::new();
        // A session key that was never registered → dormant.
        let ctx = ctx_stopping(Some("sess-never-registered-xyz"));
        assert!(v.verify(&ctx, &cancel).await.is_continue());
    }

    /// The veto names `complete_item`, which is index-addressed — so the
    /// indices it prints must be the ones that tool takes.
    ///
    /// It used to enumerate `incomplete()`, a filtered sublist. With any step
    /// already done, position-in-the-veto and item-index diverge; the model
    /// then "completes" an already-done step, the veto repeats verbatim, and
    /// the run grinds to `steer_max`. Nothing errors — `complete_item` on an
    /// in-range done item returns success.
    #[test]
    fn the_veto_prints_absolute_item_indices_not_positions_in_the_pending_list() {
        use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadSnapshot};

        let snapshot = ScratchpadSnapshot {
            objective: Some("Ship auth".to_string()),
            items: vec![
                PlanItem {
                    text: "Design schema".into(),
                    status: PlanItemStatus::Done,
                },
                PlanItem {
                    text: "Write migration".into(),
                    status: PlanItemStatus::InProgress,
                },
                PlanItem {
                    text: "Backfill".into(),
                    status: PlanItemStatus::Pending,
                },
            ],
        };

        let reason = veto_reason(&snapshot);
        assert!(
            reason.contains("- [1] Write migration (in progress)"),
            "got: {reason}"
        );
        assert!(reason.contains("- [2] Backfill"), "got: {reason}");
        assert!(
            !reason.contains("[0]"),
            "index 0 is the DONE step and must not appear: {reason}"
        );
        assert!(
            reason.contains("still has 2 incomplete step(s)"),
            "got: {reason}"
        );
    }
}
