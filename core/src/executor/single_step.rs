//! Single-step executor for Agent Loop
//!
//! This module provides a simplified executor that executes a single tool call
//! and returns the result. It is designed for use with the Agent Loop architecture
//! where the Thinker decides the next action based on each step's result.
//!
//! # Architecture
//!
//! Unlike the UnifiedExecutor which handles full execution plans (including
//! multi-step DAG graphs), SingleStepExecutor focuses on:
//!
//! 1. **Single tool execution**: One tool call at a time
//! 2. **Immediate result**: Returns result for Thinker to process
//! 3. **No planning**: Agent Loop's Thinker handles action selection
//!
//! # Usage
//!
//! ```ignore
//! use aethecore::executor::SingleStepExecutor;
//! use aethecore::agent_loop::{Action, ActionResult};
//!
//! // Create executor
//! let executor = SingleStepExecutor::new(tool_registry);
//!
//! // Execute single action
//! let result = executor.execute(&action).await;
//!
//! // Thinker processes result and decides next action
//! ```

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info};

use crate::agent_loop::{Action, ActionExecutor, ActionResult};
use crate::dispatcher::UnifiedTool;
use crate::error::{AetherError, Result};

/// Normalize tool name by extracting base tool name from various formats
///
/// LLMs sometimes return tool names with operation suffixes like:
/// - "file_ops:mkdir" -> "file_ops"
/// - "file_ops.write" -> "file_ops"
/// - "file_ops:write:extra" -> "file_ops"
///
/// This function extracts the base tool name for registry lookup.
fn normalize_tool_name(tool_name: &str) -> String {
    // Check for colon separator (e.g., "file_ops:mkdir")
    if let Some(pos) = tool_name.find(':') {
        return tool_name[..pos].to_string();
    }
    // Check for dot separator (e.g., "file_ops.write")
    if let Some(pos) = tool_name.find('.') {
        return tool_name[..pos].to_string();
    }
    // No separator found, return as-is
    tool_name.to_string()
}

/// Configuration for single-step executor
#[derive(Debug, Clone)]
pub struct SingleStepConfig {
    /// Timeout for tool execution in seconds (default: 30)
    pub timeout_seconds: u64,
    /// Maximum output size in bytes (default: 1MB)
    pub max_output_size: usize,
}

impl Default for SingleStepConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 300,
            max_output_size: 1024 * 1024, // 1MB
        }
    }
}

/// Trait for tool registry lookup
pub trait ToolRegistry: Send + Sync {
    /// Look up a tool by name
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool>;
    /// Execute a tool call
    fn execute_tool(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>>;
}

/// Single-step executor for Agent Loop
///
/// Executes individual actions as part of the observe-think-act loop.
pub struct SingleStepExecutor<R: ToolRegistry> {
    /// Tool registry for looking up and executing tools
    tool_registry: Arc<R>,
    /// Configuration
    config: SingleStepConfig,
    /// Tool result cache
    result_cache: Arc<super::cache_store::ToolResultCache>,
}

impl<R: ToolRegistry> SingleStepExecutor<R> {
    /// Create a new single-step executor
    pub fn new(tool_registry: Arc<R>) -> Self {
        let cache_config = super::cache_config::ToolCacheConfig::default();
        Self {
            tool_registry,
            config: SingleStepConfig::default(),
            result_cache: Arc::new(super::cache_store::ToolResultCache::new(cache_config)),
        }
    }

    /// Create with custom configuration
    pub fn with_config(tool_registry: Arc<R>, config: SingleStepConfig) -> Self {
        let cache_config = super::cache_config::ToolCacheConfig::default();
        Self {
            tool_registry,
            config,
            result_cache: Arc::new(super::cache_store::ToolResultCache::new(cache_config)),
        }
    }

    /// Create with custom cache configuration
    pub fn with_cache_config(
        tool_registry: Arc<R>,
        cache_config: super::cache_config::ToolCacheConfig,
    ) -> Self {
        Self {
            tool_registry,
            config: SingleStepConfig::default(),
            result_cache: Arc::new(super::cache_store::ToolResultCache::new(cache_config)),
        }
    }

