//! `StopHookVerifier` — adapts the existing `StopHookHandler` shell-hook
//! infrastructure to the `TurnVerifier` trait introduced in Stage 6a.
//!
//! Behaviour is identical to the pre-6a `evaluate_stop_hooks` helper
//! that lived in `agent.rs`: hooks fire only when the model is about
//! to stop (`ctx.stop_reason.is_some()`), execute in parallel, and the
//! first blocking exit-2 verdict becomes a `Veto`. When `stop_reason`
//! is `None` (mid-turn check) the verifier short-circuits to
//! `Continue` so non-stop turns pay zero cost.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::error::ErrorClass;
use crate::verification::stop_hooks::{
    execute_stop_hooks, StopHookContext, StopHookHandler, StopHookVerdict,
};
use crate::verification::turn_verifier::{TurnVerifier, TurnVerifyContext, VerifierVerdict};

pub struct StopHookVerifier {
    hooks: Arc<Vec<Arc<dyn StopHookHandler>>>,
}

impl StopHookVerifier {
    pub fn new(hooks: Arc<Vec<Arc<dyn StopHookHandler>>>) -> Self {
        Self { hooks }
    }
}

#[async_trait]
impl TurnVerifier for StopHookVerifier {
    fn name(&self) -> &str {
        "stop_hook"
    }

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
        // `execute_stop_hooks` wants `&[Box<dyn StopHookHandler>]`; we
        // hold `Arc`s for shareability. Wrap each in a forwarding box —
        // avoids cloning the hook implementations.
        struct ArcHook(Arc<dyn StopHookHandler>);
        #[async_trait]
        impl StopHookHandler for ArcHook {
            fn name(&self) -> &str {
                self.0.name()
            }
            async fn evaluate(
                &self,
                hctx: &StopHookContext,
                cancel: &CancellationToken,
            ) -> StopHookVerdict {
                self.0.evaluate(hctx, cancel).await
            }
        }
        let boxed: Vec<Box<dyn StopHookHandler>> = self
            .hooks
            .iter()
            .map(|h| Box::new(ArcHook(h.clone())) as Box<dyn StopHookHandler>)
            .collect();
        let hctx = StopHookContext {
            final_text: ctx.final_text.map(|s| s.to_string()),
            iterations: ctx.iterations,
            tool_calls_made: ctx.tool_calls_made,
            stop_reason: stop_reason.to_string(),
        };
        let result = execute_stop_hooks(&boxed, &hctx, cancel).await;
        match result.blocking_reason() {
            Some(reason) => VerifierVerdict::Veto {
                reason: reason.to_string(),
                class: ErrorClass::Recoverable,
            },
            None => VerifierVerdict::Continue,
        }
    }
}
