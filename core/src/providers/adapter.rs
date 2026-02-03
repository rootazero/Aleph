//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::agents::thinking::ThinkLevel;
use crate::clipboard::ImageData;
use crate::config::ProviderConfig;
use crate::core::MediaAttachment;
use crate::error::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

/// Unified request payload for protocol adapters
///
/// This DTO (Data Transfer Object) contains all possible inputs for an LLM request.
/// Protocol adapters translate this into provider-specific request formats.
#[derive(Debug, Default)]
pub struct RequestPayload<'a> {
    /// Core text input (user message)
    pub input: &'a str,

    /// System prompt (optional)
    pub system_prompt: Option<&'a str>,

    /// Legacy image format (for process_with_image compatibility)
    pub image: Option<&'a ImageData>,

    /// Multimodal attachments (for process_with_attachments compatibility)
    pub attachments: Option<&'a [MediaAttachment]>,

    /// Thinking/reasoning level configuration
    pub think_level: Option<ThinkLevel>,

    /// Force standard mode for system prompt handling
    pub force_standard_mode: bool,
}

impl<'a> RequestPayload<'a> {
    /// Create a new payload with input text
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Add system prompt
    pub fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Add legacy image
    pub fn with_image(mut self, image: Option<&'a ImageData>) -> Self {
        self.image = image;
        self
    }

    /// Add multimodal attachments
    pub fn with_attachments(mut self, attachments: Option<&'a [MediaAttachment]>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Add thinking level
    pub fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set force standard mode
    pub fn with_force_standard_mode(mut self, force: bool) -> Self {
        self.force_standard_mode = force;
        self
    }
}

/// Protocol adapter trait for building requests and parsing responses
///
/// Each protocol (OpenAI, Anthropic, Gemini, etc.) implements this trait
/// to handle protocol-specific serialization and deserialization.
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Build an HTTP request from the payload
    ///
    /// # Arguments
    /// * `payload` - The unified request payload
    /// * `config` - Provider configuration (API key, model, etc.)
    /// * `is_streaming` - Whether to enable streaming response
    ///
    /// # Returns
    /// A configured reqwest::RequestBuilder ready to send
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder>;

    /// Parse a non-streaming response
    ///
    /// # Arguments
    /// * `response` - The HTTP response from the API
    ///
    /// # Returns
    /// The extracted text content from the response
    async fn parse_response(&self, response: reqwest::Response) -> Result<String>;

    /// Parse a streaming response (SSE)
    ///
    /// # Arguments
    /// * `response` - The HTTP response with chunked body
    ///
    /// # Returns
    /// A stream of text chunks
    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>>;

    /// Get the protocol name for logging
    fn name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_builder() {
        let payload = RequestPayload::new("Hello")
            .with_system(Some("You are helpful"))
            .with_think_level(Some(ThinkLevel::Medium));

        assert_eq!(payload.input, "Hello");
        assert_eq!(payload.system_prompt, Some("You are helpful"));
        assert!(payload.think_level.is_some());
        assert!(!payload.force_standard_mode);
    }

    #[test]
    fn test_payload_default() {
        let payload = RequestPayload::new("Test");
        assert_eq!(payload.input, "Test");
        assert!(payload.system_prompt.is_none());
        assert!(payload.image.is_none());
        assert!(payload.attachments.is_none());
        assert!(payload.think_level.is_none());
    }
}
