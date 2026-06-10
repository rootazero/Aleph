//! LLM provider selection logic for harness bridge.

use std::collections::HashMap;
use crate::sync_primitives::Arc;

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
                .ok_or_else(|| FlowError::ProviderUnavailable(provider.clone()))?;
            // Pin the model too when specified — stamp it onto every request.
            Ok(match model {
                Some(m) => Arc::new(crate::providers::ModelOverrideProvider::new(llm, m.clone())),
                None => llm,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::message::UnifiedMessage;
    use crate::providers::StaticDefault;
    use std::future::Future;
    use std::pin::Pin;
    use crate::sync_primitives::Mutex;

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
            *self.seen.lock().unwrap() = payload.model.clone();
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
        assert_eq!(rec.seen.lock().unwrap().as_deref(), Some("gpt-5"));
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
        assert_eq!(rec.seen.lock().unwrap().as_deref(), None);
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
}
