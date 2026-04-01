//! SubagentTool — delegates tasks to a temporary AgentLoop.
//!
//! When the parent agent needs to run a complex sub-task autonomously,
//! it calls the `subagent` tool. This creates a fresh `AgentLoop` with
//! its own tool registry (minus the subagent tool itself to prevent
//! infinite recursion) and runs the task to completion.
//!
//! Supports agent role selection via `agent_type`, optional context
//! injection via `context_summary`, and background execution via
//! `run_in_background`.

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::background_tracker::BackgroundAgentTracker;
use super::loop_core::{AgentLoop, LoopConfig, NoopCallback};
use super::prompt_builder::{PromptBuilder, PromptSection, Stability};
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::{LoopTool, LoopToolRegistry, ToolResult};
use crate::agents::{AgentDef, AgentRegistry};
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

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
struct SubagentArgs {
    task: String,
    agent_type: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    run_in_background: bool,
    context_summary: Option<String>,
}

/// A LoopTool that delegates tasks to a temporary AgentLoop.
pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: super::chain_context::ChainContext,
    agent_registry: Arc<AgentRegistry>,
    background_tracker: Arc<BackgroundAgentTracker>,
}

impl SubagentTool {
    /// Create a new SubagentTool.
    ///
    /// - `provider`: the AI provider for the sub-agent's LLM calls
    /// - `tool_registry_factory`: builds a fresh tool registry (without "subagent")
    /// - `safety_guard_factory`: builds a fresh SafetyGuard per invocation
    /// - `chain`: the parent's chain context for depth tracking
    /// - `agent_registry`: registry of available agent definitions
    /// - `background_tracker`: tracker for background sub-agent tasks
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        chain: super::chain_context::ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<BackgroundAgentTracker>,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            chain,
            agent_registry,
            background_tracker,
        }
    }
}

/// Parse the task and options from the input JSON.
fn parse_args(input: &Value) -> Result<SubagentArgs, String> {
    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "missing required field: task".to_string())?;

    if task.trim().is_empty() {
        return Err("task must not be empty".to_string());
    }

    let agent_type = input
        .get("agent_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let model = input
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let timeout_secs = input
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);

    let run_in_background = input
        .get("run_in_background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let context_summary = input
        .get("context_summary")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(SubagentArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
    })
}

