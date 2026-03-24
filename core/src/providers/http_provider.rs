//! Generic HTTP-based AI provider
//!
//! Uses a ProtocolAdapter for protocol-specific logic.

use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::adapter::{ProtocolAdapter, ProviderResponse, RequestPayload};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use crate::secrets::leak_detector::{LeakDecision, LeakDetector};
use futures::StreamExt;
use std::future::Future;
use std::pin::Pin;
use crate::sync_primitives::Arc;
use tracing::debug;

/// Generic HTTP-based AI provider
///
/// This provider uses a ProtocolAdapter for protocol-specific request/response handling.
/// It implements the AiProvider trait by delegating to the adapter.
pub struct HttpProvider {
    name: String,
    config: ProviderConfig,
    adapter: Arc<dyn ProtocolAdapter>,
}

impl std::fmt::Debug for HttpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpProvider")
            .field("name", &self.name)
            .field("protocol", &self.adapter.name())
            .finish_non_exhaustive()
    }
}

impl HttpProvider {
    /// Create a new HttpProvider with the given adapter
    pub fn new(
        name: String,
        config: ProviderConfig,
        adapter: Arc<dyn ProtocolAdapter>,
    ) -> Result<Self> {
        debug!(
            name = %name,
            protocol = adapter.name(),
            model = %config.default_model(),
            "Creating HttpProvider"
        );

        Ok(Self {
            name,
            config,
            adapter,
        })
    }

    /// Execute a request (non-streaming)
    async fn execute(&self, payload: RequestPayload<'_>) -> Result<ProviderResponse> {
        // PII filtering: filter each text block individually
        let mut filtered_messages: Vec<UnifiedMessage> = payload.messages.to_vec();
        if let Some(engine_lock) = crate::pii::PiiEngine::global() {
            if let Ok(engine) = engine_lock.read() {
                if !engine.is_provider_excluded(&self.name) {
                    for msg in &mut filtered_messages {
                        for block in msg.content_blocks_mut() {
                            if let ContentBlock::Text { ref mut text } = block {
                                let result = engine.filter(text);
                                if result.has_detections() {
                                    *text = result.text;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Secret leak detection: scan all text content
        let detector = LeakDetector::new();
        let all_text = UnifiedMessage::extract_all_text(&filtered_messages);
        if let LeakDecision::Block { reason, .. } = detector.scan_outbound(&all_text) {
            tracing::warn!(
                provider = %self.name,
                reason = %reason,
                "Blocked outbound request: secret leak detected"
            );
            return Err(crate::error::AlephError::PermissionDenied {
                message: format!("Secret leak blocked: {}", reason),
                suggestion: Some("Remove secret values from the input before sending.".into()),
            });
        }

        let final_payload = RequestPayload {
            messages: &filtered_messages,
            system_prompt: payload.system_prompt,
            tools: payload.tools,
            think_level: payload.think_level,
            temperature: payload.temperature,
            max_tokens: payload.max_tokens,
            tool_choice: payload.tool_choice.clone(),
        };

        let request = self
            .adapter
            .build_request(&final_payload, &self.config)?;
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                crate::error::AlephError::Timeout {
                    suggestion: Some("Request timed out. Try again or switch providers.".into()),
                }
            } else {
                crate::error::AlephError::network(format!("Network error: {}", e))
            }
        })?;

        // Collect streaming deltas into a ProviderResponse
        let stream = self.adapter.stream_deltas(response).await?;
        let mut collector = crate::providers::DeltaCollector::new();
        futures::pin_mut!(stream);
        while let Some(delta) = stream.next().await {
            collector.push(delta?);
        }
        let provider_response = collector.finish();

        // Validate response
        provider_response.validate(self.adapter.name());

        // Secret leak detection: scan inbound response TEXT only
        if let Some(ref text) = provider_response.text {
            if let LeakDecision::Block { reason, .. } = detector.scan_inbound(text) {
                tracing::warn!(
                    provider = %self.name,
                    reason = %reason,
                    "Blocked inbound response: secret leak detected"
                );
                return Err(crate::error::AlephError::PermissionDenied {
                    message: format!("Secret leak in response blocked: {}", reason),
                    suggestion: Some("The AI provider response contained a secret value.".into()),
                });
            }
        }

        Ok(provider_response)
    }

    /// Expose raw delta stream with outbound safety checks applied.
    ///
    /// Used by AiProviderBridge for real streaming to AgentLoop.
    /// Inbound leak check is deferred to the DeltaCollector consumer.
    pub async fn stream_raw<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> anyhow::Result<futures::stream::BoxStream<'static, anyhow::Result<crate::providers::ProviderDelta>>> {
        // PII filtering
        let mut filtered_messages: Vec<UnifiedMessage> = payload.messages.to_vec();
        if let Some(engine_lock) = crate::pii::PiiEngine::global() {
            if let Ok(engine) = engine_lock.read() {
                if !engine.is_provider_excluded(&self.name) {
                    for msg in &mut filtered_messages {
                        for block in msg.content_blocks_mut() {
                            if let ContentBlock::Text { ref mut text } = block {
                                let result = engine.filter(text);
                                if result.has_detections() {
                                    *text = result.text;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Secret leak detection outbound
        let detector = LeakDetector::new();
        let all_text = UnifiedMessage::extract_all_text(&filtered_messages);
        if let LeakDecision::Block { reason, .. } = detector.scan_outbound(&all_text) {
            return Err(anyhow::anyhow!("Secret leak blocked: {}", reason));
        }

        let final_payload = RequestPayload {
            messages: &filtered_messages,
            system_prompt: payload.system_prompt,
            tools: payload.tools,
            think_level: payload.think_level,
            temperature: payload.temperature,
            max_tokens: payload.max_tokens,
            tool_choice: payload.tool_choice.clone(),
        };

        let request = self.adapter.build_request(&final_payload, &self.config)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let response = request.send().await
            .map_err(|e| anyhow::anyhow!("Network error: {}", e))?;
        let stream = self.adapter.stream_deltas(response).await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        Ok(stream.map(|r| r.map_err(|e| anyhow::anyhow!("{}", e))).boxed())
    }

}

impl AiProvider for HttpProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { self.execute(payload).await })
    }

    fn supports_native_tools(&self) -> bool {
        self.adapter.supports_native_tools()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }

    fn as_http_provider(&self) -> Option<&HttpProvider> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_http_provider_creation() {
        // This test just verifies the type compiles correctly
        // Actual functionality tested via integration tests
    }

    #[test]
    fn test_pii_filtering_integration() {
        use crate::config::PrivacyConfig;
        use crate::pii::PiiEngine;

        let engine = PiiEngine::new(PrivacyConfig::default());
        let result = engine.filter("User: Call 13812345678 for info");
        assert!(result.text.contains("[PHONE]"));
        assert!(!result.text.contains("13812345678"));
    }
}
