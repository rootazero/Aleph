//! Integration probe tests: AgentLoop <-> AiProviderBridge <-> AiProvider
//!
//! Unlike the unit tests in loop_core.rs which mock LoopProvider directly,
//! these tests exercise the full bridge path through AiProviderBridge wrapping
//! a real AiProvider implementation.

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    use tokio_util::sync::CancellationToken;

    use crate::agent_loop::loop_core::{AgentLoop, LoopConfig, NoopCallback};
    use crate::agent_loop::provider_bridge::AiProviderBridge;
    use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
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
            PromptBuilder::new(PromptConfig::default()),
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

        let mut agent = make_probe_loop(
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
        assert_eq!(
            result.final_text.as_deref(),
            Some("Echo complete. <task-complete/>")
        );

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

        let mut agent = make_probe_loop(
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
        assert_eq!(
            result.final_text.as_deref(),
            Some("Handled with repaired history.")
        );

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

        let mut agent = make_probe_loop(
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

        let mut agent = make_probe_loop(
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
        use crate::agent_loop::context_budget::{ContextBudget, ContextBudgetConfig};
        use crate::agent_loop::context_compactor::{CompactorConfig, ContextCompactor};

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
        let mut agent = make_probe_loop(
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
        let mut agent = agent
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

        let mut agent = make_probe_loop(
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

        let mut agent = make_probe_loop(
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
        let sys = caps[0]
            .system_prompt
            .as_ref()
            .expect("system prompt should be set");
        assert!(!sys.is_empty());
        // The prompt builder includes tool descriptions in "Available Tools" section
        assert!(
            sys.contains("echo"),
            "system prompt should mention the echo tool"
        );
        assert!(
            sys.contains("Echoes the input back"),
            "system prompt should contain tool description"
        );
    }

    // =========================================================================
    // E8: Chain context propagated in run result
    // =========================================================================

    #[tokio::test]
    async fn test_chain_context_in_run_result() {
        use crate::agent_loop::chain_context::ChainContext;

        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let registry = LoopToolRegistry::new();

        let mut agent = make_probe_loop(
            vec![ProviderResponse {
                text: Some("Done.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            }],
            captured,
            registry,
        );

        // Default chain: depth=0, non-empty chain_id
        let mut cb = NoopCallback;
        let result = agent.run("hello", &mut cb).await.unwrap();
        assert!(!result.chain_id.is_empty(), "chain_id should be non-empty");
        assert_eq!(result.depth, 0, "root agent should have depth 0");

        // Now test with an explicit child chain
        let captured2 = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let registry2 = LoopToolRegistry::new();

        let root_chain = ChainContext::with_max_depth(3);
        let child_chain = root_chain.child().expect("child should succeed");
        let expected_chain_id = child_chain.chain_id.clone();

        let mut agent2 = make_probe_loop(
            vec![ProviderResponse {
                text: Some("Child done.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            }],
            captured2,
            registry2,
        )
        .with_chain(child_chain);

        let mut cb2 = NoopCallback;
        let result2 = agent2.run("child query", &mut cb2).await.unwrap();
        assert_eq!(result2.chain_id, expected_chain_id);
        assert_eq!(result2.depth, 1, "child agent should have depth 1");
    }

    // =========================================================================
    // E9: Tool refresh signal mechanism
    // =========================================================================

    #[tokio::test]
    async fn test_tool_refresh_signal() {
        use crate::agent_loop::tool_refresh::{build_refreshed_registry, ToolRefreshSource};
        use std::sync::atomic::{AtomicBool, Ordering};

        struct MockRefreshSource {
            flag: AtomicBool,
            tools: Vec<String>,
        }

        impl MockRefreshSource {
            fn new(tool_names: Vec<&str>) -> Self {
                Self {
                    flag: AtomicBool::new(false),
                    tools: tool_names.into_iter().map(String::from).collect(),
                }
            }

            fn signal(&self) {
                self.flag.store(true, Ordering::Release);
            }
        }

        /// A named tool wrapper for testing tool refresh with distinct names.
        struct NamedTool(String);

        #[async_trait::async_trait]
        impl crate::agent_loop::tool::LoopTool for NamedTool {
            fn name(&self) -> &str {
                &self.0
            }
            fn description(&self) -> &str {
                "named test tool"
            }
            fn schema(&self) -> serde_json::Value {
                json!({"type": "object"})
            }
            async fn execute(
                &self,
                _input: serde_json::Value,
            ) -> crate::agent_loop::tool::ToolResult {
                crate::agent_loop::tool::ToolResult::Success {
                    output: serde_json::Value::Null,
                }
            }
        }

        impl ToolRefreshSource for MockRefreshSource {
            fn poll_changes(&self) -> bool {
                self.flag.swap(false, Ordering::AcqRel)
            }

            fn fetch_tools(&self) -> Vec<Box<dyn crate::agent_loop::tool::LoopTool>> {
                self.tools
                    .iter()
                    .map(|name| -> Box<dyn crate::agent_loop::tool::LoopTool> {
                        Box::new(NamedTool(name.clone()))
                    })
                    .collect()
            }
        }

        let src = MockRefreshSource::new(vec!["tool_a", "tool_b"]);

        // Before signal: no changes
        assert!(!src.poll_changes());

        // After signal: changes detected, then clears
        src.signal();
        assert!(src.poll_changes(), "poll after signal should be true");
        assert!(!src.poll_changes(), "poll should reset to false");

        // fetch_tools returns the configured count
        let tools = src.fetch_tools();
        assert_eq!(tools.len(), 2);

        // build_refreshed_registry creates a working registry with distinct tools
        let registry = build_refreshed_registry(src.fetch_tools());
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 2);
    }

    // =========================================================================
    // E10: Skill prefetcher starts scan and returns handle
    // =========================================================================

    #[tokio::test]
    async fn test_skill_prefetcher_starts_scan() {
        use crate::agent_loop::skill_prefetch::{SkillDiscoverySource, SkillInfo, SkillPrefetcher};
        use std::time::Duration;

        struct MockDiscoverySource {
            skills: Vec<SkillInfo>,
        }

        impl SkillDiscoverySource for MockDiscoverySource {
            fn discover(
                &self,
            ) -> Pin<Box<dyn std::future::Future<Output = Vec<SkillInfo>> + Send + '_>>
            {
                Box::pin(async { self.skills.clone() })
            }
        }

        let source = Arc::new(MockDiscoverySource {
            skills: vec![
                SkillInfo {
                    name: "search".to_string(),
                    description: "Search skill".to_string(),
                    schema: None,
                },
                SkillInfo {
                    name: "translate".to_string(),
                    description: "Translate skill".to_string(),
                    schema: None,
                },
            ],
        });

        let prefetcher = SkillPrefetcher::new(source, Duration::ZERO);

        // start_scan should return Some(handle)
        let handle = prefetcher
            .start_scan()
            .expect("first scan should not be throttled");

        // Await handle — should return Some(skills) since cache is empty
        let result = handle.await.unwrap();
        assert!(result.is_some(), "first scan should detect new skills");

        let skills = result.unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "search");
        assert_eq!(skills[1].name, "translate");
    }

    // =========================================================================
    // E11: Stop hook allows → loop exits normally
    // =========================================================================

    #[tokio::test]
    async fn test_stop_hook_allows_completion() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let provider = Arc::new(ProbeProvider::new(
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
                // Turn 2: final text with completion tag
                ProviderResponse {
                    text: Some("All done. <task-complete/>".to_string()),
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
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let mut agent =
            AgentLoop::new(
                bridge,
                registry,
                PromptBuilder::new(PromptConfig::default()),
                SafetyGuard::default_guard(),
                LoopConfig {
                    max_iterations: 10,
                    token_budget: 100_000,
                },
                CancellationToken::new(),
            )
            .with_stop_hooks(vec![Box::new(
                crate::agent_loop::stop_hooks::ShellStopHook::new("allow_hook", "exit 0"),
            )
                as Box<dyn crate::agent_loop::stop_hooks::StopHookHandler>]);

        let mut cb = NoopCallback;
        let result = agent.run("test stop hooks", &mut cb).await.unwrap();

        // Hook allowed → loop should exit normally with 2 iterations
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert_eq!(
            result.final_text.as_deref(),
            Some("All done. <task-complete/>")
        );
    }

    // =========================================================================
    // E12: Stop hook blocks → loop continues with injected message
    // =========================================================================

    #[tokio::test]
    async fn test_stop_hook_blocks_then_allows() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let provider = Arc::new(ProbeProvider::new(
            vec![
                // Turn 1: call echo tool
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "test" }),
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
                // Turn 2: completion tag (will be blocked by hook)
                ProviderResponse {
                    text: Some("Done. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
                // Turn 3: after block injection, LLM responds with completion again
                ProviderResponse {
                    text: Some("Fixed and done. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        // Hook blocks first time (exit 2), then allows (the hook always blocks,
        // but the MAX_STOP_HOOK_BLOCKS=3 limit means after 3 blocks the loop
        // will exit). For this test we use a hook that blocks once by checking
        // a file marker.
        let hook_script = r#"
            MARKER="/tmp/aleph_e2e_hook_marker_$$"
            if [ ! -f /tmp/aleph_e2e_stop_hook_passed ]; then
                echo "tests not passing" && touch /tmp/aleph_e2e_stop_hook_passed && exit 2
            else
                rm -f /tmp/aleph_e2e_stop_hook_passed && exit 0
            fi
        "#;

        // Clean up any leftover marker
        let _ = std::fs::remove_file("/tmp/aleph_e2e_stop_hook_passed");

        let mut agent =
            AgentLoop::new(
                bridge,
                registry,
                PromptBuilder::new(PromptConfig::default()),
                SafetyGuard::default_guard(),
                LoopConfig {
                    max_iterations: 10,
                    token_budget: 100_000,
                },
                CancellationToken::new(),
            )
            .with_stop_hooks(vec![Box::new(
                crate::agent_loop::stop_hooks::ShellStopHook::new("conditional_hook", hook_script),
            )
                as Box<dyn crate::agent_loop::stop_hooks::StopHookHandler>]);

        let mut cb = NoopCallback;
        let result = agent.run("test blocking hook", &mut cb).await.unwrap();

        // Hook blocked once → LLM got a continuation message → then allowed
        assert!(
            result.iterations >= 3,
            "Expected at least 3 iterations (tool + block + retry), got {}",
            result.iterations
        );
        assert_eq!(
            result.final_text.as_deref(),
            Some("Fixed and done. <task-complete/>")
        );

        // Clean up
        let _ = std::fs::remove_file("/tmp/aleph_e2e_stop_hook_passed");
    }

    // =========================================================================
    // E13: Stop hook timeout → treated as error, non-blocking
    // =========================================================================

    #[tokio::test]
    async fn test_stop_hook_timeout_non_blocking() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let provider = Arc::new(ProbeProvider::new(
            vec![
                // Turn 1: call tool
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "hi" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
                // Turn 2: completion
                ProviderResponse {
                    text: Some("All done. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let mut agent = AgentLoop::new(
            bridge,
            registry,
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        )
        .with_stop_hooks(vec![Box::new(
            crate::agent_loop::stop_hooks::ShellStopHook::new("slow_hook", "sleep 60")
                .with_timeout(std::time::Duration::from_millis(100)),
        )
            as Box<dyn crate::agent_loop::stop_hooks::StopHookHandler>]);

        let mut cb = NoopCallback;
        let start = std::time::Instant::now();
        let result = agent.run("test timeout hook", &mut cb).await.unwrap();
        let elapsed = start.elapsed();

        // Hook timed out → treated as error (non-blocking) → loop exits normally
        assert_eq!(result.iterations, 2);
        assert_eq!(
            result.final_text.as_deref(),
            Some("All done. <task-complete/>")
        );
        // Should not wait for the full 60s sleep
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "Hook timeout should be fast, took {:?}",
            elapsed
        );
    }

    // =========================================================================
    // E14: Cancellation before streaming → clean exit
    // =========================================================================

    #[tokio::test]
    async fn test_cancellation_during_streaming_clean_exit() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let registry = LoopToolRegistry::new();

        let provider = Arc::new(ProbeProvider::new(
            vec![
                // This response should never be fully consumed because
                // the cancel token is already fired before the loop starts.
                ProviderResponse {
                    text: Some("Should not appear.".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let cancel = CancellationToken::new();
        // Fire cancellation BEFORE the run — the select! in the streaming loop
        // should detect it immediately on the first delta poll.
        cancel.cancel();

        let mut agent = AgentLoop::new(
            bridge,
            registry,
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            cancel,
        );

        let mut cb = NoopCallback;
        let result = agent.run("test cancel", &mut cb).await.unwrap();

        assert!(result.cancelled, "Result should be marked as cancelled");
    }

    // =========================================================================
    // E15: Tool refresh rebuilds registry mid-loop
    // =========================================================================

    #[tokio::test]
    async fn test_tool_refresh_rebuilds_registry_mid_loop() {
        use crate::agent_loop::tool_refresh::ToolRefreshSource;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct DynamicRefreshSource {
            flag: AtomicBool,
        }

        impl ToolRefreshSource for DynamicRefreshSource {
            fn poll_changes(&self) -> bool {
                self.flag.swap(false, Ordering::AcqRel)
            }
            fn fetch_tools(&self) -> Vec<Box<dyn crate::agent_loop::tool::LoopTool>> {
                vec![Box::new(EchoTool), Box::new(UpperTool)]
            }
        }

        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        // Start with only EchoTool
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));

        let refresh_source = Arc::new(DynamicRefreshSource {
            flag: AtomicBool::new(true), // signal refresh on first poll
        });

        let provider = Arc::new(ProbeProvider::new(
            vec![
                // Turn 1: call echo tool (triggers refresh after tool execution)
                ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "trigger refresh" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
                // Turn 2: final response
                ProviderResponse {
                    text: Some("Registry refreshed. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let mut agent = AgentLoop::new(
            bridge,
            registry,
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        )
        .with_tool_refresh(refresh_source);

        let mut cb = NoopCallback;
        let result = agent.run("test refresh", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        // After refresh, tool definitions should include both echo and upper
        let final_defs = agent.tool_definitions();
        assert_eq!(
            final_defs.len(),
            2,
            "After refresh, registry should have 2 tools (echo + upper), got {}",
            final_defs.len()
        );
        let names: Vec<&str> = final_defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"upper"));
    }

    // =========================================================================
    // E16: Chain context depth enforcement across nested calls
    // =========================================================================

    #[tokio::test]
    async fn test_chain_context_depth_enforcement() {
        use crate::agent_loop::chain_context::ChainContext;

        // Create a chain at max depth
        let root = ChainContext::with_max_depth(1);
        let child = root.child().expect("depth 0→1 should succeed");
        assert_eq!(child.depth, 1);

        // At max depth, child() returns None
        let grandchild = child.child();
        assert!(
            grandchild.is_none(),
            "depth 1→2 should be None (max_depth=1)"
        );

        // Verify the chain context propagates through an AgentLoop run
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let registry = LoopToolRegistry::new();

        let provider = Arc::new(ProbeProvider::new(
            vec![ProviderResponse {
                text: Some("Depth-limited response.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            }],
            captured,
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let mut agent = AgentLoop::new(
            bridge,
            registry,
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        )
        .with_chain(child);

        let mut cb = NoopCallback;
        let result = agent.run("test at max depth", &mut cb).await.unwrap();
        assert_eq!(result.depth, 1, "should run at depth 1");
        assert_eq!(result.chain_id, root.chain_id, "chain_id should match root");
    }

    // =========================================================================
    // E17: Multiple concurrent tool calls with mixed results
    // =========================================================================

    #[tokio::test]
    async fn test_multiple_tools_mixed_success_and_error() {
        /// A tool that always errors.
        struct ErrorTool;

        #[async_trait::async_trait]
        impl LoopTool for ErrorTool {
            fn name(&self) -> &str {
                "error_tool"
            }
            fn description(&self) -> &str {
                "Always errors"
            }
            fn schema(&self) -> serde_json::Value {
                json!({"type": "object", "properties": {}})
            }
            async fn execute(&self, _input: serde_json::Value) -> ToolResult {
                ToolResult::Error {
                    error: "intentional error".into(),
                    retryable: false,
                }
            }
        }

        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(ErrorTool));

        let provider = Arc::new(ProbeProvider::new(
            vec![
                // Turn 1: call both tools
                ProviderResponse {
                    text: None,
                    tool_calls: vec![
                        NativeToolCall {
                            id: "call_ok".to_string(),
                            name: "echo".to_string(),
                            arguments: json!({ "message": "good" }),
                        },
                        NativeToolCall {
                            id: "call_err".to_string(),
                            name: "error_tool".to_string(),
                            arguments: json!({}),
                        },
                    ],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                },
                // Turn 2: LLM handles the error gracefully
                ProviderResponse {
                    text: Some("Partial success. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                },
            ],
            captured.clone(),
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let mut agent = AgentLoop::new(
            bridge,
            registry,
            PromptBuilder::new(PromptConfig::default()),
            SafetyGuard::default_guard(),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
            },
            CancellationToken::new(),
        );

        let mut cb = NoopCallback;
        let result = agent.run("test mixed tools", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 2);
        assert_eq!(
            result.final_text.as_deref(),
            Some("Partial success. <task-complete/>")
        );

        // Verify both tool results appear in the second request
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 2);
        let second_msgs = &caps[1].messages;
        let tool_results: Vec<_> = second_msgs
            .iter()
            .filter(|m| matches!(m, UnifiedMessage::ToolResult { .. }))
            .collect();
        assert_eq!(
            tool_results.len(),
            2,
            "Both tool results should be in the history"
        );
    }

    // =========================================================================
    // E18: Full end-to-end: tool call → intermediate text → completion with hooks
    // =========================================================================

    #[tokio::test]
    async fn test_full_e2e_tool_intermediate_completion_hooks() {
        let captured = Arc::new(Mutex::new(Vec::<CapturedRequest>::new()));
        let mut registry = LoopToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(UpperTool));

        let provider = Arc::new(ProbeProvider::new(
            vec![
                // Turn 1: call echo with intermediate text
                ProviderResponse {
                    text: Some("Let me search for that...".to_string()),
                    tool_calls: vec![NativeToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: json!({ "message": "query" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input_tokens: 10,
                        output_tokens: 15,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 2: call upper tool
                ProviderResponse {
                    text: Some("Processing results...".to_string()),
                    tool_calls: vec![NativeToolCall {
                        id: "call_2".to_string(),
                        name: "upper".to_string(),
                        arguments: json!({ "text": "result" }),
                    }],
                    thinking: None,
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input_tokens: 30,
                        output_tokens: 10,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
                // Turn 3: final answer with completion tag
                ProviderResponse {
                    text: Some("Here is your answer: RESULT. <task-complete/>".to_string()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: Some(TokenUsage {
                        input_tokens: 50,
                        output_tokens: 20,
                        cache_read_tokens: None,
                        thinking_tokens: None,
                    }),
                },
            ],
            captured.clone(),
        )) as Arc<dyn AiProvider>;
        let bridge = AiProviderBridge::new(provider);

        let mut agent =
            AgentLoop::new(
                bridge,
                registry,
                PromptBuilder::new(PromptConfig::default()),
                SafetyGuard::default_guard(),
                LoopConfig {
                    max_iterations: 10,
                    token_budget: 500_000,
                },
                CancellationToken::new(),
            )
            .with_stop_hooks(vec![Box::new(
                crate::agent_loop::stop_hooks::ShellStopHook::new("pass_hook", "exit 0"),
            )
                as Box<dyn crate::agent_loop::stop_hooks::StopHookHandler>]);

        let mut cb = NoopCallback;
        let result = agent.run("full e2e test", &mut cb).await.unwrap();

        // Verify full execution path
        assert_eq!(result.iterations, 3, "should complete in 3 turns");
        assert_eq!(result.tool_calls_made, 2, "2 tools should have been called");
        assert!(!result.hit_limit);
        assert!(!result.cancelled);
        assert!(result.total_tokens > 0, "tokens should be tracked");

        // Verify final text (intermediate texts stripped from beginning)
        let final_text = result.final_text.as_deref().unwrap();
        assert!(
            final_text.contains("RESULT"),
            "Final text should contain the answer"
        );
        assert!(
            final_text.contains("<task-complete/>"),
            "Final text should contain completion tag"
        );

        // Verify message history accumulation
        let caps = captured.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(caps.len(), 3, "3 provider calls");

        // Each subsequent call should have more messages
        assert!(
            caps[1].messages.len() > caps[0].messages.len(),
            "2nd call should have more messages than 1st"
        );
        assert!(
            caps[2].messages.len() > caps[1].messages.len(),
            "3rd call should have more messages than 2nd"
        );

        // Chain context should be present
        assert!(!result.chain_id.is_empty());
        assert_eq!(result.depth, 0, "root agent should be depth 0");
    }
}
