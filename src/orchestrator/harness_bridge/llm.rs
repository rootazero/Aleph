//! LLM provider selection logic for harness bridge.

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::BrainRef;
use crate::providers::{AiProvider, DefaultProviderHandle};

/// Pick the `AiProvider` for a given [`BrainRef`]. `Strict` returns
/// `ProviderUnavailable` when the named provider is not registered; when it
/// also carries a `model` (the "pin" form of an agent's configured model), the
/// picked provider is wrapped in [`ModelOverrideProvider`] so that model is
/// stamped onto every request — giving provider+model pinning real teeth.
///
/// `default_provider` is a live handle (see Step 5 hot-reload): every call to
/// `.current()` reads through the registry's `RwLock`, so UI-driven
/// `set_default` swaps take effect on the very next turn.
pub(super) fn pick_llm(
    brain: &BrainRef,
    default_provider: &Arc<dyn DefaultProviderHandle>,
    named: &HashMap<String, Arc<dyn AiProvider>>,
) -> Result<Arc<dyn AiProvider>, FlowError> {
    match brain {
        BrainRef::Default => Ok(default_provider.current()),
        BrainRef::Preferred { provider } => {
            if let Some(llm) = named.get(provider) {
                // rust-doctor-disable-next-line excessive-clone
                Ok(llm.clone())
            } else {
                // Silent fallback is intentional — Preferred means "use this if
                // available, otherwise default." A debug log signals the mismatch
                // so operators can spot misconfigured preferred providers.
                tracing::debug!(
                    provider = %provider,
                    "preferred provider not registered, falling back to default"
                );
                Ok(default_provider.current())
            }
        }
        BrainRef::Strict { provider, model } => {
            let llm = named
                .get(provider)
                .cloned()
                // rust-doctor-disable-next-line excessive-clone
                .ok_or_else(|| FlowError::ProviderUnavailable(provider.clone()))?;
            // Pin the model too when specified — stamp it onto every request.
            Ok(match model {
                // rust-doctor-disable-next-line excessive-clone
                Some(m) => Arc::new(crate::providers::ModelOverrideProvider::new(llm, m.clone())),
                None => llm,
            })
        }
    }
}

/// Wrap `fallback` (the result of [`pick_llm`] or the agent/brain resolution
/// this run would otherwise use) with session-model resolution, so a
/// `select_model` pick takes effect at the next request boundary rather than
/// only at run construction.
///
/// `fallback` serves every request whose session has no selection;
/// `named`/`default_provider` resolve qualified/unqualified picks (fail-closed
/// when the selected provider is missing). The seam lives here so the bridge
/// owns the construction contract without touching [`pick_llm`]'s precedence.
#[allow(dead_code)] // wired by runner_impl (Task 3); exercised by the test below
pub(super) fn with_session_model(
    session_key: String,
    fallback: Arc<dyn AiProvider>,
    default_provider: Arc<dyn DefaultProviderHandle>,
    named: Arc<HashMap<String, Arc<dyn AiProvider>>>,
) -> Arc<dyn AiProvider> {
    crate::providers::SessionModelProvider::new(session_key, fallback, default_provider, named)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::message::UnifiedMessage;
    use crate::providers::StaticDefault;
    use crate::sync_primitives::Mutex;
    use std::future::Future;
    use std::pin::Pin;

    /// Provider that records the model it was handed and reports its name.
    struct Recorder {
        name: String,
        seen: Mutex<Option<String>>,
    }

    impl Recorder {
        fn arc(name: &str) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_string(),
                seen: Mutex::new(None),
            })
        }
    }

    impl AiProvider for Recorder {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = payload.model.clone();
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".to_string())) })
        }
        fn name(&self) -> &str {
            &self.name
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    fn named(rec: Arc<Recorder>) -> HashMap<String, Arc<dyn AiProvider>> {
        let mut m: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        m.insert("openai".to_string(), rec);
        m
    }

    fn default_handle() -> Arc<dyn DefaultProviderHandle> {
        Arc::new(StaticDefault::new(Recorder::arc("default")))
    }

    #[tokio::test]
    async fn strict_with_model_stamps_it() {
        let rec = Recorder::arc("openai");
        let brain = BrainRef::Strict {
            provider: "openai".to_string(),
            model: Some("gpt-5".to_string()),
        };
        let llm = pick_llm(&brain, &default_handle(), &named(rec.clone())).unwrap();
        let msgs = [UnifiedMessage::user("hi")];
        let _ = llm.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(
            rec.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref(),
            Some("gpt-5")
        );
    }

    #[tokio::test]
    async fn strict_without_model_does_not_stamp() {
        let rec = Recorder::arc("openai");
        let brain = BrainRef::Strict {
            provider: "openai".to_string(),
            model: None,
        };
        let llm = pick_llm(&brain, &default_handle(), &named(rec.clone())).unwrap();
        let msgs = [UnifiedMessage::user("hi")];
        let _ = llm.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(
            rec.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref(),
            None
        );
    }

    #[test]
    fn strict_unknown_provider_errors() {
        let brain = BrainRef::Strict {
            provider: "ghost".to_string(),
            model: Some("x".to_string()),
        };
        let empty: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        assert!(pick_llm(&brain, &default_handle(), &empty).is_err());
    }

    #[tokio::test]
    async fn with_session_model_resolves_selection_at_request_boundary() {
        let rec = Recorder::arc("openai");
        let fallback = Recorder::arc("fallback");
        let wrapped = with_session_model(
            "llm-seam".to_string(),
            fallback.clone(),
            default_handle(),
            Arc::new(named(rec.clone())),
        );

        // No selection — the fallback serves the caller's own model.
        let msgs = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs).with_model(Some("caller-model".to_string()));
        let _ = wrapped.process(payload).await.unwrap();
        assert_eq!(
            fallback
                .seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref(),
            Some("caller-model")
        );

        // A session pick reroutes the next request to the named provider.
        crate::providers::session_model_handle::set_session_model(
            "llm-seam",
            Some("openai".to_string()),
            "gpt-5".to_string(),
        );
        let _ = wrapped.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(
            rec.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_deref(),
            Some("gpt-5")
        );
        assert_eq!(
            wrapped.last_request_target(),
            Some(crate::providers::ResolvedModelTarget {
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
                generation: 1,
            })
        );
    }
}
