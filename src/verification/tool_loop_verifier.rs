//! `ToolLoopVerifier` — structural watchdog that vetoes when the
//! model has issued N consecutive identical tool calls without
//! producing thinking text in between (closes master roadmap § 1.4
//! P1: "stop hook 仅在模型停手触发；tool_use 死循环不覆盖").
//!
//! Detection rule (deliberately conservative — false positives are
//! costly because they inject a [verifier veto] message that disrupts
//! the model):
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
use crate::verification::turn_verifier::{TurnVerifier, TurnVerifyContext, VerifierVerdict};

pub struct ToolLoopVerifier {
    pub repeat_threshold: usize,
}

impl ToolLoopVerifier {
    /// Default threshold: 5 identical consecutive calls. Matches the
    /// number cited in master spec § Stage 6 ("纯重复 tool call N
    /// 轮"). Tunable per deployment via `with_threshold`.
    pub fn new() -> Self {
        Self { repeat_threshold: 5 }
    }

    pub fn with_threshold(mut self, n: usize) -> Self {
        self.repeat_threshold = n.max(2);
        self
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
        if ctx.recent_tool_calls.len() < self.repeat_threshold {
            return VerifierVerdict::Continue;
        }
        let has_text = ctx.final_text.map(|t| !t.trim().is_empty()).unwrap_or(false);
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
