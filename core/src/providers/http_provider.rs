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
use futures::stream::BoxStream;
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
        let request = self.adapter.build_request(&payload, &self.config, false)?;
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                crate::error::AlephError::Timeout {
                    suggestion: Some("Request timed out. Try again or switch providers.".into()),
                }
            } else {
                crate::error::AlephError::network(format!("Network error: {}", e))
            }
        })?;
        self.adapter.parse_response(response).await
    }

    /// Execute a streaming request
    #[allow(dead_code)]
    async fn execute_stream(
        &self,
        payload: RequestPayload<'_>,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let request = self.adapter.build_request(&payload, &self.config, true)?;
        let response = request.send().await.map_err(|e| {
            crate::error::AlephError::network(format!("Network error: {}", e))
        })?;
        self.adapter.parse_stream(response).await
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
}
