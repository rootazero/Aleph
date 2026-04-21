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
use futures::FutureExt;
use serde_json::{json, Value};
use std::panic::AssertUnwindSafe;
use tokio_util::sync::CancellationToken;

use crate::agents::background_tracker::BackgroundAgentTracker;
use crate::agent_loop::SharedSnapshot;
use crate::agents::runtime::{AgentRuntime, AgentRuntimeConfig, SafetyGuardFactory, ToolRegistryFactory};
use crate::agents::teammates::TeammateManager;
use crate::agents::AgentRegistry;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::teams::messages::inbox::Inbox;
use crate::teams::messages::router::{MessageRouter, SendRequest};
use crate::teams::messages::types::MessageType;
use crate::tools::runtime::{LoopTool, ToolResult};

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
    /// Send a message to a named teammate.
    SendMessage {
        to: String,
        text: String,
        team_name: String,
    },
    /// Read inbox messages.
    ReadInbox { team_name: String },
}

#[derive(Debug)]
struct RunArgs {
    task: String,
    agent_type: Option<String>,
    model: Option<String>,
    timeout_secs: u64,
    run_in_background: bool,
    context_summary: Option<String>,
    /// Optional name — makes the agent addressable.
    name: Option<String>,
    /// Optional team name — enables shared tasks and messages.
    team_name: Option<String>,
}

/// A LoopTool that delegates tasks to a temporary AgentLoop.
pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: crate::agent_loop::chain_context::ChainContext,
    agent_registry: Arc<AgentRegistry>,
    background_tracker: Arc<BackgroundAgentTracker>,
    /// Optional teammate manager for auto team creation/registration.
    teammate_manager: Option<Arc<TeammateManager>>,
    /// Optional message router for send_message actions.
    message_router: Option<Arc<MessageRouter>>,
    /// Optional inbox for read_inbox actions.
    inbox: Option<Arc<Inbox>>,
    /// Identifies the calling agent (default: "primary").
    parent_agent_id: String,
    /// Shared prompt snapshot for fork path. Read-only from SubagentTool's perspective.
    shared_snapshot: Option<SharedSnapshot>,
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
        chain: crate::agent_loop::chain_context::ChainContext,
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
            teammate_manager: None,
            message_router: None,
            inbox: None,
            parent_agent_id: "primary".to_string(),
            shared_snapshot: None,
        }
    }

    /// Set the teammate manager for auto team creation/registration.
    pub fn with_teammate_manager(mut self, mgr: Arc<TeammateManager>) -> Self {
        self.teammate_manager = Some(mgr);
        self
    }

    /// Set the message router for send_message actions.
    pub fn with_message_router(mut self, router: Arc<MessageRouter>) -> Self {
        self.message_router = Some(router);
        self
    }

    /// Set the inbox for read_inbox actions.
    pub fn with_inbox(mut self, inbox: Arc<Inbox>) -> Self {
        self.inbox = Some(inbox);
        self
    }

    /// Set the parent agent id (identifies the calling agent).
    pub fn with_parent_agent_id(mut self, id: impl Into<String>) -> Self {
        self.parent_agent_id = id.into();
        self
    }

    /// Set the shared prompt snapshot for the fork path.
    pub fn with_shared_snapshot(mut self, snapshot: SharedSnapshot) -> Self {
        self.shared_snapshot = Some(snapshot);
        self
    }

    /// Check whether the fork path should be used for this invocation.
    ///
    /// Fork is eligible when the caller did not override agent_type, model,
    /// or team_name AND a snapshot is available from the parent.
    fn should_fork(&self, args: &RunArgs) -> bool {
        args.agent_type.is_none()
            && args.model.is_none()
            && args.team_name.is_none()
            && self.read_snapshot().is_some()
    }

    /// Read the current prompt snapshot from the shared lock, if available.
    fn read_snapshot(&self) -> Option<crate::thinker::prompt_builder::PromptSnapshot> {
        self.shared_snapshot
            .as_ref()
            .and_then(|s| s.read().unwrap_or_else(|e| e.into_inner()).clone())
    }
}