/// Run a sub-agent to completion.
///
/// This is a module-level async function so it can be spawned in a
/// background tokio task (which requires `'static`).
async fn run_subagent(
    provider: Arc<dyn AiProvider>,
    agent_def: AgentDef,
    task: String,
    context_summary: Option<String>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    child_chain: super::chain_context::ChainContext,
    timeout_secs: u64,
) -> Result<super::loop_core::LoopRunResult, String> {
    let bridge = AiProviderBridge::new(provider);

    // Build tool registry, then filter to agent's allowed tools
    let mut registry = (tool_registry_factory)();
    registry.retain(|name| agent_def.is_tool_allowed(name));

    // Build prompt with agent's system prompt
    let mut prompt_builder = PromptBuilder::new()
        .with_identity(&agent_def.system_prompt)
        .with_default_behavior_sections();

    // Inject parent context if provided
    if let Some(summary) = context_summary {
        prompt_builder.register(PromptSection {
            name: "parent_context".to_string(),
            stability: Stability::Dynamic,
            priority: 500,
            protected: false,
            content: format!("## Context from parent agent\n\n{}", summary),
        });
    }

    // Build loop config from agent definition
    let config = LoopConfig {
        max_iterations: agent_def.max_iterations.unwrap_or(25) as usize,
        token_budget: agent_def.token_budget.unwrap_or(100_000) as usize,
    };

    // Create and run the agent loop
    let mut agent_loop = AgentLoop::new(
        bridge,
        registry,
        prompt_builder,
        (safety_guard_factory)(),
        config,
        CancellationToken::new(),
    )
    .with_chain(child_chain);

    let mut callback = NoopCallback;
    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    let run_result =
        tokio::time::timeout(timeout_duration, agent_loop.run(&task, &mut callback)).await;

    match run_result {
        Err(_elapsed) => Err(format!("Sub-agent timed out after {}s", timeout_secs)),
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(format!("sub-agent failed: {}", e)),
    }
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
                "agent_type": {
                    "type": "string",
                    "description": "The type of agent to use (e.g., 'explore', 'coder', 'researcher', 'plan', 'verify'). Defaults to 'default'."
                },
                "model": {
                    "type": "string",
                    "description": "Model hint for the sub-agent (e.g., 'fast', 'deep')."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Maximum time in seconds for the sub-agent to run. Default: 120.",
                    "default": 120
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "If true, run the sub-agent in the background and return immediately with a request_id.",
                    "default": false
                },
                "context_summary": {
                    "type": "string",
                    "description": "A summary of the parent agent's context to pass to the sub-agent."
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // 1. Parse arguments
        let args = match parse_args(&input) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::Error {
                    error: e,
                    retryable: false,
                }
            }
        };

        tracing::info!(
            task = %args.task,
            agent_type = ?args.agent_type,
            timeout_secs = args.timeout_secs,
            background = args.run_in_background,
            "subagent: starting sub-task"
        );

        // 2. Resolve agent definition
        let agent_def = if let Some(ref agent_type) = args.agent_type {
            match self.agent_registry.get(agent_type) {
                Some(def) => def,
                None => {
                    let available = self.agent_registry.list_ids().join(", ");
                    return ToolResult::Error {
                        error: format!(
                            "Unknown agent_type '{}'. Available agents: {}",
                            agent_type, available
                        ),
                        retryable: false,
                    };
                }
            }
        } else {
            match self.agent_registry.get("default") {
                Some(def) => def,
                None => {
                    return ToolResult::Error {
                        error: "No default agent registered in AgentRegistry".to_string(),
                        retryable: false,
                    };
                }
            }
        };

        // 3. Check nesting depth
        let child_chain = match self.chain.child() {
            Some(c) => c,
            None => {
                return ToolResult::Error {
                    error: format!(
                        "Maximum subagent nesting depth ({}) exceeded",
                        self.chain.max_depth
                    ),
                    retryable: false,
                };
            }
        };

        // 4. Foreground vs background execution
        if args.run_in_background {
            let request_id = uuid::Uuid::new_v4().to_string();
            let cancel_token = CancellationToken::new();

            self.background_tracker.register(
                request_id.clone(),
                cancel_token,
                args.task.clone(),
            );

            let provider = self.provider.clone();
            let factory = self.tool_registry_factory.clone();
            let safety_factory = self.safety_guard_factory.clone();
            let task = args.task.clone();
            let context_summary = args.context_summary;
            let timeout_secs = args.timeout_secs;
            let tracker = self.background_tracker.clone();
            let rid = request_id.clone();

            tokio::spawn(async move {
                let result = run_subagent(
                    provider,
                    agent_def,
                    task,
                    context_summary,
                    factory,
                    safety_factory,
                    child_chain,
                    timeout_secs,
                )
                .await;

                let outcome = match result {
                    Ok(r) => Ok(r.final_text.unwrap_or_else(|| "(no output)".to_string())),
                    Err(e) => Err(e),
                };
                tracker.mark_completed(&rid, outcome);
            });

            ToolResult::Success {
                output: json!({
                    "status": "running_in_background",
                    "request_id": request_id,
                    "message": format!("Sub-agent started in background. Use request_id '{}' to check status.", request_id)
                }),
            }
        } else {
            // Foreground execution
            match run_subagent(
                self.provider.clone(),
                agent_def,
                args.task.clone(),
                args.context_summary,
                self.tool_registry_factory.clone(),
                self.safety_guard_factory.clone(),
                child_chain,
                args.timeout_secs,
            )
            .await
            {
                Ok(result) => {
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
                Err(e) => {
                    tracing::warn!(error = %e, "subagent: sub-task failed");
                    ToolResult::Error {
                        error: e,
                        retryable: false,
                    }
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

    use crate::agents::AgentRegistry;
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
            Box::pin(async { Ok(ProviderResponse::text_only("mock response".to_string())) })
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    fn make_registry() -> Arc<AgentRegistry> {
        let registry = AgentRegistry::with_builtins();
        Arc::new(registry)
    }

    fn make_tracker() -> Arc<BackgroundAgentTracker> {
        Arc::new(BackgroundAgentTracker::new())
    }

    fn make_tool() -> SubagentTool {
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let factory: ToolRegistryFactory = Arc::new(|| LoopToolRegistry::new());
        let safety_factory: SafetyGuardFactory = Arc::new(|| SafetyGuard::default_guard());
        let chain = super::super::chain_context::ChainContext::new();
        SubagentTool::new(
            provider,
            factory,
            safety_factory,
            chain,
            make_registry(),
            make_tracker(),
        )
    }

    #[test]
    fn test_parse_args_basic() {
        let args = parse_args(&json!({ "task": "do something" })).unwrap();
        assert_eq!(args.task, "do something");
        assert!(args.agent_type.is_none());
        assert!(args.model.is_none());
        assert_eq!(args.timeout_secs, 120);
        assert!(!args.run_in_background);
        assert!(args.context_summary.is_none());
    }

    #[test]
    fn test_parse_args_full() {
        let args = parse_args(&json!({
            "task": "analyze code",
            "agent_type": "explore",
            "model": "fast",
            "timeout_secs": 60,
            "run_in_background": true,
            "context_summary": "We are working on a Rust project."
        }))
        .unwrap();

        assert_eq!(args.task, "analyze code");
        assert_eq!(args.agent_type.as_deref(), Some("explore"));
        assert_eq!(args.model.as_deref(), Some("fast"));
        assert_eq!(args.timeout_secs, 60);
        assert!(args.run_in_background);
        assert_eq!(
            args.context_summary.as_deref(),
            Some("We are working on a Rust project.")
        );
    }

    #[test]
    fn test_parse_args_empty_task() {
        let result = parse_args(&json!({ "task": "" }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));

        let result = parse_args(&json!({ "task": "   " }));
        assert!(result.is_err());

        let result = parse_args(&json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing required field"));
    }

    #[test]
    fn test_schema_includes_new_fields() {
        let tool = make_tool();
        let schema = tool.schema();

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["task"]));

        let props = &schema["properties"];
        assert!(props["task"].is_object());
        assert!(props["agent_type"].is_object());
        assert!(props["model"].is_object());
        assert!(props["timeout_secs"].is_object());
        assert!(props["run_in_background"].is_object());
        assert!(props["context_summary"].is_object());
    }

    #[tokio::test]
    async fn test_execute_with_agent_type() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "task": "explore the codebase",
                "agent_type": "explore"
            }))
            .await;

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

    #[tokio::test]
    async fn test_execute_unknown_agent_type() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "task": "do something",
                "agent_type": "nonexistent_agent"
            }))
            .await;

        match result {
            ToolResult::Error { error, retryable } => {
                assert!(error.contains("Unknown agent_type"));
                assert!(error.contains("nonexistent_agent"));
                assert!(!retryable);
            }
            _ => panic!("expected ToolResult::Error"),
        }
    }

    #[tokio::test]
    async fn test_execute_background() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "task": "background work",
                "run_in_background": true
            }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["status"], "running_in_background");
                assert!(output["request_id"].is_string());
                assert!(!output["request_id"].as_str().unwrap().is_empty());
                assert!(output["message"].is_string());
            }
            ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
            _ => panic!("expected ToolResult::Success"),
        }
    }

    #[tokio::test]
    async fn test_execute_missing_task() {
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
    async fn test_execute_success() {
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
