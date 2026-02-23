//! Generic HTTP-based AI provider
//!
//! Uses a ProtocolAdapter for protocol-specific logic.

use crate::agents::thinking::ThinkLevel;
use crate::clipboard::ImageData;
use crate::config::ProviderConfig;
use crate::core::MediaAttachment;
use crate::error::Result;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::AiProvider;
use crate::secrets::leak_detector::{LeakDecision, LeakDetector};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
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
            model = %config.model,
            "Creating HttpProvider"
        );

        Ok(Self {
            name,
            config,
            adapter,
        })
    }

    /// Execute a request (non-streaming)
    async fn execute(&self, payload: RequestPayload<'_>) -> Result<String> {
        // PII filtering: filter outbound message before sending to API
        let filtered_input;
        let final_payload = if let Some(engine_lock) = crate::pii::PiiEngine::global() {
            if let Ok(engine) = engine_lock.read() {
                if !engine.is_provider_excluded(&self.name) {
                    let result = engine.filter(payload.input);
                    if result.has_detections() {
                        filtered_input = result.text;
                        RequestPayload {
                            input: &filtered_input,
                            system_prompt: payload.system_prompt,
                            image: payload.image,
                            attachments: payload.attachments,
                            think_level: payload.think_level,
                            force_standard_mode: payload.force_standard_mode,
                        }
                    } else {
                        payload
                    }
                } else {
                    payload
                }
            } else {
                payload
            }
        } else {
            // PII engine not initialized — pass through
            payload
        };

        // Secret leak detection: scan outbound content
        let detector = LeakDetector::new();
        if let LeakDecision::Block { reason, .. } = detector.scan_outbound(final_payload.input) {
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

        let request = self
            .adapter
            .build_request(&final_payload, &self.config, false)?;
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                crate::error::AlephError::Timeout {
                    suggestion: Some("Request timed out. Try again or switch providers.".into()),
                }
            } else {
                crate::error::AlephError::network(format!("Network error: {}", e))
            }
        })?;

        let response_text = self.adapter.parse_response(response).await?;

        // Secret leak detection: scan inbound response
        if let LeakDecision::Block { reason, .. } = detector.scan_inbound(&response_text) {
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

        Ok(response_text)
    }

}

impl AiProvider for HttpProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input).with_system(system_prompt.as_deref());
            self.execute(payload).await
        })
    }

    fn process_with_image(
        &self,
        input: &str,
        image: Option<&ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let image = image.cloned();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_image(image.as_ref());
            self.execute(payload).await
        })
    }

    fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let attachments = attachments.map(|a| a.to_vec());
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_attachments(attachments.as_deref());
            self.execute(payload).await
        })
    }

    fn process_with_mode(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        force_standard_mode: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_force_standard_mode(force_standard_mode);
            self.execute(payload).await
        })
    }

    fn process_with_thinking(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        think_level: ThinkLevel,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_think_level(Some(think_level));
            self.execute(payload).await
        })
    }

    fn supports_vision(&self) -> bool {
        true // OpenAI protocol supports vision
    }

    fn supports_thinking(&self) -> bool {
        let model_lower = self.config.model.to_lowercase();
        model_lower.contains("o1") || model_lower.contains("o3") || model_lower.contains("gpt-5")
    }

    fn max_think_level(&self) -> ThinkLevel {
        if self.supports_thinking() {
            ThinkLevel::High
        } else {
            ThinkLevel::Off
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
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
