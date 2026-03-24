//! Codex Responses API protocol adapter
//!
//! Handles the Codex backend API format at chatgpt.com/backend-api/codex/responses.
//! Uses the Responses API wire format with typed SSE streaming events.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, ProviderResponse, RequestPayload, StopReason};
use crate::providers::codex::types::ResponsesRequest;
use crate::providers::responses::shared;
use super::codex_utils::extract_codex_account_id;
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;
use tracing::{debug, error};

const CODEX_ENDPOINT: &str = "/backend-api/codex/responses";

/// Codex Responses API protocol adapter
///
/// Translates between Aleph's unified request format and the Codex
/// Responses API format used by chatgpt.com/backend-api/codex/responses.
pub struct CodexProtocol {
    client: Client,
}

impl CodexProtocol {
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

    /// Build a Responses API request from the unified payload
    ///
    /// Parses XML-tagged conversation history from the input string into
    /// native Responses API InputItems. This allows multi-turn tool calling
    /// to work correctly with the Codex API.
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
            store: Some(false),
            reasoning: shared::build_reasoning(payload.think_level),
            tools,
            // Codex mode fields (per pi_agent_rust reference implementation)
            tool_choice,
            parallel_tool_calls: Some(true),
            text: Some(
                serde_json::to_value(crate::providers::codex::types::TextConfig {
                    verbosity: "medium".to_string(),
                })
                .unwrap(),
            ),
            max_output_tokens: payload.max_tokens,
            include: Some(vec!["reasoning.encrypted_content".to_string()]),
        }
    }
}

#[async_trait]
impl ProtocolAdapter for CodexProtocol {
    fn supports_native_tools(&self) -> bool {
        true
    }

    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        _is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let request = Self::build_responses_request(payload, config.default_model());

        let access_token = config.api_key.as_ref().ok_or_else(|| {
            AlephError::invalid_config("Codex access token not set — run OAuth login first")
        })?;

        // Dump the full request JSON for debugging tool calling issues
        if let Ok(json) = serde_json::to_string_pretty(&request) {
            debug!(
                endpoint = %endpoint,
                model = %config.default_model(),
                request_body = %json,
                "Building Codex Responses API request"
            );
        }

        let mut builder = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        // Codex mode headers (per pi_agent_rust reference implementation)
        if let Some(account_id) = extract_codex_account_id(access_token) {
            builder = builder
                .header("chatgpt-account-id", account_id)
                .header("OpenAI-Beta", "responses=experimental")
                .header("originator", "aleph");
        }

