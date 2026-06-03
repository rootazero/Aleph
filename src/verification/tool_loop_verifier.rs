//! `ToolLoopVerifier` — structural watchdog that vetoes when the
//! model has issued N consecutive identical tool calls without
//! producing thinking text in between (closes master roadmap § 1.4
//! P1: "stop hook 仅在模型停手触发；tool_use 死循环不覆盖").
//!
//! Detection rule (deliberately conservative — false positives are
//! costly because they inject a [verifier veto] message that disrupts
//! the model):
//!   - this is a *mid-turn* turn (`ctx.stop_reason.is_none()`) — the loop
//!     is still emitting tool calls; the stop turn belongs to the stop /
//!     goal verifiers, and firing there would only re-judge stale history
//!   - `ctx.recent_tool_calls.len() >= threshold`
//!   - the trailing `threshold` entries all have the same `name` and
//!     `args_hash`
//!   - the current turn's `final_text` is empty/None
//!
//! When all three hold, emit a `Veto` carrying `ErrorClass::Recoverable`
//! and a human-readable reason. The harness injects this as a user
//! message; on the next turn the model sees explicit feedback that
//! its repeat behavior was caught.

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::ErrorClass;
use crate::verification::turn_verifier::{
    TurnVerifier, TurnVerifyContext, VerifierVerdict, TOOL_HISTORY_WINDOW,
};

pub struct ToolLoopVerifier {
    repeat_threshold: usize,
}

impl ToolLoopVerifier {
    /// Default threshold: 5 identical consecutive calls. Matches the
    /// number cited in master spec § Stage 6 ("纯重复 tool call N
    /// 轮"). Tunable per deployment via `with_threshold`.
    pub fn new() -> Self {
        Self {
            repeat_threshold: 5,
        }
    }

    /// Set the repetition threshold. Clamped to `[2, TOOL_HISTORY_WINDOW]`:
    /// the lower bound avoids "single call ⇒ instant veto"; the upper bound
    /// is the harness ring-buffer capacity — a threshold above it could never
    /// be satisfied (`recent_tool_calls.len()` is bounded by the window), which
    /// would silently disable detection.
    pub fn with_threshold(mut self, n: usize) -> Self {
        self.repeat_threshold = n.clamp(2, TOOL_HISTORY_WINDOW);
        self
    }

    /// Current repetition threshold (always within `[2, TOOL_HISTORY_WINDOW]`).
    pub fn threshold(&self) -> usize {
        self.repeat_threshold
    }
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
        let has_text = ctx
            .final_text
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        if has_text {
            return VerifierVerdict::Continue;
        }
        let tail_start = ctx.recent_tool_calls.len() - self.repeat_threshold;
        let tail = &ctx.recent_tool_calls[tail_start..];
        let first = &tail[0];
        let all_same = tail
            .iter()
            .all(|c| c.name == first.name && c.args_hash == first.args_hash);
        if !all_same {
            return VerifierVerdict::Continue;
        }
        VerifierVerdict::Veto {
            reason: format!(
                "tool '{}' invoked {} consecutive times with no thinking text — try a different approach or summarize what you've found",
                first.name, self.repeat_threshold
            ),
            class: ErrorClass::Recoverable,
        }
    }
}
