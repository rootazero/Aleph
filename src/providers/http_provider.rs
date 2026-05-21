//! Generic HTTP-based AI provider
//!
//! Uses a ProtocolAdapter for protocol-specific logic.

use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::adapter::{
    ProtocolAdapter, ProviderResponse, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::{AiProvider, ProviderDelta};
use crate::secrets::leak_detector::{LeakDecision, LeakDetector};
use crate::sync_primitives::Arc;
use futures::StreamExt;
use std::future::Future;
use std::pin::Pin;
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

    /// Apply outbound safety checks (PII filtering + secret leak detection).
    /// Returns filtered messages or a leak block reason.
    fn apply_outbound_safety(
        &self,
        messages: &[UnifiedMessage],
    ) -> std::result::Result<Vec<UnifiedMessage>, String> {
        let mut filtered_messages: Vec<UnifiedMessage> = messages.to_vec();

        // PII filtering: filter each text block individually
        if let Some(engine_lock) = crate::pii::PiiEngine::global() {
            if let Ok(engine) = engine_lock.read() {
                if !engine.is_provider_excluded(&self.name) {
                    for msg in &mut filtered_messages {
                        for block in msg.content_blocks_mut() {
                            if let ContentBlock::Text { ref mut text, .. } = block {
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
            return Err(reason);
        }

        Ok(filtered_messages)
    }

    /// Execute a request (non-streaming)
    async fn execute(&self, payload: RequestPayload<'_>) -> Result<ProviderResponse> {
        let filtered_messages = match self.apply_outbound_safety(payload.messages) {
            Ok(msgs) => msgs,
            Err(reason) => {
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
        };

        let final_payload = RequestPayload {
            messages: &filtered_messages,
            system_prompt: payload.system_prompt,
            tools: payload.tools,
            think_level: payload.think_level,
            temperature: payload.temperature,
            max_tokens: payload.max_tokens,
            tool_choice: payload.tool_choice.clone(),
            model: payload.model.clone(),
            metadata: payload.metadata.clone(),
        };

        // Extension hooks observe LLM provider traffic for cost metering.
        let session_id = hook_session_id(&payload);
        let base_env = self.base_request_env(&payload, false);
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PreApiRequest,
            &session_id,
            base_env.clone(),
        )
        .await;

        let request = self.adapter.build_request(&final_payload, &self.config)?;
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

        // PostApiRequest fires once the response (and its token usage) is in
        // hand — before the inbound leak check, so the cost meter records the
        // request even when the response is later blocked.
        let mut post_env = base_env;
        if let Some(ref usage) = provider_response.usage {
            append_usage_env(&mut post_env, usage);
        }
        post_env.push((
            "STOP_REASON",
            format!("{:?}", provider_response.stop_reason),
        ));
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PostApiRequest,
            &session_id,
            post_env,
        )
        .await;

        // Secret leak detection: scan inbound response TEXT only
        let detector = LeakDetector::new();
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
    ) -> anyhow::Result<
        futures::stream::BoxStream<'static, anyhow::Result<crate::providers::ProviderDelta>>,
    > {
        let filtered_messages = self
            .apply_outbound_safety(payload.messages)
            .map_err(|reason| anyhow::anyhow!("Secret leak blocked: {}", reason))?;

        let final_payload = RequestPayload {
            messages: &filtered_messages,
            system_prompt: payload.system_prompt,
            tools: payload.tools,
            think_level: payload.think_level,
            temperature: payload.temperature,
            max_tokens: payload.max_tokens,
            tool_choice: payload.tool_choice.clone(),
            model: payload.model.clone(),
            metadata: payload.metadata.clone(),
        };

        // Extension hooks observe LLM provider traffic for cost metering.
        let session_id = hook_session_id(&payload);
        let base_env = self.base_request_env(&payload, true);
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PreApiRequest,
            &session_id,
            base_env.clone(),
        )
        .await;

        let request = self
            .adapter
            .build_request(&final_payload, &self.config)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let response = request
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Network error: {}", e))?;
        let stream = self
            .adapter
            .stream_deltas(response)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let inner = stream
            .map(|r| r.map_err(|e| anyhow::anyhow!("{}", e)))
            .boxed();

        // Wrap the delta stream so `PostApiRequest` fires once — with the
        // accumulated token usage — when the stream completes naturally. An
        // aborted stream (client disconnect) skips the post hook.
        let wrapped = async_stream::stream! {
            let mut inner = inner;
            let mut usage: Option<TokenUsage> = None;
            let mut stop_reason: Option<StopReason> = None;
            while let Some(item) = inner.next().await {
                if let Ok(delta) = &item {
                    match delta {
                        ProviderDelta::Usage(u) => {
                            usage = Some(match usage.take() {
                                Some(prev) => {
                                    crate::providers::delta::merge_usage(prev, u.clone())
                                }
                                None => u.clone(),
                            });
                        }
                        ProviderDelta::Done(sr) => stop_reason = Some(sr.clone()),
                        _ => {}
                    }
                }
                yield item;
            }
            let mut env = base_env;
            if let Some(ref u) = usage {
                append_usage_env(&mut env, u);
            }
            if let Some(ref sr) = stop_reason {
                env.push(("STOP_REASON", format!("{:?}", sr)));
            }
            crate::extension::hooks::fire_global_observer(
                crate::extension::HookEvent::PostApiRequest,
                &session_id,
                env,
            )
            .await;
        };

        Ok(wrapped.boxed())
    }

    /// Build the env shared by `PreApiRequest` / `PostApiRequest` hooks.
    fn base_request_env(
        &self,
        payload: &RequestPayload<'_>,
        streaming: bool,
    ) -> Vec<(&'static str, String)> {
        let model = payload
            .model
            .clone()
            .unwrap_or_else(|| self.config.default_model().to_string());
        vec![
            ("PROVIDER_NAME", self.name.clone()),
            ("MODEL", model),
            ("PROTOCOL", self.adapter.name().to_string()),
            ("STREAMING", streaming.to_string()),
            ("MESSAGE_COUNT", payload.messages.len().to_string()),
        ]
    }
}

/// Resolve a session id for API-request hooks. Uses the `session_id` metadata
/// key when a caller threaded it through; otherwise a synthetic id so the cost
/// meter still aggregates by provider/model.
fn hook_session_id(payload: &RequestPayload<'_>) -> String {
    payload
        .metadata
        .as_ref()
        .and_then(|m| m.get("session_id"))
        .cloned()
        .unwrap_or_else(|| "provider".to_string())
}

/// Append `TokenUsage` figures to a `PostApiRequest` hook env.
fn append_usage_env(env: &mut Vec<(&'static str, String)>, usage: &TokenUsage) {
    env.push(("INPUT_TOKENS", usage.input_tokens.to_string()));
    env.push(("OUTPUT_TOKENS", usage.output_tokens.to_string()));
    if let Some(v) = usage.cache_read_tokens {
        env.push(("CACHE_READ_TOKENS", v.to_string()));
    }
    if let Some(v) = usage.cache_creation_tokens {
        env.push(("CACHE_CREATION_TOKENS", v.to_string()));
    }
    if let Some(v) = usage.thinking_tokens {
        env.push(("THINKING_TOKENS", v.to_string()));
    }
    if let Some(ref cost) = usage.cost {
        env.push(("COST_USD", format!("{:.6}", cost.calculate(usage))));
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

    fn protocol(&self) -> &str {
        self.adapter.name()
    }

    fn model_behavior_override(&self) -> Option<&str> {
        self.config.model_behavior.as_deref()
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

    #[test]
    fn hook_session_id_prefers_metadata_then_falls_back() {
        use crate::providers::adapter::RequestPayload;

        let mut meta = std::collections::HashMap::new();
        meta.insert("session_id".to_string(), "sess-42".to_string());
        let payload = RequestPayload {
            metadata: Some(meta),
            ..Default::default()
        };
        assert_eq!(super::hook_session_id(&payload), "sess-42");

        // No metadata → synthetic id so the cost meter still aggregates.
        assert_eq!(
            super::hook_session_id(&RequestPayload::default()),
            "provider"
        );
    }

    #[test]
    fn append_usage_env_emits_present_fields_only() {
        use crate::providers::adapter::TokenUsage;

        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: Some(10),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        };
        let mut env: Vec<(&'static str, String)> = Vec::new();
        super::append_usage_env(&mut env, &usage);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();

        assert_eq!(map.get("INPUT_TOKENS"), Some(&"100".to_string()));
        assert_eq!(map.get("OUTPUT_TOKENS"), Some(&"50".to_string()));
        assert_eq!(map.get("CACHE_READ_TOKENS"), Some(&"10".to_string()));
        // Absent Option fields must not emit env keys.
        assert!(!map.contains_key("CACHE_CREATION_TOKENS"));
        assert!(!map.contains_key("THINKING_TOKENS"));
        assert!(!map.contains_key("COST_USD"));
    }
}
