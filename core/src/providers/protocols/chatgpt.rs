//! ChatGPT backend-api protocol adapter
//!
//! Handles the ChatGPT subscription API format (chatgpt.com/backend-api).
//! This is NOT the official OpenAI API — it uses the ChatGPT web backend
//! with OAuth authentication instead of API keys.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::chatgpt::types::{
    Author, ChatGptContent, ChatGptMessage, ChatGptRequest, ChatGptStreamResponse,
    ConversationMode,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use reqwest::Client;
use tracing::{debug, error};

const CONVERSATION_ENDPOINT: &str = "/backend-api/conversation";

/// ChatGPT backend-api protocol adapter
///
/// Translates between Aleph's unified request format and the ChatGPT
/// backend-api format used by chatgpt.com/backend-api.
pub struct ChatGptProtocol {
    client: Client,
}

impl ChatGptProtocol {
    /// Create a new ChatGPT protocol adapter with the given HTTP client
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
        format!("{}{}", base_url, CONVERSATION_ENDPOINT)
    }

    /// Build a ChatGPT conversation request from components
    pub fn build_conversation_request(
        input: &str,
        system_prompt: Option<&str>,
        model: &str,
        conversation_id: Option<&str>,
        parent_message_id: Option<&str>,
    ) -> ChatGptRequest {
        // ChatGPT backend-api does not support a system role,
        // so prepend the system prompt to the user message.
        let full_input = match system_prompt {
            Some(sp) => format!("{}\n\n{}", sp, input),
            None => input.to_string(),
        };

        let message_id = uuid::Uuid::new_v4().to_string();
        let parent_id = parent_message_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        ChatGptRequest {
            action: "next".to_string(),
            messages: vec![ChatGptMessage {
                id: message_id,
                author: Author {
                    role: "user".to_string(),
                },
                content: ChatGptContent {
                    content_type: "text".to_string(),
                    parts: vec![serde_json::Value::String(full_input)],
                },
                metadata: None,
            }],
            model: model.to_string(),
            conversation_id: conversation_id.map(|s| s.to_string()),
            parent_message_id: parent_id,
            timezone_offset_min: 0,
            conversation_mode: ConversationMode {
                kind: "primary_assistant".to_string(),
                plugin_ids: None,
            },
        }
    }

    /// Parse a single SSE line and extract the full text content
    ///
    /// ChatGPT SSE sends cumulative text in each chunk (not deltas),
    /// so the last successfully parsed line contains the complete response.
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            return None;
        }
        let parsed: ChatGptStreamResponse = serde_json::from_str(data).ok()?;
        let message = parsed.message?;
        let parts = &message.content.parts;
        if parts.is_empty() {
            return None;
        }
        parts.last().and_then(|p| p.as_str()).map(|s| s.to_string())
    }
}