    /// Execute a tool call
    async fn execute_tool_call(&self, tool_name: &str, arguments: Value) -> ActionResult {
        let start = Instant::now();

        // Normalize tool name: extract base tool name from formats like "file_ops:mkdir" or "file_ops.write"
        // LLMs sometimes return tool names with operation suffix, but we need the base tool name
        let normalized_tool_name = normalize_tool_name(tool_name);
        debug!(
            original_tool = tool_name,
            normalized_tool = %normalized_tool_name,
            "Executing tool call"
        );

        // Try to lookup result from cache
        if let Some(cached_result) = self
            .result_cache
            .lookup(&normalized_tool_name, &arguments)
            .await
        {
            let duration_ms = start.elapsed().as_millis() as u64;
            info!(
                tool = tool_name,
                duration_ms,
                "Tool result returned from cache"
            );
            return cached_result;
        }

        // Check if tool exists using normalized name
        if self.tool_registry.get_tool(&normalized_tool_name).is_none() {
            return ActionResult::ToolError {
                error: format!("Tool not found: {}", tool_name),
                retryable: false,
            };
        }

        // Execute with timeout using normalized tool name
        let timeout = tokio::time::Duration::from_secs(self.config.timeout_seconds);
        let result = tokio::time::timeout(
            timeout,
            self.tool_registry
                .execute_tool(&normalized_tool_name, arguments.clone()),
        )
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        let action_result = match result {
            Ok(Ok(output)) => {
                info!(tool = tool_name, duration_ms, "Tool executed successfully");
                ActionResult::ToolSuccess { output, duration_ms }
            }
            Ok(Err(e)) => {
                error!(tool = tool_name, error = %e, "Tool execution failed");
                let retryable = matches!(
                    e,
                    AetherError::NetworkError { .. } | AetherError::Timeout { .. }
                );
                ActionResult::ToolError {
                    error: e.to_string(),
                    retryable,
                }
            }
            Err(_) => {
                error!(tool = tool_name, "Tool execution timed out");
                ActionResult::ToolError {
                    error: format!(
                        "Tool execution timed out after {}s",
                        self.config.timeout_seconds
                    ),
                    retryable: true,
                }
            }
        };

        // Store result in cache
        self.result_cache
            .store(&normalized_tool_name, &arguments, &action_result)
            .await;

        action_result
    }
}

#[async_trait]
impl<R: ToolRegistry + 'static> ActionExecutor for SingleStepExecutor<R> {
    async fn execute(&self, action: &Action) -> ActionResult {
        match action {
            Action::ToolCall {
                tool_name,
                arguments,
            } => self.execute_tool_call(tool_name, arguments.clone()).await,

            Action::UserInteraction { question, .. } => {
                // UserInteraction is handled by the callback system, not executor
                // The AgentLoop will use the callback to get user response
                // For now, we return a placeholder response indicating we need input
                // This should be intercepted by AgentLoop before reaching here
                ActionResult::UserResponse {
                    response: format!("Awaiting user response for: {}", question),
                }
            }

            Action::UserInteractionMultigroup { question, .. } => {
                // Multi-group user interaction is handled by the callback system
                // Similar to UserInteraction, this should be intercepted by AgentLoop
                ActionResult::UserResponse {
                    response: format!("Awaiting multi-group user response for: {}", question),
                }
            }

            Action::Completion { .. } => {
                // Completion is a terminal action
                ActionResult::Completed
            }

            Action::Failure { .. } => {
                // Failure is a terminal action
                ActionResult::Failed
            }
        }
    }
}

impl<R: ToolRegistry> SingleStepExecutor<R> {
    /// Check if an action requires user confirmation
    pub fn requires_confirmation(&self, action: &Action) -> bool {
        if let Action::ToolCall { tool_name, .. } = action {
            if let Some(tool) = self.tool_registry.get_tool(tool_name) {
                return tool.requires_confirmation;
            }
        }
        false
    }
}

/// Simple in-memory tool registry for testing
#[cfg(test)]
pub struct MockToolRegistry {
    tools: std::collections::HashMap<String, UnifiedTool>,
    results: std::sync::Mutex<std::collections::HashMap<String, Value>>,
}

#[cfg(test)]
impl MockToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: std::collections::HashMap::new(),
            results: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn add_tool(&mut self, tool: UnifiedTool) {
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn set_result(&self, tool_name: &str, result: Value) {
        self.results
            .lock()
            .unwrap()
            .insert(tool_name.to_string(), result);
    }
}

