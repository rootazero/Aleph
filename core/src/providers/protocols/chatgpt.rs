//! Codex Responses API protocol adapter
//!
//! Handles the Codex backend API format at chatgpt.com/backend-api/codex/responses.
//! Uses the Responses API wire format with typed SSE streaming events.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, ProviderResponse, RequestPayload};
use crate::providers::chatgpt::types::{
    InputItem, ReasoningConfig, ResponseResource, ResponsesRequest, StreamEvent,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use reqwest::Client;
use tracing::{debug, error, warn};

const CODEX_ENDPOINT: &str = "/backend-api/codex/responses";

/// Codex Responses API protocol adapter
///
/// Translates between Aleph's unified request format and the Codex
/// Responses API format used by chatgpt.com/backend-api/codex/responses.
pub struct ChatGptProtocol {
    client: Client,
}

impl ChatGptProtocol {
    /// Create a new Codex protocol adapter with the given HTTP client
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL from provider configuration
    fn build_endpoint(config: &ProviderConfig) -> String {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://chatgpt.com".to_string());
        format!("{}{}", base_url, CODEX_ENDPOINT)
    }

    /// Map Aleph ThinkLevel to Responses API reasoning config
    fn build_reasoning(payload: &RequestPayload) -> Option<ReasoningConfig> {
        use crate::agents::thinking::ThinkLevel;
        match payload.think_level {
            Some(ThinkLevel::Low) => Some(ReasoningConfig {
                effort: Some("low".to_string()),
                summary: Some("auto".to_string()),
            }),
            Some(ThinkLevel::Medium) => Some(ReasoningConfig {
                effort: Some("medium".to_string()),
                summary: Some("auto".to_string()),
            }),
            Some(ThinkLevel::High) => Some(ReasoningConfig {
                effort: Some("high".to_string()),
                summary: Some("auto".to_string()),
            }),
            _ => None,
        }
    }

    /// Build a Responses API request from the unified payload
    pub fn build_responses_request(
        payload: &RequestPayload,
        model: &str,
    ) -> ResponsesRequest {
        let input = vec![InputItem::Message {
            role: "user".to_string(),
            content: payload.input.to_string(),
        }];

        ResponsesRequest {
            model: model.to_string(),
            input,
            instructions: payload.system_prompt.map(|s| s.to_string()),
            stream: true,
            store: false,
            reasoning: Self::build_reasoning(payload),
        }
    }

    /// Extract text content from a completed ResponseResource
    fn extract_text(response: &ResponseResource) -> Option<String> {
        let mut texts = Vec::new();
        for item in &response.output {
            match item {
                crate::providers::chatgpt::types::OutputItem::Message { content, .. } => {
                    for part in content {
                        if !part.text.is_empty() {
                            texts.push(part.text.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        if texts.is_empty() {
            None
        } else {
            Some(texts.join(""))
        }
    }

    /// Parse a single SSE data line into a StreamEvent
    fn parse_sse_data(data: &str) -> Option<StreamEvent> {
        if data == "[DONE]" {
            return None;
        }
        serde_json::from_str(data).ok()
    }
}

#[async_trait]
impl ProtocolAdapter for ChatGptProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        _is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let request = Self::build_responses_request(payload, &config.model);

        let access_token = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("Codex access token not set — run OAuth login first")
        })?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            "Building Codex Responses API request"
        );

        let builder = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request);

        Ok(builder)
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "Codex API error");
            if status.as_u16() == 401 {
                return Err(AlephError::provider(
                    "Codex authentication expired — please re-login",
                ));
            }
            if status.as_u16() == 429 {
                return Err(AlephError::provider(
                    "Codex subscription rate limit reached — please try again later",
                ));
            }
            return Err(AlephError::provider(format!(
                "Codex API error ({}): {}",
                status, error_text
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to read Codex response: {}", e)))?;

        // Parse SSE events, looking for the Completed event with full response
        let mut result = String::new();
        for line in text.lines() {
            let data = if let Some(d) = line.strip_prefix("data: ") {
                d
            } else {
                continue;
            };
            if let Some(event) = Self::parse_sse_data(data) {
                match event {
                    StreamEvent::TextDelta { delta, .. } => {
                        result.push_str(&delta);
                    }
                    StreamEvent::Completed { ref response } => {
                        // Prefer extracting from completed response for accuracy
                        if let Some(full_text) = Self::extract_text(response) {
                            result = full_text;
                        }
                    }
                    StreamEvent::Failed { response } => {
                        let msg = response
                            .error
                            .map(|e| format!("{}: {}", e.code, e.message))
                            .unwrap_or_else(|| "Unknown error".to_string());
                        return Err(AlephError::provider(format!("Codex request failed: {}", msg)));
                    }
                    _ => {}
                }
            }
        }

        if result.is_empty() {
            Err(AlephError::provider("Empty response from Codex"))
        } else {
            Ok(ProviderResponse::text_only(result))
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
                "Codex API error ({}): {}",
                status, error_text
            )));
        }

        // Buffer for incomplete SSE lines across chunks
        let line_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        let stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .try_filter_map(move |chunk| {
                let buf = line_buf.clone();
                async move {
                    let text = std::str::from_utf8(&chunk)
                        .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                    let mut buf_guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                    buf_guard.push_str(text);

                    let mut delta = String::new();

                    // Process complete lines from buffer
                    while let Some(newline_pos) = buf_guard.find('\n') {
                        let line = buf_guard[..newline_pos].trim_end().to_string();
                        buf_guard.drain(..=newline_pos);

                        let data = if let Some(d) = line.strip_prefix("data: ") {
                            d
                        } else {
                            continue;
                        };

                        if let Some(event) = Self::parse_sse_data(data) {
                            match event {
                                StreamEvent::TextDelta { delta: d, .. } => {
                                    delta.push_str(&d);
                                }
                                StreamEvent::Failed { response } => {
                                    let msg = response
                                        .error
                                        .map(|e| format!("{}: {}", e.code, e.message))
                                        .unwrap_or_else(|| "Unknown error".to_string());
                                    warn!(error = %msg, "Codex stream failed");
                                }
                                _ => {}
                            }
                        }
                    }

                    if delta.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(delta))
                    }
                }
            });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "chatgpt"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_responses_request_basic() {
        let payload = RequestPayload::new("Hello");
        let request = ChatGptProtocol::build_responses_request(&payload, "codex-mini-latest");

        assert_eq!(request.model, "codex-mini-latest");
        assert!(!request.store);
        assert!(request.stream);
        assert!(request.instructions.is_none());
        assert!(request.reasoning.is_none());
        assert_eq!(request.input.len(), 1);
        match &request.input[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content, "Hello");
            }
        }
    }

    #[test]
    fn test_build_responses_request_with_system_prompt() {
        let payload = RequestPayload::new("Hello").with_system(Some("You are helpful"));
        let request = ChatGptProtocol::build_responses_request(&payload, "codex-mini-latest");

        assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
        // System prompt goes to instructions, NOT prepended to user message
        match &request.input[0] {
            InputItem::Message { content, .. } => {
                assert_eq!(content, "Hello");
                assert!(!content.contains("You are helpful"));
            }
        }
    }

    #[test]
    fn test_build_responses_request_with_reasoning() {
        use crate::agents::thinking::ThinkLevel;
        let payload = RequestPayload::new("Think about this")
            .with_think_level(Some(ThinkLevel::High));
        let request = ChatGptProtocol::build_responses_request(&payload, "codex-mini-latest");

        let reasoning = request.reasoning.unwrap();
        assert_eq!(reasoning.effort.as_deref(), Some("high"));
        assert_eq!(reasoning.summary.as_deref(), Some("auto"));
    }

    #[test]
    fn test_parse_sse_data_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
        let event = ChatGptProtocol::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_done() {
        let result = ChatGptProtocol::parse_sse_data("[DONE]");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_completed() {
        let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
        let event = ChatGptProtocol::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                let text = ChatGptProtocol::extract_text(&response);
                assert_eq!(text, Some("Hello world".to_string()));
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_text_from_response() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![crate::providers::chatgpt::types::OutputItem::Message {
                id: "msg_1".to_string(),
                role: "assistant".to_string(),
                content: vec![crate::providers::chatgpt::types::ContentPart {
                    part_type: "output_text".to_string(),
                    text: "Test output".to_string(),
                }],
            }],
            usage: None,
            error: None,
        };
        assert_eq!(
            ChatGptProtocol::extract_text(&response),
            Some("Test output".to_string())
        );
    }

    #[test]
    fn test_extract_text_empty_output() {
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![],
            usage: None,
            error: None,
        };
        assert_eq!(ChatGptProtocol::extract_text(&response), None);
    }

    #[test]
    fn test_adapter_name() {
        let adapter = ChatGptProtocol::new(Client::new());
        assert_eq!(adapter.name(), "chatgpt");
    }

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("codex-mini-latest");
        let endpoint = ChatGptProtocol::build_endpoint(&config);
        assert!(endpoint.ends_with("/backend-api/codex/responses"));
    }

    #[test]
    fn test_create_provider_via_factory() {
        use crate::config::ProviderConfig;
        use crate::providers::create_provider;

        let config = ProviderConfig {
            protocol: Some("chatgpt".to_string()),
            model: "codex-mini-latest".to_string(),
            api_key: Some("test_token".to_string()),
            base_url: Some("https://chatgpt.com".to_string()),
            enabled: true,
            ..ProviderConfig::test_config("codex-mini-latest")
        };

        let provider = create_provider("chatgpt-sub", config);
        assert!(
            provider.is_ok(),
            "Should create chatgpt provider: {:?}",
            provider.err()
        );
    }

    #[test]
    fn test_chatgpt_preset() {
        use crate::providers::presets::get_preset;

        let preset = get_preset("chatgpt");
        assert!(preset.is_some(), "chatgpt preset should exist");

        let p = preset.unwrap();
        assert_eq!(p.protocol, "chatgpt");
        assert_eq!(p.default_model, "codex-mini-latest");
    }
}
