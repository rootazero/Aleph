//! Provider wrapper that stamps `RequestPayload.think_level` before delegating.
//!
//! Sibling of [`super::model_override_provider::ModelOverrideProvider`], and for
//! the same reason it exists: binding a per-run value onto every request is done
//! by *wrapping* the chosen provider, not by threading the value through every
//! call site. Here that choice is load-bearing rather than merely tidy — the
//! only call sites are inside `src/harness/`, which R10 caps at 12 files (the
//! line count held by the `budget.rs` ratchet) and which must carry no per-run
//! policy. A forwarded scalar is not cognition, but the harness is at its
//! ratcheted ceiling and the repo's own guidance is explicit: new code goes
//! outside the loop. So it does.
//!
//! # Why the main loop and not the compactor
//!
//! `harness_bridge::runner_impl` hands the SAME provider `Arc` to two consumers:
//! the Think→Act loop and `ContextCompactor`, which uses it for side-channel
//! history summarization. Reasoning tokens bill at the OUTPUT rate, so stamping
//! a user's `xhigh` onto summarization calls would pay premium reasoning prices
//! to compress a transcript — a pure loss. This wrapper is therefore applied to
//! the `Arc` that goes into `HarnessDeps.llm` ONLY, after the compactor has
//! already taken its own unwrapped clone. Moving the wrap earlier in that
//! function would silently reintroduce that cost.
//!
//! Until this wrapper existed, the main loop never set `think_level` at all: the
//! whole four-protocol wire layer (`reasoning_effort`, Anthropic adaptive
//! `effort` / legacy `budget_tokens`, Gemini `thinkingBudget`) and its per-model
//! clamp matrix were unreachable outside group-chat personas.
//!
//! R7: the level is DECLARED (by the user via the composer / `chat.send`, or by
//! the model via `self_config`) and forwarded verbatim. Nothing here reads the
//! message to guess how hard the task is.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use crate::agents::thinking::ThinkLevel;
use crate::error::Result;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Wraps an [`AiProvider`], stamping `payload.think_level` before delegating.
/// The stamp is unconditional — it pins the declared depth over anything the
/// caller set, mirroring `ModelOverrideProvider`'s pin semantics.
pub struct ThinkLevelProvider {
    inner: Arc<dyn AiProvider>,
    level: ThinkLevel,
}

impl ThinkLevelProvider {
    /// Wrap `inner`, forcing `level` onto every request.
    pub fn new(inner: Arc<dyn AiProvider>, level: ThinkLevel) -> Self {
        Self { inner, level }
    }
}

impl AiProvider for ThinkLevelProvider {
    fn process<'a>(
        &'a self,
        mut payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        payload.think_level = Some(self.level);
        self.inner.process(payload)
    }

    fn execute_streaming_dyn<'a>(
        &'a self,
        mut payload: RequestPayload<'a>,
        sink: &'a dyn crate::providers::DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // Same stamp as `process` — without this, streamed turns lost the
        // declared depth entirely (harness reached the inner provider directly).
        payload.think_level = Some(self.level);
        self.inner.execute_streaming_dyn(payload, sink)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn protocol(&self) -> Cow<'_, str> {
        self.inner.protocol()
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.inner.model_behavior_override()
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.behavior_hint()
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.serving_model_hint()
    }

    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        self.inner.serving_provider_hint()
    }

    fn as_http_provider(&self) -> Option<&crate::providers::http_provider::HttpProvider> {
        self.inner.as_http_provider()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;

    /// The whole point: a payload that arrived with no thinking directive must
    /// leave carrying the declared one. Before this wrapper, `think_level` was
    /// `None` on every main-loop request ever sent.
    #[tokio::test]
    async fn stamps_the_declared_level_onto_the_payload() {
        let stamped: Arc<std::sync::Mutex<Option<ThinkLevel>>> = Arc::new(Default::default());

        struct Spy(Arc<std::sync::Mutex<Option<ThinkLevel>>>);
        impl AiProvider for Spy {
            fn process<'a>(
                &'a self,
                payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                *self.0.lock().unwrap_or_else(|e| e.into_inner()) = payload.think_level;
                Box::pin(async { Ok(ProviderResponse::default()) })
            }
            fn name(&self) -> &str {
                "spy"
            }
            fn color(&self) -> &str {
                "#000000"
            }
        }

        let provider = ThinkLevelProvider::new(Arc::new(Spy(stamped.clone())), ThinkLevel::XHigh);
        let msgs = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs);
        assert!(payload.think_level.is_none(), "precondition");

        let _ = provider.process(payload).await;
        assert_eq!(
            *stamped.lock().unwrap_or_else(|e| e.into_inner()),
            Some(ThinkLevel::XHigh),
            "the inner provider must receive the declared level"
        );
    }

    /// Every non-`process` method must delegate, or wrapping would silently
    /// change identity — `serving_provider_hint` above all, since pricing is
    /// keyed on it and a broken hint puts every run back at $0.00.
    #[test]
    fn delegates_identity_to_the_inner_provider() {
        let inner = Arc::new(crate::providers::mock::MockProvider::new("hi"));
        let inner_name = inner.name().to_string();
        let provider = ThinkLevelProvider::new(inner, ThinkLevel::Low);
        assert_eq!(provider.name(), inner_name);
    }
}
