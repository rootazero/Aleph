//! `ToolLoopVerifier` — structural watchdog that vetoes when the
//! model has issued N consecutive identical tool calls (same `name` +
//! `args_hash`), regardless of any narration text on the turn (closes
//! master roadmap § 1.4 P1: "stop hook 仅在模型停手触发；tool_use 死循环不覆盖").
//!
//! Detection rule (deliberately conservative — false positives are
//! costly because they inject a [verifier veto] message that disrupts
//! the model):
//!   - this is a *mid-turn* turn (`ctx.stop_reason.is_none()`) — the loop
//!     is still emitting tool calls; the stop turn belongs to the stop /
//!     goal verifiers, and firing there would only re-judge stale history
//!   - `ctx.recent_tool_calls.len() >= threshold`
//!   - the trailing `threshold` entries all have the same `name` and
//!     `args_hash` (identical, redundant calls — varied args reset the run)
//!
//! Two-tier escalation:
//!   - at `repeat_threshold` identical trailing calls → emit a `Veto`
//!     (`ErrorClass::Recoverable`). The harness injects it as a user message so
//!     the model sees explicit feedback and gets a chance to course-correct.
//!   - at `halt_threshold` (≥ repeat_threshold) identical trailing calls → emit
//!     a `Halt`. By this point the model has ignored several vetoes and is still
//!     repeating the same call with no thinking text; continuing would only burn
//!     LLM round-trips until the provider's rate limit (or a turn/stall
//!     timeout) kills the run with a confusing error. Halting deterministically
//!     here ends the unproductive loop with a clear reason instead.
//!
//! Tier 2 (same name, varying args): when the *entire* history window is the
//! same tool `name` but the args keep changing — so Tier 1's identical run never
//! accumulates — and the turn carries no narration text, emit a `Halt`. This
//! catches a thrash the identical-args check is blind to (e.g. re-reading three
//! reference files round and round). It is gated on silence: varying-args
//! exploration *with* narration can be legitimate, and a Tier-2 Halt is
//! terminal, so only a wholly silent loop is cut. Tier 1 fires regardless of
//! narration text; Tier 2 only on its absence.
//!
//! All tiers are pure structural checks over `(name, args_hash)` and the
//! presence/absence of text — no model reasoning, so this stays scaffolding
//! (R10-safe), never a completion judge.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::ErrorClass;
use crate::verification::turn_verifier::{
    ToolCallSummary, TurnVerifier, TurnVerifyContext, VerifierVerdict, TOOL_HISTORY_WINDOW,
};

pub struct ToolLoopVerifier {
    repeat_threshold: usize,
    halt_threshold: usize,
}

impl ToolLoopVerifier {
    /// Default thresholds: veto at 5 identical consecutive calls (master spec
    /// § Stage 6 "纯重复 tool call N 轮"), hard-halt at the full
    /// [`TOOL_HISTORY_WINDOW`] (8) — i.e. ~3 ignored vetoes before the loop is
    /// cut off. Both tunable via `with_threshold` / `with_halt_threshold`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            repeat_threshold: 5,
            halt_threshold: TOOL_HISTORY_WINDOW,
        }
    }

    /// Set the repetition (veto) threshold. Clamped to `[2, TOOL_HISTORY_WINDOW]`:
    /// the lower bound avoids "single call ⇒ instant veto"; the upper bound
    /// is the harness ring-buffer capacity — a threshold above it could never
    /// be satisfied (`recent_tool_calls.len()` is bounded by the window), which
    /// would silently disable detection. The halt threshold is lifted to stay
    /// `≥ repeat_threshold` so the two tiers never invert.
    #[must_use]
    pub fn with_threshold(mut self, n: usize) -> Self {
        self.repeat_threshold = n.clamp(2, TOOL_HISTORY_WINDOW);
        self.halt_threshold = self.halt_threshold.max(self.repeat_threshold);
        self
    }

    /// Set the hard-halt threshold. Clamped to `[repeat_threshold,
    /// TOOL_HISTORY_WINDOW]` so it can always be reached and never fires *before*
    /// the soft veto tier.
    #[must_use]
    pub fn with_halt_threshold(mut self, n: usize) -> Self {
        self.halt_threshold = n.clamp(self.repeat_threshold, TOOL_HISTORY_WINDOW);
        self
    }

    /// Current repetition (veto) threshold (always within `[2, TOOL_HISTORY_WINDOW]`).
    #[must_use]
    pub fn threshold(&self) -> usize {
        self.repeat_threshold
    }

    /// Current hard-halt threshold (always within `[repeat_threshold, TOOL_HISTORY_WINDOW]`).
    #[must_use]
    pub fn halt_threshold(&self) -> usize {
        self.halt_threshold
    }
}