        let builder = builder.json(&request);
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
            Err(AlephError::provider("Empty response from Codex"))
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
                "Codex API error ({}): {}",
                status, error_text
            )));
        }

        shared::build_sse_stream(response)
    }

    fn name(&self) -> &'static str {
        "codex"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::codex::types::{InputItem, MessageContent, StreamEvent};
    use crate::providers::responses::shared;

    #[test]
    fn test_build_responses_request_basic() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let request = CodexProtocol::build_responses_request(&payload, "codex-mini-latest");

        assert_eq!(request.model, "codex-mini-latest");
        assert_eq!(request.store, Some(false));
        assert!(request.stream);
        assert!(request.instructions.is_none());
        assert!(request.reasoning.is_none());
        assert_eq!(request.input.len(), 1);
        match &request.input[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content.as_text(), "Hello");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_build_responses_request_with_system_prompt() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_system(Some("You are helpful"));
        let request = CodexProtocol::build_responses_request(&payload, "codex-mini-latest");

        assert_eq!(request.instructions.as_deref(), Some("You are helpful"));
        match &request.input[0] {
            InputItem::Message { content, .. } => {
                assert_eq!(content.as_text(), "Hello");
                assert!(!content.as_text().contains("You are helpful"));
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_build_responses_request_with_reasoning() {
        use crate::agents::thinking::ThinkLevel;
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("Think about this")];
        let payload = RequestPayload::new(&msgs)
            .with_think_level(Some(ThinkLevel::High));
        let request = CodexProtocol::build_responses_request(&payload, "codex-mini-latest");

        let reasoning = request.reasoning.unwrap();
        assert_eq!(reasoning.effort.as_deref(), Some("high"));
        assert_eq!(reasoning.summary.as_deref(), Some("auto"));
    }

    #[test]
    fn test_parse_sse_data_text_delta() {
        let data = r#"{"type":"response.output_text.delta","delta":"Hello","output_index":0,"content_index":0}"#;
        let event = shared::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hello"),
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sse_data_done() {
        let result = shared::parse_sse_data("[DONE]");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_sse_data_completed() {
        let data = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Hello world"}]}]}}"#;
        let event = shared::parse_sse_data(data);
        assert!(event.is_some());
        match event.unwrap() {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                let text = shared::extract_text(&response);
                assert_eq!(text, Some("Hello world".to_string()));
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_extract_text_from_response() {
        use crate::providers::codex::types::{ContentPart, OutputItem, ResponseResource};
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![OutputItem::Message {
                id: "msg_1".to_string(),
                role: "assistant".to_string(),
                content: vec![ContentPart {
                    part_type: "output_text".to_string(),
                    text: "Test output".to_string(),
                }],
            }],
            usage: None,
            error: None,
        };
        assert_eq!(
            shared::extract_text(&response),
            Some("Test output".to_string())
        );
    }

    #[test]
    fn test_extract_text_empty_output() {
        use crate::providers::codex::types::ResponseResource;
        let response = ResponseResource {
            id: "resp_1".to_string(),
            status: "completed".to_string(),
            model: "codex-mini".to_string(),
            output: vec![],
            usage: None,
            error: None,
        };
        assert_eq!(shared::extract_text(&response), None);
    }

    #[test]
    fn test_adapter_name() {
        let adapter = CodexProtocol::new(Client::new());
        assert_eq!(adapter.name(), "codex");
    }

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("codex-mini-latest");
        let endpoint = CodexProtocol::build_endpoint(&config);
        assert!(endpoint.ends_with("/backend-api/codex/responses"));
    }

    #[test]
    fn test_create_provider_via_factory() {
        use crate::config::ProviderConfig;
        use crate::providers::create_provider;

        let mut config = ProviderConfig::test_config("codex-mini-latest");
        config.protocol = Some("codex".to_string());
        config.api_key = Some("test_token".to_string());
        config.base_url = Some("https://chatgpt.com".to_string());
        config.enabled = true;

        let provider = create_provider("chatgpt-sub", config);
        assert!(
            provider.is_ok(),
            "Should create codex provider: {:?}",
            provider.err()
        );
    }

    #[test]
    fn test_codex_preset() {
        use crate::providers::presets::get_preset;

        let preset = get_preset("chatgpt");
        assert!(preset.is_some(), "chatgpt preset should exist");

        let p = preset.unwrap();
        assert_eq!(p.protocol, "codex");
        assert_eq!(p.default_model, "gpt-5.4");
    }

    // ─── convert_messages tests ─────────────────────────────────────

    #[test]
    fn test_convert_s1_pure_text_user_message() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::user("hello")];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: MessageContent::Text { content: "hello".into() },
            }
        );
    }

    #[test]
    fn test_convert_s2_multi_turn_conversation() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems programming language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 3);
        match &items[0] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content.as_text(), "What is Rust?");
            }
            other => panic!("Expected Message, got {:?}", other),
        }
        match &items[1] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "assistant");
                assert_eq!(content.as_text(), "Rust is a systems programming language.");
            }
            other => panic!("Expected Message, got {:?}", other),
        }
        match &items[2] {
            InputItem::Message { role, content } => {
                assert_eq!(role, "user");
                assert_eq!(content.as_text(), "Tell me more.");
            }
            other => panic!("Expected Message, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s3_assistant_text_and_tool_call() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "Let me search for that.".to_string(),
                },
                ContentBlock::ToolCall {
                    id: "call_abc".to_string(),
                    name: "web_search".to_string(),
                    arguments: serde_json::json!({"query": "rust lang"}),
                },
            ],
        }];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "assistant".to_string(),
                content: MessageContent::Text { content: "Let me search for that.".into() },
            }
        );
        match &items[1] {
            InputItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                assert_eq!(call_id, "call_abc");
                assert_eq!(name, "web_search");
                let parsed: serde_json::Value = serde_json::from_str(arguments).unwrap();
                assert_eq!(parsed, serde_json::json!({"query": "rust lang"}));
            }
            other => panic!("Expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s4_tool_result() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::tool_result(
            "call_123",
            "search",
            "Found 5 results",
            false,
        )];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0],
            InputItem::FunctionCallOutput {
                call_id: "call_123".to_string(),
                output: "Found 5 results".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_s5_full_tool_use_cycle() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        let msgs = [
            UnifiedMessage::user("Search for Rust tutorials"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "rust tutorials"}),
                }],
            },
            UnifiedMessage::tool_result("call_1", "search", "Tutorial list: ...", false),
            UnifiedMessage::assistant("Here are some Rust tutorials I found."),
        ];
        let items = shared::convert_messages(&msgs);

        // User(1) + FunctionCall(1) + FunctionCallOutput(1) + Assistant Message(1) = 4
        // Note: Assistant with only ToolCall has no text, so no Message emitted for it
        assert_eq!(items.len(), 4);
        assert_eq!(
            items[0],
            InputItem::Message {
                role: "user".to_string(),
                content: MessageContent::Text { content: "Search for Rust tutorials".into() },
            }
        );
        match &items[1] {
            InputItem::FunctionCall { call_id, name, .. } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(name, "search");
            }
            other => panic!("Expected FunctionCall, got {:?}", other),
        }
        assert_eq!(
            items[2],
            InputItem::FunctionCallOutput {
                call_id: "call_1".to_string(),
                output: "Tutorial list: ...".to_string(),
            }
        );
        assert_eq!(
            items[3],
            InputItem::Message {
                role: "assistant".to_string(),
                content: MessageContent::Text { content: "Here are some Rust tutorials I found.".into() },
            }
        );
    }

    #[test]
    fn test_convert_s6_multiple_tool_calls_one_turn() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                ContentBlock::Text {
                    text: "Running multiple searches.".to_string(),
                },
                ContentBlock::ToolCall {
                    id: "c1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "a"}),
                },
                ContentBlock::ToolCall {
                    id: "c2".to_string(),
                    name: "fetch".to_string(),
                    arguments: serde_json::json!({"url": "http://example.com"}),
                },
                ContentBlock::ToolCall {
                    id: "c3".to_string(),
                    name: "calc".to_string(),
                    arguments: serde_json::json!({"expr": "1+1"}),
                },
            ],
        }];
        let items = shared::convert_messages(&msgs);

        // 1 Message (text) + 3 FunctionCalls = 4
        assert_eq!(items.len(), 4);
        assert!(matches!(&items[0], InputItem::Message { role, .. } if role == "assistant"));
        assert!(matches!(&items[1], InputItem::FunctionCall { call_id, .. } if call_id == "c1"));
        assert!(matches!(&items[2], InputItem::FunctionCall { call_id, .. } if call_id == "c2"));
        assert!(matches!(&items[3], InputItem::FunctionCall { call_id, .. } if call_id == "c3"));
    }

    #[test]
    fn test_convert_s7_error_tool_result() {
        use crate::providers::message::UnifiedMessage;
        let msgs = [UnifiedMessage::tool_result(
            "call_err",
            "dangerous_tool",
            "Permission denied",
            true, // is_error = true
        )];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        // convert_messages does not distinguish is_error — it just passes content through
        assert_eq!(
            items[0],
            InputItem::FunctionCallOutput {
                call_id: "call_err".to_string(),
                output: "Permission denied".to_string(),
            }
        );
    }

    #[test]
    fn test_convert_s8_json_tool_output() {
        use crate::providers::message::UnifiedMessage;
        let json_val = serde_json::json!({"results": [1, 2, 3], "total": 3});
        let msgs = [UnifiedMessage::tool_result_json(
            "call_json",
            "api_call",
            json_val.clone(),
            false,
        )];
        let items = shared::convert_messages(&msgs);

        assert_eq!(items.len(), 1);
        match &items[0] {
            InputItem::FunctionCallOutput { call_id, output } => {
                assert_eq!(call_id, "call_json");
                let parsed: serde_json::Value = serde_json::from_str(output).unwrap();
                assert_eq!(parsed, json_val);
            }
            other => panic!("Expected FunctionCallOutput, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s9_completed_event_usage_extraction() {
        // Test that parse_sse_data + extract on a Completed event with usage works
        let data = r#"{"type":"response.completed","response":{"id":"resp_u","status":"completed","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"done"}]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}}"#;
        let event = shared::parse_sse_data(data).unwrap();
        match event {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "completed");
                assert_eq!(
                    shared::extract_text(&response),
                    Some("done".to_string())
                );
                let usage = response.usage.as_ref().expect("usage should be present");
                assert_eq!(usage.input_tokens, 10);
                assert_eq!(usage.output_tokens, 5);
                assert_eq!(usage.total_tokens, 15);
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_convert_s10_incomplete_status() {
        // Test that an "incomplete" status is correctly deserialized from a Completed event
        // (the protocol maps incomplete → MaxTokens in parse_response, but we verify
        // the status field is correctly captured here)
        let data = r#"{"type":"response.completed","response":{"id":"resp_inc","status":"incomplete","model":"codex-mini","output":[{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"partial"}]}]}}"#;
        let event = shared::parse_sse_data(data).unwrap();
        match event {
            StreamEvent::Completed { response } => {
                assert_eq!(response.status, "incomplete");
                assert_eq!(
                    shared::extract_text(&response),
                    Some("partial".to_string())
                );
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
    }
}
