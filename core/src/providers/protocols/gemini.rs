//! Google Gemini protocol adapter
//!
//! Handles Google Generative AI API format.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    DiscoveredModel, NativeToolCall, ProtocolAdapter, ProviderResponse, RequestPayload, StopReason,
};
use crate::providers::gemini::{
    Content, GeminiFunctionDeclaration, GeminiToolConfig, GenerateContentRequest,
    GenerateContentResponse, GenerationConfig, Part, ThinkingConfig,
};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::Client;
use tracing::{debug, error};

/// Google Gemini protocol adapter
pub struct GeminiProtocol {
    client: Client,
}

impl GeminiProtocol {
    /// Create a new Gemini protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL
    fn build_endpoint(config: &ProviderConfig, is_streaming: bool) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        // Normalize URL
        let base_url = raw_base_url.trim_end_matches('/').to_string();

        // Build endpoint based on streaming mode
        if is_streaming {
            format!(
                "{}/v1beta/models/{}:streamGenerateContent",
                base_url, config.model
            )
        } else {
            format!(
                "{}/v1beta/models/{}:generateContent",
                base_url, config.model
            )
        }
    }

    /// Build contents array from payload
    fn build_contents(payload: &RequestPayload) -> Vec<Content> {
        // Check for image content
        let has_image = payload.image.is_some();
        let has_image_attachments = payload
            .attachments
            .map(|a| a.iter().any(|att| att.media_type == "image"))
            .unwrap_or(false);

        if has_image || has_image_attachments {
            Self::build_multimodal_contents(payload)
        } else {
            Self::build_text_contents(payload)
        }
    }

    /// Build text-only contents
    fn build_text_contents(payload: &RequestPayload) -> Vec<Content> {
        let input = if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                combine_with_document_context(&doc_context, payload.input)
            } else {
                payload.input.to_string()
            }
        } else {
            payload.input.to_string()
        };

        vec![Content {
            role: Some("user".to_string()),
            parts: vec![Part::Text { text: input }],
        }]
    }

    /// Build multimodal contents with images
    fn build_multimodal_contents(payload: &RequestPayload) -> Vec<Content> {
        let mut parts = Vec::new();

        // Handle document attachments
        let text_input = if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                combine_with_document_context(&doc_context, payload.input)
            } else {
                payload.input.to_string()
            }
        } else {
            payload.input.to_string()
        };

        // Add text content
        let text = if text_input.is_empty() {
            "Describe this image in detail.".to_string()
        } else {
            text_input
        };
        parts.push(Part::Text { text });

        // Add legacy image
        if let Some(image) = payload.image {
            let mime_type = match image.format {
                crate::clipboard::ImageFormat::Png => "image/png",
                crate::clipboard::ImageFormat::Jpeg => "image/jpeg",
                crate::clipboard::ImageFormat::Gif => "image/gif",
            };
            let base64_data = {
                use base64::{engine::general_purpose, Engine as _};
                general_purpose::STANDARD.encode(&image.data)
            };
            parts.push(Part::InlineData {
                inline_data: crate::providers::gemini::InlineData {
                    mime_type: mime_type.to_string(),
                    data: base64_data,
                },
            });
        }

        // Add image attachments
        if let Some(attachments) = payload.attachments {
            let (images, _) = separate_attachments(attachments);
            for attachment in images {
                parts.push(Part::InlineData {
                    inline_data: crate::providers::gemini::InlineData {
                        mime_type: attachment.mime_type.clone(),
                        data: attachment.data.clone(),
                    },
                });
            }
        }

        vec![Content {
            role: Some("user".to_string()),
            parts,
        }]
    }

    /// Build system instruction from system prompt
    fn build_system_instruction(system_prompt: Option<&str>) -> Option<Content> {
        system_prompt.map(|prompt| Content {
            role: None, // system instruction doesn't have a role
            parts: vec![Part::Text {
                text: prompt.to_string(),
            }],
        })
    }

    /// Map ThinkLevel to thinkingBudget
    fn map_think_level(level: &ThinkLevel) -> Option<u32> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(500),
            ThinkLevel::Low => Some(1000),
            ThinkLevel::Medium => Some(2000),
            ThinkLevel::High => Some(4000),
            ThinkLevel::XHigh => Some(8000),
        }
    }

    /// Parse SSE line for streaming
    fn parse_sse_line(line: &str) -> Option<String> {
        // Gemini SSE format: supports both "data: {json}" and plain JSON lines
        if line.is_empty() || line.starts_with(':') {
            return None;
        }

        // Extract JSON data (handle both "data: " prefix and plain JSON)
        let json_str = line.strip_prefix("data: ").unwrap_or(line);

        // Skip [DONE] marker
        if json_str == "[DONE]" {
            return None;
        }

        let parsed: serde_json::Value = serde_json::from_str(json_str).ok()?;

        // Extract text from candidates[0].content.parts[0].text
        parsed
            .get("candidates")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.get(0))
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
    }
}