/// Length of the trailing run of calls identical (same `name` + `args_hash`)
/// to the most recent one. `0` for an empty slice.
fn trailing_repeat_run(calls: &[ToolCallSummary]) -> usize {
    let Some(last) = calls.last() else {
        return 0;
    };
    calls
        .iter()
        .rev()
        .take_while(|c| c.name == last.name && c.args_hash == last.args_hash)
        .count()
}

/// Length of the trailing run of calls sharing the most recent call's `name`,
/// **ignoring `args_hash`**. `0` for an empty slice. Used by Tier 2 to catch a
/// same-tool thrash whose arguments keep changing (so the identical-args
/// [`trailing_repeat_run`] never accumulates).
fn trailing_same_name_run(calls: &[ToolCallSummary]) -> usize {
    let Some(last) = calls.last() else {
        return 0;
    };
    calls
        .iter()
        .rev()
        .take_while(|c| c.name == last.name)
        .count()
}

impl Default for ToolLoopVerifier {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TurnVerifier for ToolLoopVerifier {
    fn name(&self) -> &str {
        "tool_loop"
    }

    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        _cancel: &CancellationToken,
    ) -> VerifierVerdict {
        // Death-loop detection is a *mid-turn* concern: every turn of the loop
        // emits a tool call, so `stop_reason` is `None` while it is happening
        // (the stop turn is `StopHookVerifier`/`ScratchpadGoalVerifier`'s job).
        // Evaluating on a stop turn would only re-examine *stale* buffer
        // entries from earlier turns and could veto a legitimate stop whose
        // answer text is empty (e.g. a thinking-only finish), flipping a clean
        // Done into a Continue. Gate it out — this drops no real detection
        // because the triggering call always lands on a `stop_reason.is_none()`
        // turn.
        if ctx.stop_reason.is_some() {
            return VerifierVerdict::Continue;
        }
        if ctx.recent_tool_calls.len() < self.repeat_threshold {
            return VerifierVerdict::Continue;
        }
        let run = trailing_repeat_run(ctx.recent_tool_calls);
        // Tier 1 — identical (name + args_hash) consecutive calls. Fires
        // regardless of narration text: repeating the *exact* same call with
        // unchanged arguments is never productive, so a thinking-text escape
        // would only let a loop disguise itself.
        if run >= self.repeat_threshold {
            // `run >= repeat_threshold` guarantees a non-empty trailing run, so
            // the last entry names the offending tool.
            let tool = &ctx.recent_tool_calls[ctx.recent_tool_calls.len() - 1].name;
            // Halt only once the loop has persisted *past* at least one veto —
            // i.e. when the halt tier sits strictly above the veto tier. In the
            // degenerate clamp case where both collapse to the window size, there
            // is no "ignored veto" stage, so keep vetoing (the conservative
            // original behavior) rather than halting on first detection.
            if run >= self.halt_threshold && self.halt_threshold > self.repeat_threshold {
                return VerifierVerdict::Halt {
                    reason: format!(
                        "tool '{tool}' invoked {run} consecutive times with no thinking text despite \
                         repeated feedback — terminating to avoid an unproductive loop that would run \
                         into the provider rate limit",
                    ),
                    class: ErrorClass::Recoverable,
                };
            }
            return VerifierVerdict::Veto {
                reason: format!(
                    "tool '{tool}' invoked {run} consecutive times with no thinking text — try a different approach or summarize what you've found",
                ),
                class: ErrorClass::Recoverable,
            };
        }

        // Tier 2 — same tool NAME repeated across the *entire* history window
        // with varying arguments and no narration text. Catches a thrash Tier 1
        // misses: e.g. re-reading template.html / layouts.md / themes.md in a
        // loop — all `file_read`, never the same args twice, so the
        // identical-args run never builds. Gated on silence because varying-args
        // exploration *with* narration can be legitimate, and a Tier-2 Halt is
        // terminal; we only cut a loop the model is running with no reasoning
        // text at all.
        let same_name_run = trailing_same_name_run(ctx.recent_tool_calls);
        let has_text = ctx
            .final_text
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        if !has_text && same_name_run >= TOOL_HISTORY_WINDOW {
            let tool = &ctx.recent_tool_calls[ctx.recent_tool_calls.len() - 1].name;
            return VerifierVerdict::Halt {
                reason: format!(
                    "tool '{tool}' invoked {same_name_run} consecutive times with varying arguments \
                     and no thinking text — terminating an unproductive exploration loop that would \
                     run into the provider rate limit",
                ),
                class: ErrorClass::Recoverable,
            };
        }

        VerifierVerdict::Continue
    }
}
