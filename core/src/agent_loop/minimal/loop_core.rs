//! MinimalAgentLoop — the core think → act two-step loop.
//!
//! This is the heart of the minimal agent architecture. Each iteration:
//! 1. **Think**: Call the AI provider with the conversation history
//! 2. **Act**: Execute any tool calls the provider requested
//!
//! The loop terminates when:
//! - The provider returns text with `EndTurn` (task complete)
//! - `max_iterations` is reached
//! - Token budget is exhausted
//! - Timeout expires

use async_trait::async_trait;
use serde_json::Value;
use std::time::Instant;

use super::prompt_builder::{MinimalPromptBuilder, ToolInfo};
use super::safety::{SafetyError, SafetyGuard, ToolCall as SafetyToolCall};
use super::tool::{MinimalToolRegistry, ToolDefinition, ToolResult};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, StopReason, TokenUsage};

// =============================================================================
// MinimalProvider trait
// =============================================================================

/// Abstraction over AI provider for testability.
///
/// Implementations translate `LoopMessage` history into provider-specific
/// API calls and return a structured `ProviderResponse`.
#[async_trait]
pub trait MinimalProvider: Send + Sync {
    async fn call(
        &self,
        messages: &[LoopMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ProviderResponse>;
}

// =============================================================================
// LoopMessage
// =============================================================================

/// Messages in the conversation history.
#[derive(Debug, Clone)]
pub enum LoopMessage {
    User(String),
    Assistant(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        output: Value,
        is_error: bool,
    },
}

// =============================================================================
// LoopConfig
// =============================================================================

/// Loop configuration — guards against runaway loops.
pub struct LoopConfig {
    pub max_iterations: usize,
    pub token_budget: usize,
    pub timeout_secs: u64,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 25,
            token_budget: 100_000,
            timeout_secs: 300,
        }
    }
}

// =============================================================================
// LoopRunResult
// =============================================================================

/// Result of a loop run.
#[derive(Debug)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
}

// =============================================================================
// LoopCallback
// =============================================================================

/// Callback for streaming events during the loop.
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
}

/// No-op callback for when you don't need events.
pub struct NoopCallback;
impl LoopCallback for NoopCallback {}

// =============================================================================
// MinimalAgentLoop
// =============================================================================

/// The core agent loop: think → act, repeated until done.
pub struct MinimalAgentLoop<P: MinimalProvider> {
    provider: P,
    tool_registry: MinimalToolRegistry,
    prompt_builder: MinimalPromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
}

impl<P: MinimalProvider> MinimalAgentLoop<P> {
    /// Create a new agent loop with all dependencies injected.
    pub fn new(
        provider: P,
        tool_registry: MinimalToolRegistry,
        prompt_builder: MinimalPromptBuilder,
        safety_guard: SafetyGuard,
        config: LoopConfig,
    ) -> Self {
        Self {
            provider,
            tool_registry,
            prompt_builder,
            safety_guard,
            config,
        }
    }

