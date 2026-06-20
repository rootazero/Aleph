//! Single source of truth for a provider's governance behavior name.
//!
//! Collapses the two previously-duplicated resolutions (robustness thresholds
//! in `runner_impl` and the discarded diagnostic block in the gateway run
//! loop) into one function. The returned behavior name drives BOTH the
//! `ModelRobustnessProfile` watchdog thresholds AND the `ProviderGuidanceLayer`
//! coaching, so they can never drift.
//!
//! Precedence (highest first):
//!   1. explicit per-provider config `model_behavior` override
//!   2. vendor self-identification (`behavior_hint`, e.g. Kimi/Minimax →
//!      "strict") — MUST sit above protocol so a weak model on the anthropic
//!      wire protocol is not mistaken for Claude.
//!   3. protocol → behavior auto-mapping (anthropic/openai/gemini/ollama)
//!   4. "unknown" (conservative thresholds + non-anthropic baseline coaching)

use std::borrow::Cow;

use crate::providers::AiProvider;

/// Resolve the governance behavior name for `provider`. Always returns an
/// owned-or-static `Cow` so callers can feed it to both the robustness
/// profile (`for_behavior`) and the prompt builder without lifetime grief.
#[must_use]
pub fn resolve_behavior(provider: &dyn AiProvider) -> Cow<'static, str> {
    if let Some(over) = provider.model_behavior_override() {
        return Cow::Owned(over.into_owned());
    }
    if let Some(hint) = provider.behavior_hint() {
        return Cow::Owned(hint.into_owned());
    }
    if let Some(name) = crate::providers::model_behaviors::protocol_to_behavior(&provider.protocol())
    {
        return Cow::Borrowed(name);
    }
    Cow::Borrowed("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::message::UnifiedMessage;
    use std::future::Future;
    use std::pin::Pin;

    struct StubProvider {
        protocol: &'static str,
        override_: Option<&'static str>,
        hint: Option<&'static str>,
    }
    impl AiProvider for StubProvider {
        fn process<'a>(
            &'a self,
            _p: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("x".to_string())) })
        }
        fn name(&self) -> &str { "stub" }
        fn color(&self) -> &str { "#000" }
        fn protocol(&self) -> Cow<'_, str> { Cow::Borrowed(self.protocol) }
        fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
            self.override_.map(Cow::Borrowed)
        }
        fn behavior_hint(&self) -> Option<Cow<'_, str>> {
            self.hint.map(Cow::Borrowed)
        }
    }

    fn p(protocol: &'static str, override_: Option<&'static str>, hint: Option<&'static str>) -> StubProvider {
        StubProvider { protocol, override_, hint }
    }

    #[test]
    fn override_wins_over_everything() {
        let _ = UnifiedMessage::user("warmup");
        assert_eq!(resolve_behavior(&p("anthropic", Some("openai"), Some("strict"))), "openai");
    }

    #[test]
    fn hint_wins_over_protocol_kimi_over_anthropic() {
        // THE headline case: Kimi on the anthropic wire protocol must resolve
        // to "strict", NOT "anthropic" (Claude's loose leash).
        assert_eq!(resolve_behavior(&p("anthropic", None, Some("strict"))), "strict");
    }

    #[test]
    fn protocol_fallback_when_no_override_no_hint() {
        assert_eq!(resolve_behavior(&p("openai", None, None)), "openai");
        assert_eq!(resolve_behavior(&p("anthropic", None, None)), "anthropic");
    }

    #[test]
    fn unknown_protocol_falls_back_to_unknown() {
        assert_eq!(resolve_behavior(&p("some-custom", None, None)), "unknown");
    }
}
