//! `StopHookVerifier` — adapts the existing `StopHookHandler` shell-hook
//! infrastructure to the `TurnVerifier` trait introduced in Stage 6a.
//!
//! Behaviour is identical to the pre-6a `evaluate_stop_hooks` helper
//! that lived in `agent.rs`: hooks fire only when the model is about
//! to stop (`ctx.stop_reason.is_some()`), execute in parallel, and the
//! first blocking exit-2 verdict becomes a `Veto`. When `stop_reason`
//! is `None` (mid-turn check) the verifier short-circuits to
//! `Continue` so non-stop turns pay zero cost.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::verification::stop_hooks::{execute_stop_hooks_arc, StopHookContext, StopHookHandler};
use crate::verification::turn_verifier::{TurnVerifier, TurnVerifyContext, VerifierVerdict};

pub struct StopHookVerifier {
    hooks: Arc<Vec<Arc<dyn StopHookHandler>>>,
}

impl StopHookVerifier {
    #[must_use]
    pub fn new(hooks: Arc<Vec<Arc<dyn StopHookHandler>>>) -> Self {
        Self { hooks }
    }
}

#[async_trait]
impl TurnVerifier for StopHookVerifier {
    async fn verify(
        &self,
        ctx: &TurnVerifyContext<'_>,
        cancel: &CancellationToken,
    ) -> VerifierVerdict {
        let Some(stop_reason) = ctx.stop_reason else {
            return VerifierVerdict::Continue;
        };
        if self.hooks.is_empty() {
            return VerifierVerdict::Continue;
        }
        let hctx = StopHookContext {
            final_text: ctx.final_text.map(|s| {
                let cap = 4096; // mirror extension_stop_gate::LAST_MESSAGE_ENV_CAP
                let end = s
                    .char_indices()
                    .take(cap)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(0);
                s[..end.min(s.len())].to_string()
            }),
            iterations: ctx.iterations,
            tool_calls_made: ctx.tool_calls_made,
            stop_reason: stop_reason.to_string(),
        };
        let result = execute_stop_hooks_arc(&self.hooks, &hctx, cancel).await;
        // Surface hook execution errors at warn level so a misconfigured
        // hook (spawn failure, signal, timeout, cancellation) does not
        // vanish into the void. The verdict is still fail-open — these
        // never block or halt the turn — but the user must at least see
        // a record that the hook did not actually run.
        for (hook_name, message) in result.errors() {
            tracing::warn!(
                hook = %hook_name,
                error = %message,
                "stop hook failed to execute; turn proceeding without its verdict"
            );
        }
        // Halt outranks Block — when both fire, the loop must exit
        // (claude-code's preventContinuation semantics). A Halt verdict
        // is permanent; a Block verdict triggers a Continue+retry.
        if let Some(reason) = result.halt_reason() {
            return VerifierVerdict::Halt {
                reason: reason.to_string(),
            };
        }
        match result.blocking_reason() {
            Some(reason) => VerifierVerdict::Veto {
                reason: reason.to_string(),
            },
            None => VerifierVerdict::Continue,
        }
    }
}
