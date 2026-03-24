//! OpenAI Responses API protocol adapter
//!
//! Handles the standard OpenAI Responses API at /v1/responses.
//! Uses the shared Responses API wire format with typed SSE streaming events.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, ProviderResponse, RequestPayload, StopReason};
use crate::providers::responses::shared;
use crate::providers::responses::types::ResponsesRequest;
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;
use tracing::{debug, error};

/// OpenAI Responses API protocol adapter
///
/// Translates between Aleph's unified request format and the standard
/// OpenAI Responses API format at /v1/responses.
pub struct OpenAiResponsesProtocol {
    client: Client,
}

impl OpenAiResponsesProtocol {
    /// Create a new OpenAI Responses protocol adapter with the given HTTP client
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL from provider configuration
    ///
    /// Normalizes the base_url by stripping trailing `/v1` and appending `/v1/responses`.
    /// Default: `https://api.openai.com/v1/responses`
    pub fn build_endpoint(config: &ProviderConfig) -> String {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| {
                let trimmed = s.trim_end_matches('/');
                trimmed.trim_end_matches("/v1").to_string()
            })
            .unwrap_or_else(|| "https://api.openai.com".to_string());
        format!("{}/v1/responses", base_url)
    }

    /// Build a Responses API request from the unified payload
    ///
    /// Uses shared conversion functions. Unlike CodexProtocol, this sets
    /// store=None, text=None, include=None (no Codex-specific fields).
    pub fn build_responses_request(
        payload: &RequestPayload,
        model: &str,
    ) -> ResponsesRequest {
        let input = shared::convert_messages(payload.messages);
        let tools = shared::build_tools(payload.tools);
        let tool_choice = shared::map_tool_choice(payload.tool_choice.as_ref())
            .or(Some("auto".to_string()));

        ResponsesRequest {
            model: model.to_string(),
            input,
            instructions: payload.system_prompt.map(|s| s.to_string()),
            stream: true,
            store: None,
            reasoning: shared::build_reasoning(payload.think_level),
            tools,
            tool_choice,
            parallel_tool_calls: Some(true),
            text: None,
            max_output_tokens: payload.max_tokens,
            include: None,
        }
    }
}

#[async_trait]
impl ProtocolAdapter for OpenAiResponsesProtocol {
    fn supports_native_tools(&self) -> bool {
        true
    }

    fn supports_strict_schema(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "openai-responses"
    }

    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        _is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let request = Self::build_responses_request(payload, config.default_model());

        let api_key = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("OpenAI API key not set")
        })?;

        if let Ok(json) = serde_json::to_string_pretty(&request) {
            debug!(
                endpoint = %endpoint,
                model = %config.default_model(),
                request_body = %json,
                "Building OpenAI Responses API request"
            );
        }

        let builder = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request);

        Ok(builder)
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "OpenAI Responses API error");
            if status.as_u16() == 401 {
                return Err(AlephError::provider(
                    "OpenAI authentication failed — check your API key",
                ));
            }
            if status.as_u16() == 429 {
                return Err(AlephError::provider(
                    "OpenAI rate limit reached — please try again later",
                ));
            }
            return Err(AlephError::provider(format!(
                "OpenAI Responses API error ({}): {}",
                status, error_text
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to read OpenAI response: {}", e)))?;

        let (result, tool_calls, is_incomplete, usage) = shared::parse_sse_body(&text)?;

        let stop_reason = if is_incomplete {
            StopReason::MaxTokens
        } else if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };

        if !tool_calls.is_empty() {
            Ok(ProviderResponse {
                text: if result.is_empty() { None } else { Some(result) },
                tool_calls,
                stop_reason,
                usage,
                ..Default::default()
            })
        } else if result.is_empty() {
            Err(AlephError::provider("Empty response from OpenAI Responses API"))
        } else {
            Ok(ProviderResponse {
                text: Some(result),
                stop_reason,
                usage,
                ..Default::default()
            })
        }
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "OpenAI Responses API error ({}): {}",
                status, error_text
            )));
        }

        shared::build_sse_stream(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.openai.com/v1/responses");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://custom.api.com/v1".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://custom.api.com/v1/responses");
    }

    #[test]
    fn test_build_endpoint_openrouter() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://openrouter.ai/api/v1".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://openrouter.ai/api/v1/responses");
    }

    #[test]
    fn test_build_endpoint_trailing_slash() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://api.example.com/v1/".to_string());
        let endpoint = OpenAiResponsesProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.example.com/v1/responses");
    }

    #[test]
    fn test_build_responses_request_basic() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let request = OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o");

        assert_eq!(request.model, "gpt-4o");
        assert!(request.stream);
        assert!(request.store.is_none());
        assert!(request.text.is_none());
        assert!(request.include.is_none());
        assert!(request.instructions.is_none());
        assert!(request.reasoning.is_none());
        assert_eq!(request.input.len(), 1);
    }

    #[test]
    fn test_build_responses_request_with_system() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("You are helpful"));
        let request = OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o");

        assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
    }

    #[test]
    fn test_build_responses_request_with_reasoning() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Think about this")];
        let payload = RequestPayload::new(&msgs)
            .with_think_level(Some(ThinkLevel::High));
        let request = OpenAiResponsesProtocol::build_responses_request(&payload, "gpt-4o");

        let reasoning = request.reasoning.unwrap();
        assert_eq!(reasoning.effort.as_deref(), Some("high"));
        assert_eq!(reasoning.summary.as_deref(), Some("auto"));
    }

    #[test]
    fn test_adapter_name() {
        let adapter = OpenAiResponsesProtocol::new(Client::new());
        assert_eq!(adapter.name(), "openai-responses");
    }

    #[test]
    fn test_supports_native_tools() {
        let adapter = OpenAiResponsesProtocol::new(Client::new());
        assert!(adapter.supports_native_tools());
    }
}