#[async_trait]
impl ProtocolAdapter for GeminiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config, is_streaming);
        let contents = Self::build_contents(payload);
        let system_instruction = Self::build_system_instruction(payload.system_prompt);

        // Build generation config
        let thinking_config = payload
            .think_level
            .as_ref()
            .and_then(Self::map_think_level)
            .map(|budget| ThinkingConfig {
                thinking_budget: Some(budget),
            });

        // Per-request overrides provider config
        let generation_config = GenerationConfig {
            max_output_tokens: payload.max_tokens.or(config.max_tokens).or(Some(DEFAULT_MAX_TOKENS)),
            temperature: payload.temperature.or(config.temperature),
            top_p: config.top_p,
            top_k: None,
            thinking_config,
        };

        // Build tool declarations if provided
        let tools = payload.tools.map(|tool_defs| {
            let declarations: Vec<GeminiFunctionDeclaration> = tool_defs
                .iter()
                .map(|td| {
                    // Ensure parameters has "type" field — required by strict
                    // backends; keeps parity with Anthropic/OpenAI adapters.
                    let mut params = td.parameters.clone();
                    if let Some(obj) = params.as_object_mut() {
                        obj.entry("type")
                            .or_insert_with(|| serde_json::json!("object"));
                    }
                    GeminiFunctionDeclaration {
                        name: td.name.clone(),
                        description: td.description.clone(),
                        parameters: params,
                    }
                })
                .collect();
            vec![GeminiToolConfig {
                function_declarations: declarations,
            }]
        });

        let request_body = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config: Some(generation_config),
            tools,
        };

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building Gemini request"
        );

        // Build URL with query parameters
        let mut url = endpoint;
        url.push_str("?key=");
        url.push_str(api_key);

        // Add alt=sse for streaming
        if is_streaming {
            url.push_str("&alt=sse");
        }

        Ok(self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<ProviderResponse> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "Gemini API error");

            // Try to parse Gemini error response
            if let Ok(error_response) = serde_json::from_str::<GenerateContentResponse>(&error_text)
            {
                if let Some(err) = error_response.error {
                    let msg = err.message;
                    return match status.as_u16() {
                        401 | 403 => Err(AlephError::authentication("Gemini", &msg)),
                        429 => Err(AlephError::rate_limit(format!("Gemini: {}", msg))),
                        _ => Err(AlephError::provider(format!("Gemini error: {}", msg))),
                    };
                }
            }

            return Err(AlephError::provider(format!(
                "Gemini error ({}): {}",
                status, error_text
            )));
        }

        let response_body: GenerateContentResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse Gemini response");
            AlephError::provider(format!("Failed to parse response: {}", e))
        })?;

        // Check for error in response
        if let Some(err) = response_body.error {
            return Err(AlephError::provider(format!(
                "Gemini error: {}",
                err.message
            )));
        }

        let candidates = response_body
            .candidates
            .ok_or_else(|| AlephError::provider("No candidates in response"))?;
        let candidate = candidates
            .first()
            .ok_or_else(|| AlephError::provider("No candidates in response"))?;

        let mut provider_response = ProviderResponse::default();

        // Iterate all parts: collect text and functionCall entries
        let mut text_parts = Vec::new();
        for (index, part) in candidate.content.parts.iter().enumerate() {
            if let Some(ref text) = part.text {
                text_parts.push(text.clone());
            }
            if let Some(ref fc) = part.function_call {
                provider_response.tool_calls.push(NativeToolCall {
                    // Gemini does not assign tool call IDs; generate synthetic ones
                    id: format!("gemini-fc-{}", index),
                    name: fc.name.clone(),
                    arguments: fc.args.clone(),
                });
            }
        }

        if !text_parts.is_empty() {
            provider_response.text = Some(text_parts.join(""));
        }

        // Map Gemini finish reason to StopReason
        provider_response.stop_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => StopReason::EndTurn,
            Some("FUNCTION_CALL") => StopReason::ToolUse,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            _ => StopReason::Unknown,
        };

        Ok(provider_response)
    }

    fn supports_native_tools(&self) -> bool {
        true
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Gemini streaming error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map(move |chunk| {
                let bytes = chunk.map_err(|e| AlephError::network(e.to_string()))?;
                let text = String::from_utf8_lossy(&bytes);

                let mut result = String::new();
                for line in text.lines() {
                    if let Some(content) = Self::parse_sse_line(line) {
                        result.push_str(&content);
                    }
                }

                Ok(result)
            })
            .filter(|r| {
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                std::future::ready(keep)
            })
            .boxed();

        Ok(stream)
    }

    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn list_models(&self, config: &ProviderConfig) -> Result<Option<Vec<DiscoveredModel>>> {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        let api_key = config.api_key.as_deref().unwrap_or("");
        let url = format!("{}/v1beta/models?key={}", base_url, api_key);

        let response = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Gemini model list request failed: {}", e)))?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AlephError::network(format!("Failed to parse Gemini model list: {}", e)))?;

        let models = parse_gemini_models_response(&body)?;

        Ok(Some(models))
    }
}

