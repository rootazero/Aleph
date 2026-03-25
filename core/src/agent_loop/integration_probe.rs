//! Integration probe tests: AgentLoop <-> AiProviderBridge <-> AiProvider
//!
//! Unlike the unit tests in loop_core.rs which mock LoopProvider directly,
//! these tests exercise the full bridge path through AiProviderBridge wrapping
//! a real AiProvider implementation.

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    use serde_json::json;

    use crate::agent_loop::loop_core::{AgentLoop, LoopConfig, NoopCallback};
    use crate::agent_loop::prompt_builder::PromptBuilder;
    use crate::agent_loop::provider_bridge::AiProviderBridge;
    use crate::agent_loop::safety::SafetyGuard;
    use crate::agent_loop::tool::{LoopTool, LoopToolRegistry, ToolResult};
    use crate::providers::adapter::{
        NativeToolCall, ProviderResponse, RequestPayload, StopReason, TokenUsage,
    };
    use crate::providers::message::{ContentBlock, UnifiedMessage};
    use crate::providers::AiProvider;
    use crate::sync_primitives::Arc;

    // =========================================================================
    // Test infrastructure
    // =========================================================================

    /// Captured request data from a ProbeProvider call.
    #[derive(Debug)]
    struct CapturedRequest {
        messages: Vec<UnifiedMessage>,
        system_prompt: Option<String>,
        tool_count: usize,
    }

    /// A test AiProvider that records requests and returns pre-configured responses.
    struct ProbeProvider {
        responses: Mutex<Vec<ProviderResponse>>,
        captured: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    impl ProbeProvider {
        fn new(
            responses: Vec<ProviderResponse>,
            captured: Arc<Mutex<Vec<CapturedRequest>>>,
        ) -> Self {
            let mut responses = responses;
            responses.reverse();
            Self {
                responses: Mutex::new(responses),
                captured,
            }
        }
    }

    impl AiProvider for ProbeProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            // Capture request details
            let captured_req = CapturedRequest {
                messages: payload.messages.to_vec(),
                system_prompt: payload.system_prompt.map(|s| s.to_string()),
                tool_count: payload.tools.map(|t| t.len()).unwrap_or(0),
            };
            self.captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(captured_req);

            let resp = {
                let mut responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(resp) = responses.pop() {
                    resp
                } else {
                    ProviderResponse::text_only("(no more probe responses)".to_string())
                }
            };

            Box::pin(async move { Ok(resp) })
        }

        fn name(&self) -> &str {
            "probe"
        }

        fn color(&self) -> &str {
            "#00FF00"
        }

        fn supports_native_tools(&self) -> bool {
            true
        }
    }

    /// Simple echo tool for integration tests.
    struct EchoTool;

    #[async_trait::async_trait]
    impl LoopTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }
        async fn execute(&self, input: serde_json::Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    /// Another tool for testing tool count.
    struct UpperTool;

    #[async_trait::async_trait]
    impl LoopTool for UpperTool {
        fn name(&self) -> &str {
            "upper"
        }
        fn description(&self) -> &str {
            "Uppercases text"
        }
        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            })
        }
        async fn execute(&self, input: serde_json::Value) -> ToolResult {
            let text = input["text"].as_str().unwrap_or("");
            ToolResult::Success {
                output: json!({ "result": text.to_uppercase() }),
            }
        }
    }

    /// Helper: create an AgentLoop backed by AiProviderBridge + ProbeProvider.
    fn make_probe_loop(
        responses: Vec<ProviderResponse>,
        captured: Arc<Mutex<Vec<CapturedRequest>>>,
        registry: LoopToolRegistry,
    ) -> AgentLoop<AiProviderBridge> {
        let provider = Arc::new(ProbeProvider::new(responses, captured)) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        AgentLoop::new(
            bridge,
            registry,
            PromptBuilder::new(),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
        )
    }

    // =========================================================================
    // E1: Full tool call cycle through bridge
    // =========================================================================

    #[tokio::test]
    async fn test_full_tool_call_cycle_through_bridge() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let agent = make_probe_loop(
            vec![
                // Turn 1: call echo tool
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "hello" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: None,
                thinking_tokens: None,
                    }),
                },
                // Turn 2: final text
                ProviderResponse {
                    text: Some("Echo complete. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: Some(TokenUsage {
                        input_tokens: 20,
                        output_tokens: 5,
                        cache_read_tokens: None,
                thinking_tokens: None,
                    }),
                },
            ],
            captured.clone(),
            registry,
        );

        let mut cb = NoopCallback;
        let result = agent.run("test echo", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert_eq!(result.final_text.as_deref(), Some("Echo complete. <task-complete/>"));

        // Verify captured requests show history accumulation
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 2);

        // First call: just the user message
        assert_eq!(caps[0].messages.len(), 1);

        // Second call: user + assistant(tool_call) + tool_result
        assert_eq!(caps[1].messages.len(), 3);
        // Verify the tool result is present
        match &caps[1].messages[2] {
            UnifiedMessage::ToolResult {
                tool_call_id,
                tool_name,
                is_error,
                ..
            } => {
                assert_eq!(tool_call_id, "call_1");
                assert_eq!(tool_name, "echo");
                assert!(!is_error);
            }
            _ => panic!("expected ToolResult message"),
        }
    }

    // =========================================================================
    // E2: Orphaned tool call repair
    // =========================================================================

    #[tokio::test]
    async fn test_orphaned_tool_call_repair() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let agent = make_probe_loop(
            vec![
                // Immediate text response
                ProviderResponse {
                    text: Some("Handled with repaired history.".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
            registry,
        );

        // Inject history with an orphaned tool call (no matching ToolResult)
        let history = vec![
            UnifiedMessage::user("old question"),
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "orphan_1".to_string(),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "orphaned" }),
                }],
            },
            // Note: NO ToolResult for orphan_1
        ];

        let mut cb = NoopCallback;
        let result = agent
            .run_with_history("new question", history, &mut cb)
            .await
            .unwrap();

        assert_eq!(result.iterations, 1);
        assert_eq!(result.final_text.as_deref(), Some("Handled with repaired history."));

        // Verify the bridge's transform_messages inserted a synthetic ToolResult
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 1);

        let messages = &caps[0].messages;
        // Original: [old_user, assistant(tool_call), new_user]
        // After transform_messages repair: should have synthetic tool_result appended
        // The bridge receives the raw messages from AgentLoop, then transform_messages
        // adds the synthetic ToolResult. So we expect 4 messages:
        // [old_user, assistant(tool_call), new_user, synthetic_tool_result]
        assert_eq!(messages.len(), 4);

        // The last message should be the synthetic error ToolResult
        match &messages[3] {
            UnifiedMessage::ToolResult {
                tool_call_id,
                is_error,
                content,
                ..
            } => {
                assert_eq!(tool_call_id, "orphan_1");
                assert!(is_error);
                let text = content[0].as_text().unwrap();
                assert!(text.contains("interrupted"));
            }
            _ => panic!("expected synthetic ToolResult for orphaned tool call"),
        }
    }

    // =========================================================================
    // E3: Tool definitions passed through
    // =========================================================================

    #[tokio::test]
    async fn test_tool_definitions_passed_through() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(UpperTool));

        let agent = make_probe_loop(
            vec![ProviderResponse {
                text: Some("Done.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            }],
            captured.clone(),
            registry,
        );

        let mut cb = NoopCallback;
        let _result = agent.run("simple query", &mut cb).await.unwrap();

        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 1);
        // ProbeProvider should have received 2 tool definitions
        assert_eq!(caps[0].tool_count, 2);
    }

    // =========================================================================
    // E4: System prompt passed through
    // =========================================================================

    #[tokio::test]
    async fn test_system_prompt_passed_through() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let agent = make_probe_loop(
            vec![ProviderResponse {
                text: Some("OK.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            }],
            captured.clone(),
            registry,
        );

        let mut cb = NoopCallback;
        let _result = agent.run("hello", &mut cb).await.unwrap();

        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 1);

        // System prompt should be non-empty and contain tool description
        let sys = caps[0].system_prompt.as_ref().expect("system prompt should be set");
        assert!(!sys.is_empty());
        // The prompt builder includes tool descriptions in "Available Tools" section
        assert!(sys.contains("echo"), "system prompt should mention the echo tool");
        assert!(
            sys.contains("Echoes the input back"),
            "system prompt should contain tool description"
        );
    }
}