    /// Run the agent loop with the given user input.
    ///
    /// This is THE CORE LOOP:
    /// 1. Build system prompt and tool definitions
    /// 2. Start with `LoopMessage::User(input)`
    /// 3. Loop: call provider → process response → execute tools → repeat
    /// 4. Stop on EndTurn, max iterations, token budget, or timeout
    pub async fn run(
        &self,
        input: &str,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        // Build system prompt with tool info (no memory context for now)
        let tool_infos: Vec<ToolInfo> = self
            .tool_registry
            .tool_definitions()
            .iter()
            .map(|td| ToolInfo {
                name: td.name.clone(),
                description: td.description.clone(),
            })
            .collect();
        let system_prompt = self.prompt_builder.build(&tool_infos, None);

        // Get tool definitions for the provider
        let tool_defs = self.tool_registry.tool_definitions();

        // Initialize conversation with user message
        let mut messages = vec![LoopMessage::User(input.to_string())];

        let mut final_text: Option<String> = None;
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut total_tokens: usize = 0;
        let mut hit_limit = false;

        let start = Instant::now();

        // === THE LOOP ===
        while iterations < self.config.max_iterations {
            // Check timeout
            if start.elapsed().as_secs() >= self.config.timeout_secs {
                hit_limit = true;
                break;
            }

            iterations += 1;

            // Think: call the provider
            let response = self
                .provider
                .call(&messages, &system_prompt, &tool_defs)
                .await?;

            // Track tokens
            if let Some(usage) = &response.usage {
                total_tokens += (usage.input_tokens + usage.output_tokens) as usize;
            }

            // Process text output
            if let Some(text) = &response.text {
                callback.on_text(text);
                final_text = Some(text.clone());
            }

            // If no tool calls and EndTurn → done
            if !response.has_tool_calls() && response.stop_reason == StopReason::EndTurn {
                break;
            }

            // If no tool calls but not EndTurn (e.g., MaxTokens) → done with limit
            if !response.has_tool_calls() {
                hit_limit = response.stop_reason == StopReason::MaxTokens;
                break;
            }

            // Act: process each tool call
            for tc in &response.tool_calls {
                // Push ToolUse message
                messages.push(LoopMessage::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                });

                // Safety check
                let safety_call = SafetyToolCall {
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                };

                match self.safety_guard.check(&safety_call) {
                    Err(SafetyError::Blocked { ref tool, ref pattern }) => {
                        let err = SafetyError::Blocked {
                            tool: tool.clone(),
                            pattern: pattern.clone(),
                        };
                        callback.on_safety_block(&err);
                        messages.push(LoopMessage::ToolResult {
                            id: tc.id.clone(),
                            output: Value::String(format!(
                                "BLOCKED: tool '{}' blocked by safety pattern '{}'",
                                tool, pattern
                            )),
                            is_error: true,
                        });
                    }
                    Err(SafetyError::NeedsConfirmation { ref tool }) => {
                        let err = SafetyError::NeedsConfirmation { tool: tool.clone() };
                        callback.on_safety_block(&err);
                        // TODO: wire to real UI confirmation flow
                        messages.push(LoopMessage::ToolResult {
                            id: tc.id.clone(),
                            output: Value::String(format!(
                                "NEEDS_CONFIRMATION: tool '{}' requires user approval (auto-denied for now)",
                                tool
                            )),
                            is_error: true,
                        });
                    }
                    Ok(()) => {
                        // Safe — execute the tool
                        callback.on_tool_start(&tc.name, &tc.arguments);
                        let result = self.tool_registry.execute(&tc.name, tc.arguments.clone()).await;
                        callback.on_tool_done(&tc.name, &result);

                        let (output, is_error) = match &result {
                            ToolResult::Success { output } => (output.clone(), false),
                            ToolResult::Error { error, .. } => {
                                (Value::String(error.clone()), true)
                            }
                        };

                        messages.push(LoopMessage::ToolResult {
                            id: tc.id.clone(),
                            output,
                            is_error,
                        });
                    }
                }

                tool_calls_made += 1;
            }

            // Check token budget
            if total_tokens >= self.config.token_budget {
                hit_limit = true;
                break;
            }
        }

        // Check if we hit max iterations
        if iterations >= self.config.max_iterations {
            hit_limit = true;
        }

        Ok(LoopRunResult {
            final_text,
            iterations,
            tool_calls_made,
            total_tokens,
            hit_limit,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    // =========================================================================
    // MockProvider — returns predetermined response sequences
    // =========================================================================

    struct MockProvider {
        responses: Mutex<Vec<ProviderResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            // Reverse so we can pop from the end (FIFO order)
            let mut responses = responses;
            responses.reverse();
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl MinimalProvider for MockProvider {
        async fn call(
            &self,
            _messages: &[LoopMessage],
            _system_prompt: &str,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<ProviderResponse> {
            let mut responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(resp) = responses.pop() {
                Ok(resp)
            } else {
                // If no more responses, return EndTurn to stop the loop
                Ok(ProviderResponse::text_only("(no more mock responses)".to_string()))
            }
        }
    }

    // =========================================================================
    // TrackingCallback — records events for test assertions
    // =========================================================================

    #[derive(Default)]
    struct TrackingCallback {
        texts: Vec<String>,
        tool_starts: Vec<String>,
        tool_dones: Vec<String>,
        safety_blocks: Vec<String>,
    }

    impl LoopCallback for TrackingCallback {
        fn on_text(&mut self, text: &str) {
            self.texts.push(text.to_string());
        }
        fn on_tool_start(&mut self, name: &str, _input: &Value) {
            self.tool_starts.push(name.to_string());
        }
        fn on_tool_done(&mut self, name: &str, _result: &ToolResult) {
            self.tool_dones.push(name.to_string());
        }
        fn on_safety_block(&mut self, error: &SafetyError) {
            self.safety_blocks.push(error.to_string());
        }
    }

    // =========================================================================
    // EchoTool for tests
    // =========================================================================

    struct EchoTool;

    #[async_trait]
    impl super::super::tool::MinimalTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"]
            })
        }
        async fn execute(&self, input: Value) -> ToolResult {
            ToolResult::Success { output: input }
        }
    }

    // =========================================================================
    // Helper to build a minimal loop
    // =========================================================================