/// Parse Gemini /v1beta/models JSON response into DiscoveredModel list
pub(crate) fn parse_gemini_models_response(
    body: &serde_json::Value,
) -> crate::error::Result<Vec<DiscoveredModel>> {
    let models = body["models"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    // Gemini returns "models/gemini-1.5-pro" format
                    let full_name = m["name"].as_str()?;
                    let id = full_name.strip_prefix("models/").unwrap_or(full_name);
                    let display_name = m["displayName"].as_str().map(|s| s.to_string());
                    Some(DiscoveredModel {
                        id: id.to_string(),
                        name: display_name,
                        owned_by: Some("google".to_string()),
                        capabilities: vec!["chat".to_string()],
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_endpoint_non_streaming() {
        let config = ProviderConfig::test_config("gemini-pro");
        let endpoint = GeminiProtocol::build_endpoint(&config, false);
        assert_eq!(
            endpoint,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:generateContent"
        );
    }

    #[test]
    fn test_build_endpoint_streaming() {
        let config = ProviderConfig::test_config("gemini-pro");
        let endpoint = GeminiProtocol::build_endpoint(&config, true);
        assert_eq!(
            endpoint,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-pro:streamGenerateContent"
        );
    }

    #[test]
    fn test_build_endpoint_custom_base_url() {
        let mut config = ProviderConfig::test_config("gemini-pro");
        config.base_url = Some("https://custom.api.com".to_string());
        let endpoint = GeminiProtocol::build_endpoint(&config, false);
        assert_eq!(
            endpoint,
            "https://custom.api.com/v1beta/models/gemini-pro:generateContent"
        );
    }

    #[test]
    fn test_map_think_level() {
        assert_eq!(GeminiProtocol::map_think_level(&ThinkLevel::Off), None);
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::Minimal),
            Some(500)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::Low),
            Some(1000)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::Medium),
            Some(2000)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::High),
            Some(4000)
        );
        assert_eq!(
            GeminiProtocol::map_think_level(&ThinkLevel::XHigh),
            Some(8000)
        );
    }

    #[test]
    fn test_build_text_contents() {
        let payload = RequestPayload::new("Hello, Gemini!");
        let contents = GeminiProtocol::build_contents(&payload);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, Some("user".to_string()));
        assert_eq!(contents[0].parts.len(), 1);

        if let Part::Text { text } = &contents[0].parts[0] {
            assert_eq!(text, "Hello, Gemini!");
        } else {
            panic!("Expected text part");
        }
    }

    #[test]
    fn test_build_system_instruction() {
        let instruction = GeminiProtocol::build_system_instruction(Some("You are helpful"));

        assert!(instruction.is_some());
        let content = instruction.unwrap();
        assert_eq!(content.role, None);
        assert_eq!(content.parts.len(), 1);

        if let Part::Text { text } = &content.parts[0] {
            assert_eq!(text, "You are helpful");
        } else {
            panic!("Expected text part");
        }
    }

    #[test]
    fn test_parse_sse_line() {
        // Valid SSE line with "data:" prefix
        let line_with_prefix = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        assert_eq!(
            GeminiProtocol::parse_sse_line(line_with_prefix),
            Some("Hello".to_string())
        );

        // Valid SSE line without prefix (plain JSON)
        let line_plain = r#"{"candidates":[{"content":{"parts":[{"text":"World"}]}}]}"#;
        assert_eq!(
            GeminiProtocol::parse_sse_line(line_plain),
            Some("World".to_string())
        );

        // [DONE] marker with prefix
        let done_with_prefix = "data: [DONE]";
        assert!(GeminiProtocol::parse_sse_line(done_with_prefix).is_none());

        // Empty line
        assert!(GeminiProtocol::parse_sse_line("").is_none());

        // Comment line
        assert!(GeminiProtocol::parse_sse_line(": keep-alive").is_none());
    }

    #[test]
    fn test_parse_response_error() {
        // Test error parsing logic
        let error_json = r#"{
            "error": {
                "code": 400,
                "message": "Invalid request",
                "status": "INVALID_ARGUMENT"
            }
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(error_json).unwrap();
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, 400);
    }

    #[test]
    fn test_parse_response_success() {
        // Test successful response parsing
        let success_json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "This is a test response"}]
                },
                "finishReason": "STOP"
            }]
        }"#;

        let response: GenerateContentResponse = serde_json::from_str(success_json).unwrap();
        assert!(response.candidates.is_some());

        let candidates = response.candidates.unwrap();
        let text = candidates[0].content.parts[0].text.as_deref();
        assert_eq!(text, Some("This is a test response"));
        assert_eq!(candidates[0].finish_reason.as_deref(), Some("STOP"));
    }

    #[test]
    fn test_build_request_basic() {
        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let payload = RequestPayload::new("Hello");

        let request = protocol
            .build_request(&payload, &config, false)
            .expect("Failed to build request");

        let url = request.build().unwrap().url().to_string();
        assert!(url.contains("generateContent"));
        assert!(url.contains("key=test-api-key"));
        assert!(!url.contains("alt=sse"));
    }

    #[test]
    fn test_build_request_streaming() {
        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let payload = RequestPayload::new("Hello");

        let request = protocol
            .build_request(&payload, &config, true)
            .expect("Failed to build request");

        let url = request.build().unwrap().url().to_string();
        assert!(url.contains("streamGenerateContent"));
        assert!(url.contains("key=test-api-key"));
        assert!(url.contains("alt=sse"));
    }

    #[test]
    fn test_build_request_with_thinking() {
        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let payload = RequestPayload::new("Solve this problem")
            .with_think_level(Some(ThinkLevel::Medium));

        let request = protocol
            .build_request(&payload, &config, false)
            .expect("Failed to build request");

        // We can't easily inspect the request body, but we can verify it builds successfully
        assert!(request.build().is_ok());
    }

    #[test]
    fn test_supports_native_tools() {
        let protocol = GeminiProtocol::new(Client::new());
        assert!(protocol.supports_native_tools());
    }

    #[test]
    fn test_build_request_with_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::ToolCategory;

        let client = Client::new();
        let protocol = GeminiProtocol::new(client);

        let mut config = ProviderConfig::test_config("gemini-pro");
        config.api_key = Some("test-api-key".to_string());

        let tools = vec![ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
            ToolCategory::Builtin,
        )];

        let payload = RequestPayload::new("Search for Rust").with_tools(Some(&tools));

        let request = protocol
            .build_request(&payload, &config, false)
            .expect("Failed to build request");

        assert!(request.build().is_ok());
    }

    /// Helper: simulate parse_response logic on a deserialized GenerateContentResponse
    /// (avoids needing to construct a real reqwest::Response in unit tests)
    fn extract_provider_response(
        response_body: GenerateContentResponse,
    ) -> Result<ProviderResponse> {
        if let Some(err) = response_body.error {
            return Err(AlephError::provider(format!(
                "Gemini error: {}",
                err.message
            )));
        }

        let candidates = response_body
            .candidates
            .ok_or_else(|| AlephError::provider("No candidates in response"))?;
        let candidate = candidates
            .first()
            .ok_or_else(|| AlephError::provider("No candidates in response"))?;

        let mut provider_response = ProviderResponse::default();

        let mut text_parts = Vec::new();
        for (index, part) in candidate.content.parts.iter().enumerate() {
            if let Some(ref text) = part.text {
                text_parts.push(text.clone());
            }
            if let Some(ref fc) = part.function_call {
                provider_response.tool_calls.push(NativeToolCall {
                    id: format!("gemini-fc-{}", index),
                    name: fc.name.clone(),
                    arguments: fc.args.clone(),
                });
            }
        }

        if !text_parts.is_empty() {
            provider_response.text = Some(text_parts.join(""));
        }

        provider_response.stop_reason = match candidate.finish_reason.as_deref() {
            Some("STOP") => StopReason::EndTurn,
            Some("FUNCTION_CALL") => StopReason::ToolUse,
            Some("MAX_TOKENS") => StopReason::MaxTokens,
            _ => StopReason::Unknown,
        };

        Ok(provider_response)
    }

    #[test]
    fn test_extract_response_text_only() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello world"}]
                },
                "finishReason": "STOP"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Hello world"));
        assert!(!result.has_tool_calls());
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_extract_response_with_function_call() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{
                        "functionCall": {
                            "name": "search",
                            "args": {"query": "Rust programming"}
                        }
                    }]
                },
                "finishReason": "FUNCTION_CALL"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert!(result.text.is_none());
        assert!(result.has_tool_calls());
        assert_eq!(result.tool_calls.len(), 1);
        assert_eq!(result.tool_calls[0].name, "search");
        assert_eq!(result.tool_calls[0].arguments["query"], "Rust programming");
        assert!(result.tool_calls[0].id.starts_with("gemini-fc-"));
        assert_eq!(result.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_extract_response_with_text_and_function_call() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [
                        {"text": "Let me search for that."},
                        {"functionCall": {"name": "web_search", "args": {"q": "test"}}}
                    ]
                },
                "finishReason": "FUNCTION_CALL"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Let me search for that."));
        assert!(result.has_tool_calls());
        assert_eq!(result.tool_calls[0].name, "web_search");
        assert_eq!(result.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn test_extract_response_max_tokens() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Truncated output..."}]
                },
                "finishReason": "MAX_TOKENS"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Truncated output..."));
        assert_eq!(result.stop_reason, StopReason::MaxTokens);
    }

    #[test]
    fn test_extract_response_unknown_finish_reason() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Some text"}]
                },
                "finishReason": "SAFETY"
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.stop_reason, StopReason::Unknown);
    }

    #[test]
    fn test_extract_response_no_finish_reason() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "Partial response"}]
                }
            }]
        }"#;

        let response_body: GenerateContentResponse = serde_json::from_str(json).unwrap();
        let result = extract_provider_response(response_body).unwrap();

        assert_eq!(result.text.as_deref(), Some("Partial response"));
        assert_eq!(result.stop_reason, StopReason::Unknown);
    }

    #[test]
    fn parse_gemini_models_success() {
        let body = serde_json::json!({
            "models": [
                {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro"},
                {"name": "models/gemini-2.5-flash", "displayName": "Gemini 2.5 Flash"}
            ]
        });
        let models = parse_gemini_models_response(&body).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-2.5-pro");
        assert_eq!(models[0].name, Some("Gemini 2.5 Pro".to_string()));
        assert_eq!(models[0].owned_by, Some("google".to_string()));
        assert_eq!(models[0].capabilities, vec!["chat".to_string()]);
    }

    #[test]
    fn parse_gemini_models_empty() {
        let body = serde_json::json!({"models": []});
        let models = parse_gemini_models_response(&body).unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn parse_gemini_models_malformed() {
        let body = serde_json::json!({"invalid": true});
        let models = parse_gemini_models_response(&body).unwrap();
        assert!(models.is_empty());
    }
}
