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
pub use super::subagent_runner::{run_subagent, SafetyGuardFactory, ToolRegistryFactory};
use super::tool::{LoopTool, ToolResult};
use crate::agents::AgentRegistry;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
}

#[derive(Debug)]
struct RunArgs {
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

/// Parse the input JSON into a SubagentAction.
fn parse_args(input: &Value) -> Result<SubagentAction, String> {
    // Check for status-check mode first
    let request_id = input
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // If request_id is provided without task, this is a status check
    if let Some(rid) = request_id {
        if task.is_none() || task.as_ref().map_or(false, |t| t.trim().is_empty()) {
            return Ok(SubagentAction::CheckStatus(rid));
        }
    }

    // Otherwise, this is a run action — task is required
    let task = task.ok_or_else(|| {
        "missing required field: task (or provide request_id to check background status)"
            .to_string()
    })?;

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

    Ok(SubagentAction::Run(RunArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
    }))
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
                },
                "request_id": {
                    "type": "string",
                    "description": "Check status of a background sub-agent. Provide request_id without task to retrieve the result."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // 1. Parse arguments
        let action = match parse_args(&input) {
            Ok(a) => a,
            Err(e) => {
                return ToolResult::Error {
                    error: e,
                    retryable: false,
                }
            }
        };

        // Handle status check mode
        let args = match action {
            SubagentAction::CheckStatus(request_id) => {
                // Check running first
                let running = self.background_tracker.list_running();
                if running.iter().any(|(id, _, _)| id == &request_id) {
                    return ToolResult::Success {
                        output: json!({
                            "status": "running",
                            "request_id": request_id,
                        }),
                    };
                }
                // Check completed
                match self.background_tracker.take_result(&request_id) {
                    Some(Ok(result)) => {
                        return ToolResult::Success {
                            output: json!({
                                "status": "completed",
                                "request_id": request_id,
                                "result": result,
                            }),
                        };
                    }
                    Some(Err(err)) => {
                        return ToolResult::Error {
                            error: format!("Background sub-agent failed: {}", err),
                            retryable: false,
                        };
                    }
                    None => {
                        return ToolResult::Error {
                            error: format!(
                                "No background sub-agent found with request_id '{}'",
                                request_id
                            ),
                            retryable: false,
                        };
                    }
                }
            }
            SubagentAction::Run(run_args) => run_args,
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

            self.background_tracker
                .register(request_id.clone(), cancel_token, args.task.clone());

            let provider = self.provider.clone();
            let factory = self.tool_registry_factory.clone();
            let safety_factory = self.safety_guard_factory.clone();
            let task = args.task.clone();
            let context_summary = args.context_summary;
            let model = args.model.clone();
            let timeout_secs = args.timeout_secs;
            let tracker = self.background_tracker.clone();
            let rid = request_id.clone();

            tokio::spawn(async move {
                let result = run_subagent(
                    provider,
                    agent_def,
                    task,
                    context_summary,
                    model,
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
                args.model,
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

    use super::super::safety::SafetyGuard;
    use super::super::tool::LoopToolRegistry;
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
        let action = parse_args(&json!({ "task": "do something" })).unwrap();
        match action {
            SubagentAction::Run(args) => {
                assert_eq!(args.task, "do something");
                assert!(args.agent_type.is_none());
                assert!(args.model.is_none());
                assert_eq!(args.timeout_secs, 120);
                assert!(!args.run_in_background);
                assert!(args.context_summary.is_none());
            }
            _ => panic!("expected SubagentAction::Run"),
        }
    }

    #[test]
    fn test_parse_args_full() {
        let action = parse_args(&json!({
            "task": "analyze code",
            "agent_type": "explore",
            "model": "fast",
            "timeout_secs": 60,
            "run_in_background": true,
            "context_summary": "We are working on a Rust project."
        }))
        .unwrap();

        match action {
            SubagentAction::Run(args) => {
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
            _ => panic!("expected SubagentAction::Run"),
        }
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
    fn test_parse_args_check_status() {
        let action = parse_args(&json!({ "request_id": "abc-123" })).unwrap();
        match action {
            SubagentAction::CheckStatus(rid) => assert_eq!(rid, "abc-123"),
            _ => panic!("expected SubagentAction::CheckStatus"),
        }
    }

    #[test]
    fn test_parse_args_request_id_with_task_is_run() {
        // When both task and request_id are provided, it's a Run action
        let action = parse_args(&json!({ "task": "do work", "request_id": "abc" })).unwrap();
        match action {
            SubagentAction::Run(args) => assert_eq!(args.task, "do work"),
            _ => panic!("expected SubagentAction::Run when both task and request_id given"),
        }
    }

    #[test]
    fn test_schema_includes_new_fields() {
        let tool = make_tool();
        let schema = tool.schema();

        assert_eq!(schema["type"], "object");

        let props = &schema["properties"];
        assert!(props["task"].is_object());
        assert!(props["agent_type"].is_object());
        assert!(props["model"].is_object());
        assert!(props["timeout_secs"].is_object());
        assert!(props["run_in_background"].is_object());
        assert!(props["context_summary"].is_object());
        assert!(props["request_id"].is_object());
    }

    #[tokio::test]
    async fn test_check_status_not_found() {
        let tool = make_tool();
        let result = tool.execute(json!({ "request_id": "nonexistent" })).await;
        match result {
            ToolResult::Error { error, .. } => {
                assert!(error.contains("No background sub-agent found"));
            }
            _ => panic!("expected error for unknown request_id"),
        }
    }

    #[tokio::test]
    async fn test_check_status_completed() {
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.mark_completed("test-id", Ok("the result".to_string()));

        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let factory: ToolRegistryFactory = Arc::new(|| LoopToolRegistry::new());
        let safety_factory: SafetyGuardFactory = Arc::new(|| SafetyGuard::default_guard());
        let chain = super::super::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            factory,
            safety_factory,
            chain,
            make_registry(),
            tracker,
        );

        let result = tool.execute(json!({ "request_id": "test-id" })).await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["status"], "completed");
                assert_eq!(output["result"], "the result");
            }
            _ => panic!("expected success with completed status"),
        }
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