#[async_trait]
impl ProtocolAdapter for ChatGptProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let request = Self::build_conversation_request(
            payload.input,
            payload.system_prompt,
            &config.model,
            None,
            None,
        );

        let access_token = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("ChatGPT access token not set — run OAuth login first")
        })?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building ChatGPT request"
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

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "ChatGPT API error");
            if status.as_u16() == 401 {
                return Err(AlephError::provider(
                    "ChatGPT authentication expired — please re-login",
                ));
            }
            if status.as_u16() == 429 {
                return Err(AlephError::provider(
                    "ChatGPT subscription rate limit reached — please try again later",
                ));
            }
            return Err(AlephError::provider(format!(
                "ChatGPT API error ({}): {}",
                status, error_text
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to read ChatGPT response: {}", e)))?;

        // ChatGPT returns SSE even for non-streaming; take the last cumulative text
        let mut result = String::new();
        for line in text.lines() {
            if let Some(content) = Self::parse_sse_line(line) {
                result = content;
            }
        }

        if result.is_empty() {
            Err(AlephError::provider("Empty response from ChatGPT"))
        } else {
            Ok(result)
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
                "ChatGPT API error ({}): {}",
                status, error_text
            )));
        }

        // Track previously seen cumulative text to compute deltas
        let prev_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        let stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .try_filter_map(move |chunk| {
                let prev = prev_text.clone();
                async move {
                    let text = std::str::from_utf8(&chunk)
                        .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                    let mut delta = String::new();
                    for line in text.lines() {
                        if let Some(full_text) = Self::parse_sse_line(line) {
                            let mut prev_guard =
                                prev.lock().unwrap_or_else(|e| e.into_inner());
                            if full_text.len() > prev_guard.len() {
                                let new_part = &full_text[prev_guard.len()..];
                                delta.push_str(new_part);
                            } else if full_text != *prev_guard {
                                delta.push_str(&full_text);
                            }
                            *prev_guard = full_text;
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
    fn test_parse_sse_line_text_content() {
        let line = r#"data: {"message":{"id":"abc","author":{"role":"assistant"},"content":{"content_type":"text","parts":["Hello world"]},"status":"in_progress"},"conversation_id":"conv123","error":null}"#;
        let result = ChatGptProtocol::parse_sse_line(line);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn test_parse_sse_line_done() {
        let result = ChatGptProtocol::parse_sse_line("data: [DONE]");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_sse_line_non_text_content() {
        let line = r#"data: {"message":{"id":"abc","author":{"role":"assistant"},"content":{"content_type":"code","parts":["print('hi')"]},"status":"in_progress"},"conversation_id":"conv123","error":null}"#;
        let result = ChatGptProtocol::parse_sse_line(line);
        assert_eq!(result, Some("print('hi')".to_string()));
    }

    #[test]
    fn test_parse_sse_line_empty_parts() {
        let line = r#"data: {"message":{"id":"abc","author":{"role":"assistant"},"content":{"content_type":"text","parts":[]},"status":"in_progress"},"conversation_id":"conv123","error":null}"#;
        let result = ChatGptProtocol::parse_sse_line(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_build_conversation_request() {
        let request =
            ChatGptProtocol::build_conversation_request("Hello", None, "gpt-4o", None, None);
        assert_eq!(request.action, "next");
        assert_eq!(request.model, "gpt-4o");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].author.role, "user");
        assert!(request.conversation_id.is_none());
    }

    #[test]
    fn test_build_conversation_request_with_system_prompt() {
        let request = ChatGptProtocol::build_conversation_request(
            "Hello",
            Some("You are helpful"),
            "gpt-4o",
            None,
            None,
        );
        let parts = &request.messages[0].content.parts;
        let text = parts[0].as_str().unwrap();
        assert!(text.contains("You are helpful"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn test_adapter_name() {
        let adapter = ChatGptProtocol::new(Client::new());
        assert_eq!(adapter.name(), "chatgpt");
    }

    #[test]
    fn test_create_chatgpt_provider_via_factory() {
        use crate::config::ProviderConfig;
        use crate::providers::create_provider;

        let config = ProviderConfig {
            protocol: Some("chatgpt".to_string()),
            model: "gpt-4o".to_string(),
            api_key: Some("test_token".to_string()),
            base_url: Some("https://chatgpt.com".to_string()),
            enabled: true,
            ..ProviderConfig::test_config("gpt-4o")
        };

        let provider = create_provider("chatgpt-sub", config);
        assert!(
            provider.is_ok(),
            "Should create chatgpt provider: {:?}",
            provider.err()
        );

        let p = provider.unwrap();
        assert_eq!(p.name(), "chatgpt-sub");
    }

    #[test]
    fn test_chatgpt_preset_applied() {
        use crate::providers::presets::get_preset;

        let preset = get_preset("chatgpt");
        assert!(preset.is_some(), "chatgpt preset should exist");

        let p = preset.unwrap();
        assert_eq!(p.protocol, "chatgpt");
        assert_eq!(p.base_url, "https://chatgpt.com");
        assert_eq!(p.default_model, "gpt-4o");
    }
}
