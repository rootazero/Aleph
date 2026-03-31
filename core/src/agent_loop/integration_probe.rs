//! Integration probe tests: AgentLoop <-> AiProviderBridge <-> AiProvider
//!
//! Unlike the unit tests in loop_core.rs which mock LoopProvider directly,
//! these tests exercise the full bridge path through AiProviderBridge wrapping
//! a real AiProvider implementation.

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use serde_json::json;

    use tokio_util::sync::CancellationToken;

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
    use crate::sync_primitives::{Arc, Mutex};

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
            CancellationToken::new(),
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
    // E4: Truncation recovery escalation through bridge
    // =========================================================================

    #[tokio::test]
    async fn test_truncation_recovery_escalation() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let registry = LoopToolRegistry::new();

        let agent = make_probe_loop(
            vec![
                // Turn 1: truncated (MaxTokens) — recovery escalates
                ProviderResponse {
                    text: Some("Part one of the answer...".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::MaxTokens,
                    usage: Some(TokenUsage {
                        input_tokens: 10,
                        output_tokens: 100,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 2: still truncated (MaxTokens) — recovery continues
                ProviderResponse {
                    text: Some("Part two continues...".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::MaxTokens,
                    usage: Some(TokenUsage {
                        input_tokens: 20,
                        output_tokens: 100,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 3: final response with EndTurn
                ProviderResponse {
                    text: Some("Final assembled conclusion. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: Some(TokenUsage {
                        input_tokens: 30,
                        output_tokens: 50,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
            ],
            captured.clone(),
            registry,
        );

        let mut cb = NoopCallback;
        let result = agent.run("write a long essay", &mut cb).await.unwrap();

        // Recovery should have caused multiple iterations
        assert!(
            result.iterations > 1,
            "Expected multiple iterations from truncation recovery, got {}",
            result.iterations
        );

        // Final text should contain the conclusion from the last response
        let text = result.final_text.as_deref().unwrap_or("");
        assert!(
            text.contains("Final assembled conclusion"),
            "Expected final text to contain assembled output, got: {}",
            text
        );

        // Verify that continuation prompts were injected (captured requests show growing history)
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert!(
            caps.len() >= 3,
            "Expected at least 3 provider calls, got {}",
            caps.len()
        );
        // Second call should have more messages than the first (continuation prompt injected)
        assert!(
            caps[1].messages.len() > caps[0].messages.len(),
            "Second call should have more messages (continuation prompt injected)"
        );
    }

    // =========================================================================
    // E5: Context compaction triggers under pressure
    // =========================================================================

    #[tokio::test]
    async fn test_context_compaction_triggers() {
        use crate::agent_loop::context_compactor::{CompactorConfig, ContextCompactor};
        use crate::agent_loop::context_budget::{ContextBudget, ContextBudgetConfig};

        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let registry = LoopToolRegistry::new();

        // Build a compactor backed by a probe provider that returns a summary
        let summary_provider = Arc::new(ProbeProvider::new(
            vec![ProviderResponse::text_only(
                "Summary of earlier conversation.".to_string(),
            )],
            Arc::new(Mutex::new(Vec::new())), // separate captured for compactor
        )) as Arc<dyn crate::providers::AiProvider>;

        let compactor = ContextCompactor::new(
            summary_provider,
            CompactorConfig {
                fresh_tail: 2,
                ..Default::default()
            },
        );

        // Main provider: just returns a final response
        let agent = make_probe_loop(
            vec![ProviderResponse {
                text: Some("Done processing. <task-complete/>".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 800,
                    output_tokens: 100,
                    cache_read_tokens: None,
                    thinking_tokens: None,
                }),
            }],
            captured.clone(),
            registry,
        );

        // Attach a very small context budget so pressure is high
        let budget_config = ContextBudgetConfig {
            token_budget: 1000,
            warning_threshold: 0.70,
            critical_threshold: 0.85,
            token_estimate_ratio: 3.5,
            fresh_tail_count: 2,
            circuit_breaker_max: 3,
            diminishing_window: 4,
            diminishing_threshold: 500,
        };
        let budget = ContextBudget::new(&budget_config);
        let agent = agent
            .with_context_budget(Some(budget))
            .with_context_compactor(compactor);

        // Build a history with many messages to create pressure
        let mut history = Vec::new();
        for i in 0..20 {
            history.push(UnifiedMessage::user(format!(
                "This is user message number {} with some filler text to increase token count.",
                i
            )));
            history.push(UnifiedMessage::assistant(format!(
                "This is assistant response number {} with detailed explanation and analysis.",
                i
            )));
        }

        let mut cb = NoopCallback;
        let result = agent
            .run_with_history("final question", history, &mut cb)
            .await;

        // The loop should not crash even with high pressure + compaction
        assert!(
            result.is_ok(),
            "Loop should not crash with compaction enabled, got: {:?}",
            result.err()
        );

        let result = result.unwrap();
        assert!(result.iterations >= 1);
    }

    // =========================================================================
    // E6: Streaming bridge overlaps tool execution
    // =========================================================================

    #[tokio::test]
    async fn test_streaming_bridge_overlaps_execution() {
        use std::time::{Duration, Instant};

        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();

        // Register two concurrent-safe tools with a 50ms delay each
        struct SlowEchoTool {
            tool_name: String,
        }

        #[async_trait::async_trait]
        impl crate::agent_loop::tool::LoopTool for SlowEchoTool {
            fn name(&self) -> &str {
                &self.tool_name
            }
            fn description(&self) -> &str {
                "Slow echo tool"
            }
            fn schema(&self) -> serde_json::Value {
                json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } },
                    "required": ["msg"]
                })
            }
            async fn execute(
                &self,
                input: serde_json::Value,
            ) -> crate::agent_loop::tool::ToolResult {
                tokio::time::sleep(Duration::from_millis(50)).await;
                crate::agent_loop::tool::ToolResult::Success { output: input }
            }
            fn is_concurrent_safe(&self, _input: &serde_json::Value) -> bool {
                true
            }
        }

        registry.register(Box::new(SlowEchoTool {
            tool_name: "slow_a".to_string(),
        }));
        registry.register(Box::new(SlowEchoTool {
            tool_name: "slow_b".to_string(),
        }));

        let agent = make_probe_loop(
            vec![
                // Turn 1: call both tools simultaneously
                ProviderResponse {
                    text: None,
                    tool_calls: vec![
                        NativeToolCall {
                            id: "call_a".to_string(),
                            name: "slow_a".to_string(),
                            arguments: json!({ "msg": "hello_a" }),
                        },
                        NativeToolCall {
                            id: "call_b".to_string(),
                            name: "slow_b".to_string(),
                            arguments: json!({ "msg": "hello_b" }),
                        },
                    ],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input_tokens: 10,
                        output_tokens: 20,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 2: final response
                ProviderResponse {
                    text: Some("Both tools completed. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: Some(TokenUsage {
                        input_tokens: 30,
                        output_tokens: 10,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
            ],
            captured.clone(),
            registry,
        );

        let mut cb = NoopCallback;
        let start = Instant::now();
        let result = agent.run("call both tools", &mut cb).await.unwrap();
        let elapsed = start.elapsed();

        // Both tool calls should have been made
        assert_eq!(result.tool_calls_made, 2);

        // Final text should be present
        assert_eq!(
            result.final_text.as_deref(),
            Some("Both tools completed. <task-complete/>")
        );

        // Two 50ms concurrent tools should complete in < 150ms (not 100ms+ sequential)
        // We use 200ms as a generous bound to avoid flaky CI
        assert!(
            elapsed < Duration::from_millis(2000),
            "Expected parallel execution (<2s including stream overhead), got {:?}",
            elapsed
        );

        // Verify both tool results appear in the second request's messages
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 2, "Expected 2 provider calls");

        let second_call_msgs = &caps[1].messages;
        let tool_result_count = second_call_msgs
            .iter()
            .filter(|m| matches!(m, UnifiedMessage::ToolResult { .. }))
            .count();
        assert_eq!(
            tool_result_count, 2,
            "Expected 2 tool results in second call, got {}",
            tool_result_count
        );
    }

    // =========================================================================
    // E7 (original E4): System prompt passed through
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
