//! Agent Loop Executor
//!
//! Implements the main agent loop for executing tool calls.

use super::conversation::ConversationHistory;
use super::types::{AgentConfig, AgentResult, ToolCallInfo, ToolCallResult};
use crate::error::Result;
use crate::tools::{NativeToolRegistry, ToolDefinition};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

// =============================================================================
// Chat Response (LLM output)
// =============================================================================

/// Response from an LLM chat call
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// Text content (may be None if only tool calls)
    pub content: Option<String>,

    /// Tool calls requested by the model
    pub tool_calls: Vec<ToolCallInfo>,

    /// Stop reason
    pub stop_reason: Option<String>,
}

impl ChatResponse {
    /// Create a text-only response
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            tool_calls: Vec::new(),
            stop_reason: Some("stop".to_string()),
        }
    }

    /// Create a response with tool calls
    pub fn with_tools(content: Option<String>, tool_calls: Vec<ToolCallInfo>) -> Self {
        Self {
            content,
            tool_calls,
            stop_reason: Some("tool_calls".to_string()),
        }
    }

    /// Check if this response has tool calls
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

// =============================================================================
// Tool Calling Provider Trait
// =============================================================================

/// Trait for providers that support tool calling
///
/// This extends the basic AiProvider with function calling capabilities.
#[async_trait::async_trait]
pub trait ToolCallingProvider: Send + Sync {
    /// Chat with tool definitions
    ///
    /// # Arguments
    ///
    /// * `messages` - Conversation history in API format
    /// * `tools` - Tool definitions for function calling
    /// * `system_prompt` - Optional system prompt
    ///
    /// # Returns
    ///
    /// ChatResponse with optional tool calls
    async fn chat_with_tools(
        &self,
        messages: &[Value],
        tools: &[ToolDefinition],
        system_prompt: Option<&str>,
    ) -> Result<ChatResponse>;

    /// Provider name
    fn name(&self) -> &str;
}

// =============================================================================
// Agent Loop
// =============================================================================

/// Agent loop for executing multi-turn tool calling interactions
///
/// The agent loop:
/// 1. Sends user input to the LLM with available tools
/// 2. If the LLM requests tool calls, executes them
/// 3. Sends tool results back to the LLM
/// 4. Repeats until the LLM provides a final response or max turns reached
pub struct AgentLoop<P: ToolCallingProvider> {
    /// The provider that supports tool calling
    provider: Arc<P>,

    /// Tool registry for executing tools
    registry: Arc<NativeToolRegistry>,

    /// Agent configuration
    config: AgentConfig,
}

impl<P: ToolCallingProvider> AgentLoop<P> {
    /// Create a new agent loop
    pub fn new(provider: Arc<P>, registry: Arc<NativeToolRegistry>) -> Self {
        Self {
            provider,
            registry,
            config: AgentConfig::default(),
        }
    }