/// Parse the input JSON into a SubagentAction.
fn parse_args(input: &Value) -> Result<SubagentAction, String> {
    // Determine action from explicit field, falling back to legacy heuristics.
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

    match action {
        "send_message" => {
            let to = input
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "send_message requires 'to' field".to_string())?;
            let text = input
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "send_message requires 'text' field".to_string())?;
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "send_message requires 'team_name' field".to_string())?;
            return Ok(SubagentAction::SendMessage {
                to: to.to_string(),
                text: text.to_string(),
                team_name: team_name.to_string(),
            });
        }
        "read_inbox" => {
            let team_name = input
                .get("team_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "read_inbox requires 'team_name' field".to_string())?;
            return Ok(SubagentAction::ReadInbox {
                team_name: team_name.to_string(),
            });
        }
        "check_status" => {
            let rid = input
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "check_status requires 'request_id' field".to_string())?;
            return Ok(SubagentAction::CheckStatus(rid.to_string()));
        }
        // "run" or "" (default) — fall through to legacy run/check_status logic
        "run" | "" => {}
        other => {
            return Err(format!("unknown action '{other}'. Expected one of: run, check_status, send_message, read_inbox"));
        }
    }

    // Legacy heuristic: request_id without task → check_status
    let request_id = input
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let task = input
        .get("task")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if let Some(rid) = request_id {
        if task.is_none() || task.as_ref().is_some_and(|t| t.trim().is_empty()) {
            return Ok(SubagentAction::CheckStatus(rid));
        }
    }

    // Run action — task is required
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

    let name = input
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let team_name = input
        .get("team_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Validate: team_name without name is an error
    if team_name.is_some() && name.is_none() {
        return Err("team_name requires 'name' to be set (agent must be addressable)".to_string());
    }

    // Named teammates always run in background — override explicitly at parse time
    let run_in_background = if name.is_some() {
        if !run_in_background {
            tracing::info!(
                "Named teammates always run in background — overriding run_in_background to true"
            );
        }
        true
    } else {
        run_in_background
    };

    Ok(SubagentAction::Run(RunArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
        name,
        team_name,
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
                "action": {
                    "type": "string",
                    "enum": ["run", "check_status", "send_message", "read_inbox"],
                    "description": "The action to perform. Defaults to 'run' (or 'check_status' if only request_id is provided)."
                },
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
                },
                "name": {
                    "type": "string",
                    "description": "Optional name for the sub-agent, making it addressable by teammates."
                },
                "team_name": {
                    "type": "string",
                    "description": "Optional team name. Enables shared tasks and inter-agent messaging. Requires 'name' to be set."
                },
                "to": {
                    "type": "string",
                    "description": "Target agent name for send_message action."
                },
                "text": {
                    "type": "string",
                    "description": "Message text for send_message action."
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

        // Handle non-run actions
        let args = match action {
            SubagentAction::SendMessage {
                to,
                text,
                team_name,
            } => {
                let router = match &self.message_router {
                    Some(r) => r.clone(),
                    None => {
                        return ToolResult::Error {
                            error: "send_message requires a message router (not configured)"
                                .to_string(),
                            retryable: false,
                        };
                    }
                };

                // Resolve team_name to team_id via teammate_manager
                let resolved_team_id = if let Some(ref mgr) = self.teammate_manager {
                    mgr.ensure_team(&team_name, &self.parent_agent_id)
                        .await
                        .unwrap_or_else(|_| team_name.clone())
                } else {
                    team_name.clone()
                };

                match router
                    .send(SendRequest {
                        team_id: resolved_team_id,
                        from_agent: self.parent_agent_id.clone(),
                        to: vec![to.clone()],
                        cc: vec![],
                        msg_type: MessageType::Message,
                        subject: format!("Message to {to}"),
                        content: text,
                        reply_to: None,
                        attachments: vec![],
                    })
                    .await
                {
                    Ok(sent) => {
                        return ToolResult::Success {
                            output: json!({
                                "status": "sent",
                                "message_id": sent.id,
                                "to": to,
                            }),
                        };
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to send message: {e}"),
                            retryable: false,
                        };
                    }
                }
            }
            SubagentAction::ReadInbox { team_name } => {
                let inbox = match &self.inbox {
                    Some(i) => i.clone(),
                    None => {
                        return ToolResult::Error {
                            error: "read_inbox requires an inbox (not configured)".to_string(),
                            retryable: false,
                        };
                    }
                };

                // Resolve team_name to team_id via teammate_manager
                let resolved_team_id = if let Some(ref mgr) = self.teammate_manager {
                    mgr.ensure_team(&team_name, &self.parent_agent_id)
                        .await
                        .unwrap_or_else(|_| team_name.clone())
                } else {
                    team_name.clone()
                };

                match inbox
                    .read(&self.parent_agent_id, &resolved_team_id, None, true)
                    .await
                {
                    Ok(messages) => {
                        let summaries: Vec<Value> = messages
                            .iter()
                            .map(|m| {
                                json!({
                                    "id": m.id,
                                    "from": m.from_agent,
                                    "subject": m.subject,
                                    "content": m.content,
                                    "type": m.msg_type.as_str(),
                                })
                            })
                            .collect();
                        return ToolResult::Success {
                            output: json!(summaries),
                        };
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to read inbox: {e}"),
                            retryable: false,
                        };
                    }
                }
            }
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

        // 4. Teammate registration (when name + team_name are both provided)
        if let (Some(ref name), Some(ref tname)) = (&args.name, &args.team_name) {
            if let Some(ref mgr) = self.teammate_manager {
                match mgr.ensure_team(tname, &self.parent_agent_id).await {
                    Ok(tid) => {
                        if let Err(e) = mgr.register_teammate(&tid, name, "worker").await {
                            return ToolResult::Error {
                                error: format!("Failed to register teammate '{}': {}", name, e),
                                retryable: true,
                            };
                        }
                    }
                    Err(e) => {
                        return ToolResult::Error {
                            error: format!("Failed to create team '{}': {}", tname, e),
                            retryable: false,
                        };
                    }
                }
            } else {
                tracing::warn!(
                    name = %name,
                    team = %tname,
                    "subagent: teammate_manager not configured, skipping team registration"
                );
            }
        }

        // 5. Foreground vs background execution
        if args.run_in_background {
            let request_id = uuid::Uuid::new_v4().to_string();
            let cancel_token = CancellationToken::new();

            self.background_tracker.register(
                request_id.clone(),
                cancel_token.clone(),
                args.task.clone(),
            );

            // Compute fork decision and clone snapshot BEFORE moving into spawn
            let should_fork_flag = self.should_fork(&args);
            let prompt_snapshot_clone = if should_fork_flag {
                self.read_snapshot()
            } else {
                None
            };

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
                let snapshot = if should_fork_flag {
                    prompt_snapshot_clone
                } else {
                    None
                };

                let runtime_config = AgentRuntimeConfig {
                    agent_def,
                    task,
                    context_summary,
                    model,
                    timeout_secs,
                    prompt_snapshot: snapshot,
                };

                let runtime =
                    AgentRuntime::new(provider, factory, safety_factory, child_chain, cancel_token);

                let result = AssertUnwindSafe(runtime.run(runtime_config))
                    .catch_unwind()
                    .await;

                let outcome = match result {
                    Ok(Ok(r)) => Ok(r.final_text.unwrap_or_else(|| "(no output)".to_string())),
                    Ok(Err(e)) => Err(e),
                    Err(_panic) => Err("Sub-agent panicked".to_string()),
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
            let snapshot = if self.should_fork(&args) {
                self.read_snapshot()
            } else {
                None
            };

            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task: args.task.clone(),
                context_summary: args.context_summary,
                model: args.model,
                timeout_secs: args.timeout_secs,
                prompt_snapshot: snapshot,
            };

            let runtime = AgentRuntime::new(
                self.provider.clone(),
                self.tool_registry_factory.clone(),
                self.safety_guard_factory.clone(),
                child_chain,
                CancellationToken::new(),
            );

            match runtime.run(runtime_config).await {
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

    use crate::tools::runtime::LoopToolRegistry;
    use crate::agents::AgentRegistry;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use crate::session::ingress_safety::SafetyGuard;

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
        let chain = crate::agent_loop::chain_context::ChainContext::new();
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
        assert!(props["action"].is_object());
        assert!(props["name"].is_object());
        assert!(props["team_name"].is_object());
        assert!(props["to"].is_object());
        assert!(props["text"].is_object());
    }

    #[test]
    fn test_parse_args_with_name_and_team() {
        let action = parse_args(&json!({
            "task": "build feature",
            "name": "builder-1",
            "team_name": "alpha"
        }))
        .unwrap();

        match action {
            SubagentAction::Run(args) => {
                assert_eq!(args.task, "build feature");
                assert_eq!(args.name.as_deref(), Some("builder-1"));
                assert_eq!(args.team_name.as_deref(), Some("alpha"));
            }
            _ => panic!("expected SubagentAction::Run"),
        }
    }

    #[test]
    fn test_parse_args_team_without_name_is_error() {
        let result = parse_args(&json!({
            "task": "build feature",
            "team_name": "alpha"
        }));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("team_name requires 'name'"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_args_send_message() {
        let action = parse_args(&json!({
            "action": "send_message",
            "to": "builder-1",
            "text": "please review the PR",
            "team_name": "alpha"
        }))
        .unwrap();

        match action {
            SubagentAction::SendMessage {
                to,
                text,
                team_name,
            } => {
                assert_eq!(to, "builder-1");
                assert_eq!(text, "please review the PR");
                assert_eq!(team_name, "alpha");
            }
            _ => panic!("expected SubagentAction::SendMessage"),
        }
    }

    #[test]
    fn test_parse_args_read_inbox() {
        let action = parse_args(&json!({
            "action": "read_inbox",
            "team_name": "alpha"
        }))
        .unwrap();

        match action {
            SubagentAction::ReadInbox { team_name } => {
                assert_eq!(team_name, "alpha");
            }
            _ => panic!("expected SubagentAction::ReadInbox"),
        }
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
        let chain = crate::agent_loop::chain_context::ChainContext::new();
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

    #[test]
    fn test_builder_methods() {
        let tool = make_tool();
        // Verify the builder methods compile and don't panic
        let _tool = tool.with_parent_agent_id("test-agent");
    }

    #[tokio::test]
    async fn test_send_message_without_router() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "action": "send_message",
                "to": "agent-b",
                "text": "hello",
                "team_name": "alpha"
            }))
            .await;
        match result {
            ToolResult::Error { error, .. } => {
                assert!(
                    error.contains("message router"),
                    "unexpected error: {error}"
                );
            }
            _ => panic!("expected error when message router not configured"),
        }
    }

    #[tokio::test]
    async fn test_read_inbox_without_inbox() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "action": "read_inbox",
                "team_name": "alpha"
            }))
            .await;
        match result {
            ToolResult::Error { error, .. } => {
                assert!(error.contains("inbox"), "unexpected error: {error}");
            }
            _ => panic!("expected error when inbox not configured"),
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