#[cfg(test)]
impl ToolRegistry for MockToolRegistry {
    fn get_tool(&self, name: &str) -> Option<&UnifiedTool> {
        self.tools.get(name)
    }

    fn execute_tool(
        &self,
        tool_name: &str,
        _arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send + '_>> {
        let result = self
            .results
            .lock()
            .unwrap()
            .get(tool_name)
            .cloned()
            .unwrap_or(serde_json::json!({"status": "ok"}));

        Box::pin(async move { Ok(result) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::ToolSource;
    use serde_json::json;

    fn create_test_tool(name: &str) -> UnifiedTool {
        UnifiedTool::new(
            format!("test:{}", name),
            name,
            format!("Test tool: {}", name),
            ToolSource::Builtin,
        )
    }

    #[tokio::test]
    async fn test_successful_tool_execution() {
        let mut registry = MockToolRegistry::new();
        registry.add_tool(create_test_tool("search"));
        registry.set_result("search", json!({"results": ["result1", "result2"]}));

        let executor = SingleStepExecutor::new(Arc::new(registry));

        let action = Action::ToolCall {
            tool_name: "search".to_string(),
            arguments: json!({"query": "test"}),
        };

        let result = executor.execute(&action).await;

        assert!(matches!(result, ActionResult::ToolSuccess { .. }));
        if let ActionResult::ToolSuccess { output, .. } = result {
            assert_eq!(output["results"], json!(["result1", "result2"]));
        }
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let registry = MockToolRegistry::new();
        let executor = SingleStepExecutor::new(Arc::new(registry));

        let action = Action::ToolCall {
            tool_name: "unknown".to_string(),
            arguments: json!({}),
        };

        let result = executor.execute(&action).await;

        assert!(matches!(result, ActionResult::ToolError { retryable: false, .. }));
    }

    #[tokio::test]
    async fn test_completion_action() {
        let registry = MockToolRegistry::new();
        let executor = SingleStepExecutor::new(Arc::new(registry));

        let action = Action::Completion {
            summary: "Task done".to_string(),
        };

        let result = executor.execute(&action).await;

        assert!(matches!(result, ActionResult::Completed));
    }

    #[tokio::test]
    async fn test_user_interaction_action() {
        let registry = MockToolRegistry::new();
        let executor = SingleStepExecutor::new(Arc::new(registry));

        let action = Action::UserInteraction {
            question: "Which option?".to_string(),
            options: Some(vec!["A".to_string(), "B".to_string()]),
        };

        let result = executor.execute(&action).await;

        assert!(matches!(result, ActionResult::UserResponse { .. }));
    }

    #[test]
    fn test_normalize_tool_name() {
        // Test colon separator (e.g., "file_ops:mkdir")
        assert_eq!(normalize_tool_name("file_ops:mkdir"), "file_ops");
        assert_eq!(normalize_tool_name("file_ops:write"), "file_ops");
        assert_eq!(normalize_tool_name("search:query"), "search");

        // Test multiple colons (e.g., "file_ops:mkdir:extra")
        assert_eq!(normalize_tool_name("file_ops:mkdir:extra"), "file_ops");

        // Test dot separator (e.g., "file_ops.write")
        assert_eq!(normalize_tool_name("file_ops.write"), "file_ops");

        // Test no separator - should return as-is
        assert_eq!(normalize_tool_name("file_ops"), "file_ops");
        assert_eq!(normalize_tool_name("search"), "search");
        assert_eq!(normalize_tool_name("generate_image"), "generate_image");
    }

    #[tokio::test]
    async fn test_tool_execution_with_suffixed_name() {
        // Test that "file_ops:mkdir" correctly executes the "file_ops" tool
        let mut registry = MockToolRegistry::new();
        registry.add_tool(create_test_tool("file_ops"));
        registry.set_result("file_ops", json!({"operation": "mkdir", "success": true}));

        let executor = SingleStepExecutor::new(Arc::new(registry));

        // LLM returns tool name with suffix "file_ops:mkdir"
        let action = Action::ToolCall {
            tool_name: "file_ops:mkdir".to_string(),
            arguments: json!({"operation": "mkdir", "path": "/tmp/test"}),
        };

        let result = executor.execute(&action).await;

        // Should succeed because "file_ops:mkdir" is normalized to "file_ops"
        assert!(matches!(result, ActionResult::ToolSuccess { .. }), "Expected ToolSuccess, got {:?}", result);
    }
}