    /// Set the agent configuration
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.system_prompt = Some(prompt.into());
        self
    }

    /// Set max turns
    pub fn with_max_turns(mut self, max_turns: usize) -> Self {
        self.config.max_turns = max_turns;
        self
    }

    /// Run the agent loop with user input
    ///
    /// # Arguments
    ///
    /// * `input` - Initial user input
    ///
    /// # Returns
    ///
    /// AgentResult with the final response and execution details
    pub async fn run(&self, input: &str) -> Result<AgentResult> {
        let start = Instant::now();

        // Initialize conversation
        let mut history = if let Some(ref prompt) = self.config.system_prompt {
            ConversationHistory::with_system_prompt(prompt)
        } else {
            ConversationHistory::new()
        };

        history.add_user_message(input);

        // Get available tools (async)
        let tool_defs: Vec<ToolDefinition> = self.registry.get_definitions().await;

        let mut turns = 0;
        let mut tool_calls_made = 0;
        let mut tool_history: Vec<ToolCallResult> = Vec::new();

        // Main agent loop
        loop {
            turns += 1;

            // Check max turns
            if turns > self.config.max_turns {
                warn!(
                    max_turns = self.config.max_turns,
                    "Agent loop: Max turns exceeded"
                );
                return Ok(AgentResult::failure(
                    "Maximum turns exceeded",
                    turns,
                    start.elapsed().as_millis() as u64,
                    tool_history,
                ));
            }

            info!(turn = turns, "Agent loop: Starting turn");

            // Build messages for API call
            let messages = history.to_openai_messages();

            // Call LLM with tools
            let response = match tokio::time::timeout(
                Duration::from_millis(self.config.turn_timeout_ms),
                self.provider
                    .chat_with_tools(&messages, &tool_defs, self.config.system_prompt.as_deref()),
            )
            .await
            {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    warn!(error = %e, turn = turns, "Agent loop: Provider error");
                    return Ok(AgentResult::failure(
                        format!("Provider error: {}", e),
                        turns,
                        start.elapsed().as_millis() as u64,
                        tool_history,
                    ));
                }
                Err(_) => {
                    warn!(turn = turns, "Agent loop: Turn timeout");
                    return Ok(AgentResult::failure(
                        "Turn timeout",
                        turns,
                        start.elapsed().as_millis() as u64,
                        tool_history,
                    ));
                }
            };

            // Check if we have tool calls
            if response.has_tool_calls() {
                debug!(
                    tool_count = response.tool_calls.len(),
                    "Agent loop: Executing tool calls"
                );

                // Add assistant message with tool calls
                history.add_assistant_with_tool_calls(
                    response.content.clone(),
                    response.tool_calls.clone(),
                );

                // Execute each tool call
                for tool_call in &response.tool_calls {
                    let result = self.execute_tool_call(tool_call).await;

                    // Track tool call
                    tool_calls_made += 1;

                    // Check for errors if stop_on_error is enabled
                    if self.config.stop_on_error && !result.success {
                        warn!(
                            tool = %tool_call.name,
                            error = ?result.error,
                            "Agent loop: Tool error, stopping"
                        );
                        tool_history.push(result);
                        return Ok(AgentResult::failure(
                            format!("Tool error: {}", tool_call.name),
                            turns,
                            start.elapsed().as_millis() as u64,
                            tool_history,
                        ));
                    }

                    // Add tool result to history
                    if self.config.include_tool_results {
                        history.add_tool_result(&result);
                    }

                    tool_history.push(result);
                }

                // Continue loop to send tool results back to LLM
                continue;
            }

            // No tool calls - this is the final response
            let final_response = response.content.unwrap_or_default();

            info!(
                turns,
                tool_calls_made,
                response_len = final_response.len(),
                "Agent loop: Complete"
            );

            return Ok(AgentResult::success(
                final_response,
                tool_calls_made,
                turns,
                start.elapsed().as_millis() as u64,
                tool_history,
            ));
        }
    }

    /// Execute a single tool call
    async fn execute_tool_call(&self, tool_call: &ToolCallInfo) -> ToolCallResult {
        let start = Instant::now();

        debug!(
            tool = %tool_call.name,
            id = %tool_call.id,
            "Agent loop: Executing tool"
        );

        // Convert arguments to string for execution
        let args_str = serde_json::to_string(&tool_call.arguments).unwrap_or_default();

        // Execute the tool via registry (handles tool lookup internally)
        match self.registry.execute(&tool_call.name, &args_str).await {
            Ok(result) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                debug!(
                    tool = %tool_call.name,
                    duration_ms,
                    "Agent loop: Tool execution complete"
                );

                if result.is_success() {
                    ToolCallResult::success(&tool_call.id, &tool_call.name, result.content, duration_ms)
                } else {
                    ToolCallResult::failure(
                        &tool_call.id,
                        &tool_call.name,
                        result.error.unwrap_or_else(|| "Unknown error".to_string()),
                        duration_ms,
                    )
                }
            }
            Err(e) => {
                let duration_ms = start.elapsed().as_millis() as u64;
                warn!(
                    tool = %tool_call.name,
                    error = %e,
                    "Agent loop: Tool execution failed"
                );
                ToolCallResult::failure(&tool_call.id, &tool_call.name, e.to_string(), duration_ms)
            }
        }
    }

    /// Build tool definitions for the registry
    pub async fn build_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.registry.get_definitions().await
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_response_text() {
        let response = ChatResponse::text("Hello!");
        assert_eq!(response.content, Some("Hello!".to_string()));
        assert!(!response.has_tool_calls());
    }

    #[test]
    fn test_chat_response_with_tools() {
        let tool_call = ToolCallInfo::new("call_1", "search", serde_json::json!({}));
        let response = ChatResponse::with_tools(Some("Let me search".to_string()), vec![tool_call]);

        assert!(response.has_tool_calls());
        assert_eq!(response.tool_calls.len(), 1);
    }

    // Mock provider for testing
    struct MockToolProvider {
        responses: Vec<ChatResponse>,
        call_count: std::sync::atomic::AtomicUsize,
    }

    impl MockToolProvider {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses,
                call_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolCallingProvider for MockToolProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[Value],
            _tools: &[ToolDefinition],
            _system_prompt: Option<&str>,
        ) -> Result<ChatResponse> {
            let idx = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if idx < self.responses.len() {
                Ok(self.responses[idx].clone())
            } else {
                Ok(ChatResponse::text("Done"))
            }
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_agent_loop_simple_response() {
        let provider = Arc::new(MockToolProvider::new(vec![ChatResponse::text(
            "Hello, how can I help?",
        )]));

        let registry = Arc::new(NativeToolRegistry::new());
        let agent = AgentLoop::new(provider, registry).with_max_turns(5);

        let result = agent.run("Hello").await.unwrap();

        assert!(result.success);
        assert_eq!(result.response, "Hello, how can I help?");
        assert_eq!(result.turns, 1);
        assert_eq!(result.tool_calls_made, 0);
    }

    #[tokio::test]
    async fn test_agent_loop_max_turns() {
        // Provider that always returns tool calls
        let tool_call = ToolCallInfo::new("call_1", "unknown_tool", serde_json::json!({}));
        let responses: Vec<ChatResponse> = (0..15)
            .map(|_| ChatResponse::with_tools(None, vec![tool_call.clone()]))
            .collect();

        let provider = Arc::new(MockToolProvider::new(responses));
        let registry = Arc::new(NativeToolRegistry::new());

        let agent = AgentLoop::new(provider, registry).with_max_turns(5);

        let result = agent.run("Keep calling tools").await.unwrap();

        assert!(!result.success);
        assert!(result.error.as_ref().unwrap().contains("Maximum turns"));
    }

    #[test]
    fn test_agent_config() {
        let config = AgentConfig::with_system_prompt("Be helpful")
            .max_turns(20)
            .turn_timeout_ms(60000)
            .stop_on_error(true);

        assert_eq!(config.system_prompt, Some("Be helpful".to_string()));
        assert_eq!(config.max_turns, 20);
        assert_eq!(config.turn_timeout_ms, 60000);
        assert!(config.stop_on_error);
    }
}
