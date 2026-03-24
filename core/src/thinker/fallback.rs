//! Provider fallback: try primary, fall back on transient errors.

use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use tracing::{info, warn};

use super::ProviderRegistry;

/// Call the primary provider; on transient failure, try fallbacks in order.
/// Returns (response, provider_name_used).
pub async fn call_with_fallback(
    registry: &dyn ProviderRegistry,
    primary_name: &str,
    fallbacks: &[String],
    payload: RequestPayload<'_>,
) -> Result<(ProviderResponse, String)> {
    match try_provider(registry, primary_name, &payload).await {
        Ok(resp) => return Ok((resp, primary_name.to_string())),
        Err(e) if e.is_transient() => {
            warn!(provider = primary_name, error = %e, "Primary provider transient failure");
        }
        Err(e) => return Err(e),
    }

    for name in fallbacks {
        match try_provider(registry, name, &payload).await {
            Ok(resp) => {
                info!(provider = %name, primary = primary_name, "Fallback succeeded");
                return Ok((resp, name.clone()));
            }
            Err(e) if e.is_transient() => {
                warn!(provider = %name, error = %e, "Fallback also failed");
                continue;
            }
            Err(e) => return Err(e),
        }
    }

    Err(AlephError::provider(format!(
        "All providers failed: primary '{}' + {} fallback(s)",
        primary_name, fallbacks.len()
    )))
}

async fn try_provider(
    registry: &dyn ProviderRegistry,
    name: &str,
    payload: &RequestPayload<'_>,
) -> Result<ProviderResponse> {
    let provider = registry.get(name).ok_or_else(|| {
        AlephError::provider(format!("Provider '{}' not found in registry", name))
    })?;
    provider.process(RequestPayload {
        messages: payload.messages,
        system_prompt: payload.system_prompt,
        tools: payload.tools,
        think_level: payload.think_level.clone(),
        temperature: payload.temperature,
        max_tokens: payload.max_tokens,
        tool_choice: payload.tool_choice.clone(),
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::adapter::ProviderResponse;
    use crate::providers::message::UnifiedMessage;
    use crate::sync_primitives::Arc;
    use crate::thinker::MultiProviderRegistry;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FailProvider { name: String, call_count: AtomicU32, transient: bool }
    impl crate::providers::AiProvider for FailProvider {
        fn process(&self, _: RequestPayload<'_>)
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProviderResponse>> + Send + '_>>
        {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let t = self.transient;
            Box::pin(async move {
                if t {
                    Err(AlephError::RateLimitError { message: "429".into(), suggestion: None })
                } else {
                    Err(AlephError::AuthenticationError {
                        message: "invalid".into(), provider: "t".into(), suggestion: None,
                    })
                }
            })
        }
        fn name(&self) -> &str { &self.name }
        fn color(&self) -> &str { "#000" }
    }

    struct OkProvider { name: String }
    impl crate::providers::AiProvider for OkProvider {
        fn process(&self, _: RequestPayload<'_>)
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ProviderResponse>> + Send + '_>>
        {
            Box::pin(async { Ok(ProviderResponse::text_only("ok".into())) })
        }
        fn name(&self) -> &str { &self.name }
        fn color(&self) -> &str { "#000" }
    }

    #[tokio::test]
    async fn test_primary_succeeds() {
        let r = MultiProviderRegistry::new("ok".into(), Arc::new(OkProvider { name: "ok".into() }));
        let msgs = [UnifiedMessage::user("t")];
        let (resp, used) = call_with_fallback(&r, "ok", &[], RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(resp.text_content(), "ok");
        assert_eq!(used, "ok");
    }

    #[tokio::test]
    async fn test_uses_fallback() {
        let r = MultiProviderRegistry::new(
            "fail".into(),
            Arc::new(FailProvider { name: "fail".into(), call_count: AtomicU32::new(0), transient: true }),
        );
        r.register("ok".into(), Arc::new(OkProvider { name: "ok".into() }));
        let msgs = [UnifiedMessage::user("t")];
        let (_, used) = call_with_fallback(&r, "fail", &["ok".into()], RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(used, "ok");
    }

    #[tokio::test]
    async fn test_permanent_no_retry() {
        let r = MultiProviderRegistry::new(
            "fail".into(),
            Arc::new(FailProvider { name: "fail".into(), call_count: AtomicU32::new(0), transient: false }),
        );
        r.register("ok".into(), Arc::new(OkProvider { name: "ok".into() }));
        let msgs = [UnifiedMessage::user("t")];
        assert!(call_with_fallback(&r, "fail", &["ok".into()], RequestPayload::new(&msgs)).await.is_err());
    }

    #[tokio::test]
    async fn test_all_fail() {
        let r = MultiProviderRegistry::new(
            "f1".into(),
            Arc::new(FailProvider { name: "f1".into(), call_count: AtomicU32::new(0), transient: true }),
        );
        r.register("f2".into(), Arc::new(FailProvider { name: "f2".into(), call_count: AtomicU32::new(0), transient: true }));
        let msgs = [UnifiedMessage::user("t")];
        let err = call_with_fallback(&r, "f1", &["f2".into()], RequestPayload::new(&msgs)).await.unwrap_err();
        assert!(err.to_string().contains("All providers failed"));
    }
}
