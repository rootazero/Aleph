//! Per-chunk idle timeout for streaming LLM responses.
//!
//! A streaming response can stall mid-flight — the upstream stops sending SSE
//! bytes after a network blip or proxy hang — and the agent turn would hang
//! indefinitely. `wrap_idle_timeout` aborts the stream when no chunk arrives
//! within `idle_secs`, surfacing `AlephError::Timeout` so the caller's
//! existing transient-error path handles it.

use axum::body::Bytes;
use futures::stream::BoxStream;

use crate::error::{AlephError, Result};

/// Built-in idle timeout (seconds) when `ProviderConfig.stream_idle_timeout_secs`
/// is unset. A stalled SSE stream that sends no byte for this long is aborted.
pub(crate) const DEFAULT_STREAM_IDLE_SECS: u64 = 60;

/// Resolve the effective idle timeout from provider config: the configured
/// value, or `DEFAULT_STREAM_IDLE_SECS` when unset.
pub(crate) fn effective_idle_secs(config: &crate::config::ProviderConfig) -> u64 {
    config
        .stream_idle_timeout_secs
        .unwrap_or(DEFAULT_STREAM_IDLE_SECS)
}

/// Wrap a byte stream so a gap longer than `idle_secs` between chunks yields
/// `AlephError::Timeout`. `idle_secs == 0` disables the wrap (returns the
/// stream unchanged). `provider_label` names the upstream in the error
/// message (e.g. `"OpenAI"`, `"Gemini"`, `"Anthropic"`).
pub(crate) fn wrap_idle_timeout(
    stream: BoxStream<'static, Result<Bytes>>,
    idle_secs: u64,
    provider_label: &'static str,
) -> BoxStream<'static, Result<Bytes>> {
    if idle_secs == 0 {
        return stream;
    }
    use tokio_stream::StreamExt as _;
    let timed = stream.timeout(std::time::Duration::from_secs(idle_secs));
    let mapped = futures::StreamExt::map(timed, move |res| match res {
        Ok(inner) => inner,
        Err(_elapsed) => Err(AlephError::Timeout {
            suggestion: Some(format!(
                "{provider_label} stream stalled: no SSE event received for \
                 {idle_secs}s. The upstream may be unresponsive; retry or \
                 increase ProviderConfig.stream_idle_timeout_secs."
            )),
        }),
    });
    Box::pin(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    #[tokio::test]
    async fn idle_timeout_fires_after_threshold() {
        let pending: BoxStream<'static, Result<Bytes>> = futures::stream::pending().boxed();
        let mut wrapped = wrap_idle_timeout(pending, 1, "TestProvider");
        let first = wrapped.next().await;
        assert!(
            matches!(first, Some(Err(AlephError::Timeout { .. }))),
            "stalled stream must yield AlephError::Timeout, got {first:?}",
        );
    }

    #[tokio::test]
    async fn idle_timeout_resets_on_event() {
        let s: BoxStream<'static, Result<Bytes>> =
            futures::stream::once(async { Ok(Bytes::from_static(b"data: x\n")) }).boxed();
        let mut wrapped = wrap_idle_timeout(s, 10, "TestProvider");
        let first = wrapped.next().await;
        assert!(
            matches!(first, Some(Ok(_))),
            "a prompt chunk must pass through, got {first:?}",
        );
    }

    #[tokio::test]
    async fn idle_timeout_zero_disables() {
        let pending: BoxStream<'static, Result<Bytes>> = futures::stream::pending().boxed();
        let mut wrapped = wrap_idle_timeout(pending, 0, "TestProvider");
        let raced = tokio::time::timeout(std::time::Duration::from_millis(50), wrapped.next()).await;
        assert!(raced.is_err(), "idle_secs==0 must not inject any timeout");
    }

    #[tokio::test]
    async fn timeout_message_carries_provider_label() {
        let pending: BoxStream<'static, Result<Bytes>> = futures::stream::pending().boxed();
        let mut wrapped = wrap_idle_timeout(pending, 1, "Gemini");
        match wrapped.next().await {
            Some(Err(AlephError::Timeout { suggestion: Some(msg) })) => {
                assert!(msg.contains("Gemini"), "label must appear in: {msg}");
            }
            other => panic!("expected labelled Timeout, got {other:?}"),
        }
    }

    #[test]
    fn effective_idle_secs_defaults_to_60_when_unset() {
        let config = crate::config::ProviderConfig::test_config("any-model");
        assert_eq!(effective_idle_secs(&config), 60);
    }

    #[test]
    fn effective_idle_secs_uses_configured_value() {
        let mut config = crate::config::ProviderConfig::test_config("any-model");
        config.stream_idle_timeout_secs = Some(15);
        assert_eq!(effective_idle_secs(&config), 15);
    }
}
