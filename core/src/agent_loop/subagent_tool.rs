//! SubagentTool — delegates tasks to a temporary AgentLoop.
//!
//! When the parent agent needs to run a complex sub-task autonomously,
//! it calls the `subagent` tool. This creates a fresh `AgentLoop` with
//! its own tool registry (minus the subagent tool itself to prevent
//! infinite recursion) and runs the task to completion.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::loop_core::{AgentLoop, LoopConfig, NoopCallback};
use super::prompt_builder::PromptBuilder;
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::{LoopToolRegistry, LoopTool, ToolResult};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Factory that builds a fresh LoopToolRegistry for the sub-agent.
///
/// The factory is responsible for providing the parent's tools minus
/// the "subagent" tool itself (to prevent infinite recursion).
/// Created at a higher layer where UnifiedTool/ToolRegistry are available.
pub type ToolRegistryFactory = Arc<dyn Fn() -> LoopToolRegistry + Send + Sync>;

/// Factory that builds a SafetyGuard for the sub-agent.
///
/// SafetyGuard is not Clone, so we use a factory to produce a fresh instance
/// each time a sub-agent is spawned.
pub type SafetyGuardFactory = Arc<dyn Fn() -> SafetyGuard + Send + Sync>;

const SUBAGENT_SYSTEM_PROMPT: &str = "\
You are a focused sub-agent executing a specific task delegated by a parent agent. \
Complete the task thoroughly and return a clear, concise result. \
Do not ask clarifying questions — work with what you have. \
If you cannot complete the task, explain exactly what blocked you.";

/// A LoopTool that delegates tasks to a temporary AgentLoop.
pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
}

impl SubagentTool {
    /// Create a new SubagentTool.
    ///
    /// - `provider`: the AI provider for the sub-agent's LLM calls
    /// - `tool_registry_factory`: builds a fresh tool registry (without "subagent")
    /// - `safety_guard_factory`: builds a fresh SafetyGuard per invocation
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
        }
    }
}

/// Parse the task and timeout from the input JSON.
fn parse_args(input: &Value) -> Result<(String, u64), String> {
    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "missing required field: task".to_string())?;

    if task.trim().is_empty() {
        return Err("task must not be empty".to_string());
    }

    let timeout_secs = input
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);

    Ok((task, timeout_secs))
}

#[async_trait]
impl LoopTool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Delegate a task to an autonomous sub-agent. The sub-agent runs independently \
         with its own tool access and returns the result when complete. Use this for \
         complex sub-tasks that require multiple steps."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "A clear description of the task for the sub-agent to complete."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum time in seconds for the sub-agent to run. Default: 120.",
                    "default": 120
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // 1. Parse arguments
        let (task, timeout_secs) = match parse_args(&input) {
            Ok(args) => args,
            Err(e) => {
                return ToolResult::Error {
                    error: e,
                    retryable: false,
                }
            }
        };

        tracing::info!(task = %task, timeout_secs = timeout_secs, "subagent: starting sub-task");

        // 2. Build sub-agent components
        let bridge = AiProviderBridge::new(self.provider.clone());
        let registry = (self.tool_registry_factory)();
        let prompt_builder = PromptBuilder::new().with_soul_identity(SUBAGENT_SYSTEM_PROMPT);
        let config = LoopConfig {
            max_iterations: 25,
            token_budget: 100_000,
        };

        // 3. Create and run the agent loop
        let agent_loop = AgentLoop::new(
            bridge,
            registry,
            prompt_builder,
            (self.safety_guard_factory)(),
            config,
        );

        let mut callback = NoopCallback;
        let timeout_duration = std::time::Duration::from_secs(timeout_secs);
        let run_result = tokio::time::timeout(
            timeout_duration,
            agent_loop.run(&task, &mut callback),
        ).await;

        match run_result {
            Err(_elapsed) => {
                tracing::warn!(task = %task, timeout_secs, "subagent: timed out");
                ToolResult::Error {
                    error: format!("Sub-agent timed out after {}s", timeout_secs),
                    retryable: false,
                }
            }
            Ok(Ok(result)) => {
                tracing::info!(
                    iterations = result.iterations,
                    tool_calls = result.tool_calls_made,
                    tokens = result.total_tokens,
                    "subagent: sub-task completed"
                );

                ToolResult::Success {
                    output: json!({
                        "result": result.final_text.unwrap_or_else(|| "(no output)".to_string()),
                        "iterations": result.iterations,
                        "tool_calls_made": result.tool_calls_made
                    }),
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "subagent: sub-task failed");

                ToolResult::Error {
                    error: format!("sub-agent failed: {}", e),
                    retryable: true,
                }
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::future::Future;
    use std::pin::Pin;

    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;

    /// Mock AI provider for unit tests.
    struct MockAiProvider;

    impl AiProvider for MockAiProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(ProviderResponse::text_only("mock response".to_string()))
            })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    fn make_tool() -> SubagentTool {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let factory: ToolRegistryFactory = Arc::new(|| LoopToolRegistry::new());
        let safety_factory: SafetyGuardFactory =
            Arc::new(|| SafetyGuard::default_guard());
        SubagentTool::new(provider, factory, safety_factory)
    }

    #[test]
    fn test_subagent_tool_schema() {
        let tool = make_tool();
        assert_eq!(tool.name(), "subagent");
        assert!(!tool.description().is_empty());

        let schema = tool.schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["task"]));
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["timeout_secs"].is_object());
    }

    #[test]
    fn test_subagent_args_parsing() {
        // Default timeout
        let (task, timeout) = parse_args(&json!({ "task": "do something" })).unwrap();
        assert_eq!(task, "do something");
        assert_eq!(timeout, 120);

        // Explicit timeout
        let (task, timeout) =
            parse_args(&json!({ "task": "do something", "timeout_secs": 60 })).unwrap();
        assert_eq!(task, "do something");
        assert_eq!(timeout, 60);
    }

    #[test]
    fn test_subagent_invalid_args() {
        // Missing task
        let result = parse_args(&json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing required field"));

        // Empty task
        let result = parse_args(&json!({ "task": "" }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));

        // Whitespace-only task
        let result = parse_args(&json!({ "task": "   " }));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_subagent_execute_missing_task() {
        let tool = make_tool();
        let result = tool.execute(json!({})).await;

        match result {
            ToolResult::Error { error, retryable } => {
                assert!(error.contains("missing required field"));
                assert!(!retryable);
            }
            _ => panic!("expected ToolResult::Error"),
        }
    }

    #[tokio::test]
    async fn test_subagent_execute_success() {
        let tool = make_tool();
        let result = tool.execute(json!({ "task": "say hello" })).await;

        match result {
            ToolResult::Success { output } => {
                assert!(output["result"].is_string());
                assert!(output["iterations"].is_number());
                assert!(output["tool_calls_made"].is_number());
            }
            ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
            _ => panic!("expected ToolResult::Success"),
        }
    }
}
