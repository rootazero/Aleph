//! Provider wrapper that stamps `RequestPayload.model` before delegating.
//!
//! Model binding in Aleph is done by *wrapping* the chosen base provider so it
//! forces a specific model onto every request, rather than threading the model
//! through every call site. Two consumers share this wrapper:
//!
//! * sub-agent spawn — a per-spawn `model` / `model_hint` pins the child's model;
//! * `pick_llm` — a `BrainRef::Strict { provider, model: Some(..) }` (the "pin"
//!   form of an agent's configured model) stamps that model onto the picked
//!   provider, closing the model half of provider/model pinning.
//!
//! All trait methods except `process` delegate verbatim to the inner provider,
//! so wrapping is transparent to name/color/protocol/tool-support.

use crate::sync_primitives::Arc;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;

/// Wraps an [`AiProvider`], stamping `payload.model` with `model` before
/// delegating. The stamp is unconditional — it overrides any model the caller
/// already set, which is the intended "pin" behaviour.
pub struct ModelOverrideProvider {
    inner: Arc<dyn AiProvider>,
    model: String,
}

impl ModelOverrideProvider {
    /// Wrap `inner`, forcing `model` onto every request.
    pub fn new(inner: Arc<dyn AiProvider>, model: impl Into<String>) -> Self {
        Self {
            inner,
            model: model.into(),
        }
    }
}

impl AiProvider for ModelOverrideProvider {
    fn process<'a>(
        &'a self,
        mut payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        payload.model = Some(self.model.clone());
        self.inner.process(payload)
    }

    /// Same stamp as `process`, then delegate — without this override the trait
    /// default collapses to `process` and replays the response in one batch, so
    /// live streaming would silently switch off for exactly the sessions that
    /// have a `select_model` pick or an agent `model_hint`.
    fn execute_streaming_dyn<'a>(
        &'a self,
        mut payload: RequestPayload<'a>,
        sink: &'a dyn crate::providers::DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        // rust-doctor-disable-next-line excessive-clone
        payload.model = Some(self.model.clone());
        self.inner.execute_streaming_dyn(payload, sink)
    }

    fn supports_streaming(&self) -> bool {
        self.inner.supports_streaming()
    }

    fn as_http_provider(&self) -> Option<&crate::providers::http_provider::HttpProvider> {
        self.inner.as_http_provider()
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

    fn model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.inner.model_behavior_override()
    }

    fn behavior_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.inner.behavior_hint()
    }

    /// The stamped model IS the serving model — this wrapper exists to pin it.
    fn serving_model_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(std::borrow::Cow::Borrowed(self.model.as_str()))
    }

    /// This wrapper pins the MODEL, not the provider — the provider is still
    /// whoever the inner chain resolves to.
    fn serving_provider_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.inner.serving_provider_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use crate::sync_primitives::Mutex;

    /// Inner provider that records the `model` it was handed.
    struct Recorder {
        seen: Mutex<Option<String>>,
    }

    impl AiProvider for Recorder {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            *self.seen.lock().unwrap() = payload.model.clone();
            Box::pin(async move { Ok(ProviderResponse::text_only("inner".to_string())) })
        }
        fn name(&self) -> &str {
            "recorder"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    #[tokio::test]
    async fn stamps_model_onto_payload() {
        let inner = Arc::new(Recorder {
            seen: Mutex::new(None),
        });
        let wrapped = ModelOverrideProvider::new(inner.clone(), "claude-opus-4");
        let msgs = [UnifiedMessage::user("hi")];
        // Caller leaves model unset — the wrapper must fill it.
        let _ = wrapped.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(inner.seen.lock().unwrap().as_deref(), Some("claude-opus-4"));
    }

    #[tokio::test]
    async fn stamp_overrides_caller_model() {
        let inner = Arc::new(Recorder {
            seen: Mutex::new(None),
        });
        let wrapped = ModelOverrideProvider::new(inner.clone(), "pinned-model");
        let msgs = [UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs).with_model(Some("caller-model".to_string()));
        let _ = wrapped.process(payload).await.unwrap();
        assert_eq!(inner.seen.lock().unwrap().as_deref(), Some("pinned-model"));
    }

    #[test]
    fn delegates_metadata_to_inner() {
        let inner = Arc::new(Recorder {
            seen: Mutex::new(None),
        });
        let wrapped = ModelOverrideProvider::new(inner, "m");
        assert_eq!(wrapped.name(), "recorder");
        assert_eq!(wrapped.color(), "#000");
    }

    #[test]
    fn delegates_behavior_override_to_inner() {
        struct OverrideInner;
        impl AiProvider for OverrideInner {
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::text_only("inner".to_string())) })
            }
            fn name(&self) -> &str {
                "inner"
            }
            fn color(&self) -> &str {
                "#000"
            }
            fn model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>> {
                Some(std::borrow::Cow::Borrowed("openrouter-anthropic"))
            }
        }
        let wrapped = ModelOverrideProvider::new(Arc::new(OverrideInner), "m");
        assert_eq!(
            wrapped.model_behavior_override().as_deref(),
            Some("openrouter-anthropic")
        );
    }

    #[test]
    fn delegates_behavior_hint_to_inner() {
        struct HintInner;
        impl AiProvider for HintInner {
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::text_only("inner".to_string())) })
            }
            fn name(&self) -> &str {
                "inner"
            }
            fn color(&self) -> &str {
                "#000"
            }
            fn behavior_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
                Some(std::borrow::Cow::Borrowed("strict"))
            }
        }
        let wrapped = ModelOverrideProvider::new(Arc::new(HintInner), "m");
        assert_eq!(wrapped.behavior_hint().as_deref(), Some("strict"));
    }

    /// The stamped model is the serving model — the wrapper reports it even
    /// when the inner provider has no opinion (default `None`), so the
    /// context-gauge window resolves from the actually-pinned model.
    #[test]
    fn serving_model_hint_reports_stamped_model() {
        let inner = Arc::new(Recorder {
            seen: Mutex::new(None),
        });
        let wrapped = ModelOverrideProvider::new(inner, "kimi-k2.7");
        assert_eq!(wrapped.serving_model_hint().as_deref(), Some("kimi-k2.7"));
    }
}