    fn make_loop(provider: MockProvider) -> MinimalAgentLoop<MockProvider> {
        let mut registry = MinimalToolRegistry::new();
        registry.register(Box::new(EchoTool));

        MinimalAgentLoop::new(
            provider,
            registry,
            MinimalPromptBuilder::new(),
            SafetyGuard::new(vec![], vec![]),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
                timeout_secs: 60,
            },
        )
    }

    // =========================================================================
    // Tests
    // =========================================================================

    #[tokio::test]
    async fn test_simple_text_response() {
        // Provider returns text with EndTurn → loop completes in 1 iteration
        let provider = MockProvider::new(vec![ProviderResponse {
            text: Some("Hello, world!".to_string()),
            tool_calls: vec![],
            thinking: None,
            stop_reason: StopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: None,
            }),
        }]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("Hi", &mut cb).await.unwrap();

        assert_eq!(result.final_text.as_deref(), Some("Hello, world!"));
        assert_eq!(result.iterations, 1);
        assert_eq!(result.tool_calls_made, 0);
        assert_eq!(result.total_tokens, 15);
        assert!(!result.hit_limit);
        assert_eq!(cb.texts, vec!["Hello, world!"]);
    }

    #[tokio::test]
    async fn test_tool_call_then_response() {
        // Iteration 1: provider returns tool_call
        // Iteration 2: provider returns text (after seeing tool result)
        let provider = MockProvider::new(vec![
            // First response: tool call
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
                    input_tokens: 20,
                    output_tokens: 10,
                    cache_read_tokens: None,
                }),
            },
            // Second response: final text
            ProviderResponse {
                text: Some("Done echoing.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 30,
                    output_tokens: 5,
                    cache_read_tokens: None,
                }),
            },
        ]);

        let agent = make_loop(provider);
        let mut cb = TrackingCallback::default();
        let result = agent.run("Echo something", &mut cb).await.unwrap();

        assert_eq!(result.final_text.as_deref(), Some("Done echoing."));
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert_eq!(result.total_tokens, 65); // 30 + 35
        assert!(!result.hit_limit);
        assert_eq!(cb.tool_starts, vec!["echo"]);
        assert_eq!(cb.tool_dones, vec!["echo"]);
    }

    #[tokio::test]
    async fn test_max_iterations_guard() {
        // Provider always returns a tool call → should hit max_iterations
        let responses: Vec<ProviderResponse> = (0..15)
            .map(|i| ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: format!("call_{}", i),
                    name: "echo".to_string(),
                    arguments: json!({ "message": "loop" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: Some(TokenUsage {
                    input_tokens: 5,
                    output_tokens: 5,
                    cache_read_tokens: None,
                }),
            })
            .collect();

        let provider = MockProvider::new(responses);
        let agent = MinimalAgentLoop::new(
            provider,
            {
                let mut r = MinimalToolRegistry::new();
                r.register(Box::new(EchoTool));
                r
            },
            MinimalPromptBuilder::new(),
            SafetyGuard::new(vec![], vec![]),
            LoopConfig {
                max_iterations: 5,
                token_budget: 100_000,
                timeout_secs: 60,
            },
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("keep going", &mut cb).await.unwrap();

        assert_eq!(result.iterations, 5);
        assert!(result.hit_limit);
        assert_eq!(result.tool_calls_made, 5);
    }

    #[tokio::test]
    async fn test_safety_guard_blocks_tool() {
        // Provider requests a tool that the safety guard blocks
        let provider = MockProvider::new(vec![
            // First response: blocked tool call
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_bad".to_string(),
                    name: "shell".to_string(),
                    arguments: json!({ "command": "rm -rf /" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Second response: provider gives up after seeing the error
            ProviderResponse {
                text: Some("I cannot do that.".to_string()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let agent = MinimalAgentLoop::new(
            provider,
            MinimalToolRegistry::new(), // no tools registered
            MinimalPromptBuilder::new(),
            SafetyGuard::new(
                vec![r"rm\s+-rf\s+/".to_string()],
                vec![],
            ),
            LoopConfig {
                max_iterations: 10,
                token_budget: 100_000,
                timeout_secs: 60,
            },
        );

        let mut cb = TrackingCallback::default();
        let result = agent.run("delete everything", &mut cb).await.unwrap();

        assert_eq!(result.final_text.as_deref(), Some("I cannot do that."));
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
        assert!(!result.hit_limit);
        // Safety block callback was invoked
        assert_eq!(cb.safety_blocks.len(), 1);
        assert!(cb.safety_blocks[0].contains("blocked"));
        // Tool was NOT actually started (no on_tool_start)
        assert!(cb.tool_starts.is_empty());
    }
}
