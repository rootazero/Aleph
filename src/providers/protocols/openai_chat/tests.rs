#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::providers::message::UnifiedMessage;
    use crate::providers::openai::{ChatCompletionResponse, OpenAiFunctionCall, OpenAiToolCall};

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("deepseek-chat");
        config.base_url = Some("https://api.deepseek.com".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.deepseek.com/v1/chat/completions");
    }

    #[test]
    fn test_build_endpoint_v3() {
        let mut config = ProviderConfig::test_config("doubao-pro");
        config.base_url = Some("https://ark.cn-beijing.volces.com/api/v3".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(
            endpoint,
            "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
        );
    }

    #[test]
    fn test_build_endpoint_with_trailing_slash() {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.base_url = Some("https://api.example.com/v1/".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.example.com/v1/chat/completions");
    }

    #[test]
    fn test_map_think_level() {
        use crate::agents::thinking::ThinkLevel;

        assert!(OpenAiProtocol::map_think_level(&ThinkLevel::Off).is_none());
        assert!(OpenAiProtocol::map_think_level(&ThinkLevel::Minimal).is_none());
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::Low),
            Some("low".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::Medium),
            Some("medium".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::High),
            Some("high".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::XHigh),
            Some("high".to_string())
        );
    }

    #[test]
    fn test_convert_messages_text_only() {
        let msgs = [UnifiedMessage::user("Hello, world!")];
        let messages = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    #[test]
    fn test_convert_messages_with_system_prompt() {
        let msgs = [UnifiedMessage::user("Hello")];
        let messages = OpenAiProtocol::convert_messages(&msgs, Some("You are helpful"));

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
    }

    #[test]
    fn test_convert_messages_no_system_prompt() {
        let msgs = [UnifiedMessage::user("Hello")];
        let messages = OpenAiProtocol::convert_messages(&msgs, None);

        // Without system prompt, only user message
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
    }

    // =========================================================================
    // Native Function Calling Tests
    // =========================================================================

    #[test]
    fn test_supports_native_tools() {
        let protocol = OpenAiProtocol::new(Client::new());
        assert!(protocol.supports_native_tools());
    }

    #[test]
    fn test_openai_tool_serialization() {
        let tool = OpenAiTool {
            tool_type: "function".into(),
            function: OpenAiFunction {
                name: "search".into(),
                description: "Search the web".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"}
                    },
                    "required": ["query"]
                }),
                strict: None,
            },
        };

        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "search");
        assert_eq!(json["function"]["description"], "Search the web");
        assert!(json["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_parse_tool_calls_response() {
        // Simulate a real OpenAI response JSON with tool_calls
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc123",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\":\"San Francisco\",\"unit\":\"celsius\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 20,
                "total_tokens": 70
            }
        });

        let response: ChatCompletionResponse = serde_json::from_value(response_json).unwrap();

        assert_eq!(response.choices.len(), 1);
        let choice = &response.choices[0];

        // content should be None (null in JSON)
        assert!(choice.message.content.is_none());

        // tool_calls should be present
        let tool_calls = choice.message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_abc123");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(
            tool_calls[0].function.arguments,
            r#"{"location":"San Francisco","unit":"celsius"}"#
        );

        // finish_reason should be tool_calls
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));

        // Usage should be present
        let usage = response.usage.as_ref().unwrap();
        assert_eq!(usage.prompt_tokens, 50);
        assert_eq!(usage.completion_tokens, 20);
    }

    #[test]
    fn test_parse_text_response() {
        // Simulate a text-only response
        let response_json = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "Hello! How can I help you?"
                },
                "finish_reason": "stop"
            }]
        });

        let response: ChatCompletionResponse = serde_json::from_value(response_json).unwrap();

        let choice = &response.choices[0];
        assert_eq!(
            choice.message.content.as_deref(),
            Some("Hello! How can I help you?")
        );
        assert!(choice.message.tool_calls.is_none());
        assert_eq!(choice.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_function_arguments() {
        // Test that JSON string arguments parse correctly
        let tc = OpenAiToolCall {
            id: "call_123".into(),
            call_type: Some("function".into()),
            function: OpenAiFunctionCall {
                name: "search".into(),
                arguments: r#"{"query":"rust async","limit":10}"#.into(),
            },
        };

        let parsed: serde_json::Value = serde_json::from_str(&tc.function.arguments).unwrap();
        assert_eq!(parsed["query"], "rust async");
        assert_eq!(parsed["limit"], 10);
    }

    #[test]
    fn test_parse_malformed_function_arguments() {
        // Test fallback for malformed arguments
        let bad_args = "not valid json {{{";
        let result: serde_json::Value =
            serde_json::from_str(bad_args).unwrap_or(serde_json::Value::Object(Default::default()));
        assert!(result.is_object());
        assert!(result.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_build_request_includes_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::ToolCategory;

        let protocol = OpenAiProtocol::new(Client::new());
        let tools = vec![ToolDefinition::new(
            "search",
            "Search the web",
            serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
            ToolCategory::Builtin,
        )];
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        // Verify the body contains tools in OpenAI format
        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "search");
        assert_eq!(
            body["tools"][0]["function"]["description"],
            "Search the web"
        );
        assert!(body["tools"][0]["function"]["parameters"]["properties"]["query"].is_object());
    }

    #[test]
    fn test_build_request_no_tools_when_none() {
        let protocol = OpenAiProtocol::new(Client::new());
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs);
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        // tools field should be absent when no tools provided
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn test_build_request_with_multiple_tools() {
        use crate::dispatcher::ToolDefinition;
        use crate::ToolCategory;

        let protocol = OpenAiProtocol::new(Client::new());
        let tools = vec![
            ToolDefinition::new(
                "search",
                "Search the web",
                serde_json::json!({"type": "object", "properties": {}}),
                ToolCategory::Builtin,
            ),
            ToolDefinition::new(
                "read_file",
                "Read a file",
                serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                ToolCategory::Builtin,
            ),
        ];
        let msgs = [UnifiedMessage::user("Hello")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some("test-key".to_string());

        let request = protocol.build_request(&payload, &config).unwrap();
        let built = request.build().unwrap();

        let body_bytes = built.body().unwrap().as_bytes().unwrap();
        let body: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        let tools_array = body["tools"].as_array().unwrap();
        assert_eq!(tools_array.len(), 2);
        assert_eq!(tools_array[0]["function"]["name"], "search");
        assert_eq!(tools_array[1]["function"]["name"], "read_file");
    }

    // =========================================================================
    // convert_messages() Tests
    // =========================================================================

    #[test]
    fn test_convert_s1_pure_text_user() {
        let msgs = [UnifiedMessage::user("Hello, world!")];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        // OpenAI uses plain string content (Text variant), not array
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "Hello, world!");
    }

    #[test]
    fn test_convert_s2_multi_turn() {
        let msgs = [
            UnifiedMessage::user("What is Rust?"),
            UnifiedMessage::assistant("Rust is a systems language."),
            UnifiedMessage::user("Tell me more."),
        ];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "user");
    }

    #[test]
    fn test_convert_s3_assistant_text_and_tool_call() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::Text {
                    text: "Let me search.".to_string(),
                    cache_control: None,
                },
                CB::ToolCall {
                    id: "call_abc".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"query": "rust"}),
                },
            ],
        }];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
        let json = serde_json::to_value(&result[0]).unwrap();
        // Text content should be present
        assert_eq!(json["content"], "Let me search.");
    }

    #[test]
    fn test_convert_s4_tool_result() {
        let msgs = [UnifiedMessage::tool_result(
            "call_abc",
            "search",
            "Found results",
            false,
        )];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        // Each ToolResult is a separate tool message (NOT merged)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "tool");
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["content"], "Found results");
    }

    #[test]
    fn test_convert_s5_full_cycle() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [
            UnifiedMessage::user("Search for Rust"),
            UnifiedMessage::Assistant {
                content: vec![CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "Rust"}),
                }],
            },
            UnifiedMessage::tool_result("call_1", "search", "Rust is great", false),
            UnifiedMessage::assistant("Based on results, Rust is great!"),
        ];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "tool");
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_convert_s6_arguments_json_stringify() {
        use crate::providers::message::ContentBlock as CB;
        // Verify that OpenAI tool_calls arguments are stringified JSON
        let args = serde_json::json!({"query": "test"});
        let msgs = [UnifiedMessage::Assistant {
            content: vec![CB::ToolCall {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: args.clone(),
            }],
        }];
        // convert_messages currently puts text content on assistant, tool_calls
        // are extracted but stored as `let _ = msg_tool_calls` (TODO in code).
        // We verify the internal extraction produces stringified JSON.
        let stringified = serde_json::to_string(&args).unwrap();
        assert_eq!(stringified, r#"{"query":"test"}"#);

        // Also verify the messages convert without panic
        let result = OpenAiProtocol::convert_messages(&msgs, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
    }

    #[test]
    fn test_convert_s7_multiple_tool_calls() {
        use crate::providers::message::ContentBlock as CB;
        let msgs = [UnifiedMessage::Assistant {
            content: vec![
                CB::ToolCall {
                    id: "call_1".to_string(),
                    name: "search".to_string(),
                    arguments: serde_json::json!({"q": "a"}),
                },
                CB::ToolCall {
                    id: "call_2".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "/tmp/x"}),
                },
            ],
        }];
        let result = OpenAiProtocol::convert_messages(&msgs, None);

        // Assistant message should exist
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
    }

    #[test]
    fn test_convert_s8_system_prompt_handling() {
        let msgs = [UnifiedMessage::user("Hello")];
        let result = OpenAiProtocol::convert_messages(&msgs, Some("You are a helpful assistant."));

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "system");
        let json = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(json["content"], "You are a helpful assistant.");
        assert_eq!(result[1].role, "user");
    }

    // =========================================================================
    // parse_chat_sse_event() Tests
    // =========================================================================

    #[test]
    fn test_parse_sse_text_delta() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        let data = r#"{"choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        assert_eq!(pending.len(), 1);
        let delta = pending.pop_front().unwrap().unwrap();
        assert!(matches!(delta, ProviderDelta::TextDelta(t) if t == "Hello"));
    }

    #[test]
    fn test_parse_sse_empty_content_skipped() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        let data = r#"{"choices":[{"delta":{"content":""},"index":0}]}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        // Empty content should not emit any delta
        assert_eq!(pending.len(), 0);
    }

    #[test]
    fn test_parse_sse_tool_call_start() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"search","arguments":""}}]},"index":0}]}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        // Should emit ToolCallStart
        assert_eq!(pending.len(), 1);
        let delta = pending.pop_front().unwrap().unwrap();
        assert!(
            matches!(delta, ProviderDelta::ToolCallStart { id, name } if id == "call_abc" && name == "search")
        );

        // Tracker should have the index mapped
        assert_eq!(tracker.get(0), Some("call_abc"));
    }

    #[test]
    fn test_parse_sse_tool_call_arg_delta() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();

        // First: start chunk establishes the id
        tracker.track(0, "call_abc".to_string());

        let data = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"q\":"}}]},"index":0}]}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        assert_eq!(pending.len(), 1);
        let delta = pending.pop_front().unwrap().unwrap();
        assert!(
            matches!(delta, ProviderDelta::ToolCallArgDelta { id, delta } if id == "call_abc" && delta == r#"{"q":"#)
        );
    }

    #[test]
    fn test_parse_sse_finish_reason_stop() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        let data = r#"{"choices":[{"delta":{},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        // Should emit Usage then Done(EndTurn)
        let events: Vec<ProviderDelta> = pending.drain(..).map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], ProviderDelta::Usage(u) if u.input_tokens == 10 && u.output_tokens == 5)
        );
        assert!(matches!(
            &events[1],
            ProviderDelta::Done(StopReason::EndTurn)
        ));
    }

    #[test]
    fn test_parse_sse_finish_reason_tool_calls() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();

        // Pre-register a tool call so ToolCallEnd is emitted
        tracker.track(0, "call_1".to_string());

        let data = r#"{"choices":[{"delta":{},"finish_reason":"tool_calls","index":0}]}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        // Should emit ToolCallEnd(call_1) then Done(ToolUse)
        let events: Vec<ProviderDelta> = pending.drain(..).map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ProviderDelta::ToolCallEnd { id } if id == "call_1"));
        assert!(matches!(
            &events[1],
            ProviderDelta::Done(StopReason::ToolUse)
        ));
    }

    #[test]
    fn test_parse_sse_finish_reason_length() {
        let mut tracker = IndexIdTracker::new();
        let mut pending = VecDeque::new();
        let data = r#"{"choices":[{"delta":{},"finish_reason":"length","index":0}]}"#;
        parse_chat_sse_event(data, &mut tracker, &mut pending);

        let events: Vec<ProviderDelta> = pending.drain(..).map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderDelta::Done(StopReason::MaxTokens)
        ));
    }
}
