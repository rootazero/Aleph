//! SubagentTool — delegates tasks to a temporary child harness.
//!
//! When the parent agent needs to run a complex sub-task autonomously,
//! it calls the `subagent` tool. `AgentRuntime::execute_via_harness` spawns a
//! fresh `AgentHarness` (via `subagent_spawner`) with its parent tool service
//! wrapped by `AllowlistToolService`. SubAgent-mode agents are denied
//! invocation of this tool via `AgentDef::is_tool_allowed` (recursion
//! guard); see `agents/types.rs` for the rule.
//!
//! Supports agent role selection via `agent_type`, optional context
//! injection via `context_summary`, and background execution via
//! `run_in_background`.

use futures::FutureExt;
use serde_json::Value;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use tokio_util::sync::CancellationToken;

use crate::agents::background_tracker::{BackgroundAgentTracker, CompletedOutcome};
use crate::agents::runtime::{AgentRuntime, AgentRuntimeConfig};
use crate::agents::teammates::TeammateManager;
use crate::agents::AgentDef;
use crate::agents::AgentRegistry;
use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionService;
use crate::sync_primitives::Arc;
use crate::teams::messages::inbox::Inbox;
use crate::teams::messages::router::MessageRouter;
#[cfg(test)]
use crate::tools::runtime::ToolResult;
use crate::tools::service::ToolService;

mod loop_tool;

/// A2 — default cap on concurrently-running subagent spawns per top-level
/// agent run. Matches the deleted `Lane::Subagent` default.
const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;

/// Parsed arguments for the subagent tool.
#[derive(Debug)]
pub(super) enum SubagentAction {
    /// Run a new sub-agent task.
    Run(RunArgs),
    /// Check status of a background sub-agent.
    CheckStatus(String),
    /// Cancel a still-running background sub-agent by request_id. Hits the
    /// shared `CancellationToken`; the running task observes it on its next
    /// await point and unwinds cleanly. Idempotent — cancelling an unknown
    /// or already-completed request returns an error rather than panicking.
    Cancel(String),
    /// Send a message to a named teammate.
    SendMessage {
        to: String,
        text: String,
        team_name: String,
    },
    /// Read inbox messages.
    ReadInbox { team_name: String },
    /// Enumerate every background sub-agent (running + recently-completed)
    /// so the parent can recover request_ids it no longer holds.
    List,
}

/// A single task within a batch execution.
#[derive(Debug)]
pub(super) struct BatchTask {
    task: String,
    agent_type: Option<String>,
    model: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Debug)]
pub(super) struct RunArgs {
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
    /// Batch tasks for parallel execution. When provided, all tasks run in
    /// background automatically and a list of request_ids is returned.
    batch_tasks: Option<Vec<BatchTask>>,
}

/// A LoopTool that delegates tasks to a temporary AgentLoop.
pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    chain: crate::harness::chain_context::ChainContext,
    agent_registry: Arc<AgentRegistry>,
    background_tracker: Arc<BackgroundAgentTracker>,
    /// Shared session actor threaded to child `AgentRuntime` instances.
    session: Arc<dyn SessionService>,
    /// Parent tool service; the harness decorates it with an allowlist.
    parent_tools: Arc<dyn ToolService>,
    /// Shared sandbox passed to child harnesses.
    sandbox: Arc<dyn Sandbox>,
    /// Optional teammate manager for auto team creation/registration.
    teammate_manager: Option<Arc<TeammateManager>>,
    /// Optional message router for send_message actions.
    message_router: Option<Arc<MessageRouter>>,
    /// Optional inbox for read_inbox actions.
    inbox: Option<Arc<Inbox>>,
    /// Identifies the calling agent (default: "primary").
    parent_agent_id: String,
    /// Spec 1 G2 — threaded into child `AgentRuntime`s so the spawner emits
    /// `RawMemory(Delegation)` after each successful local subagent run.
    raw_memory_writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
    /// Optional capture-filter registry threaded with the writer.
    capture_registry: Option<Arc<crate::memory::extensions::MemoryExtensionRegistry>>,
    /// Parent session id stamped onto emitted Delegation rows. `None` leaves
    /// the row untagged for session-level lookups.
    parent_session_id: Option<String>,
    /// Stage F (P2) — parent trace sink threaded into background subagent
    /// runtimes wrapped by ForwardingTraceSink for progress observation.
    /// Sync subagents do NOT receive this wrapper (Stage A inheritance suffices).
    trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
    /// A2 — shared concurrency cap; one per tool instance (= per agent run).
    subagent_semaphore: Arc<tokio::sync::Semaphore>,
    /// A3 — parent run's cancellation token. Each spawn path derives a
    /// `child_token()` so a cancelled parent stops its subagents.
    parent_cancel: Option<CancellationToken>,
    /// B2 — global plugin registry, threaded into each AgentRuntime.
    plugin_registry: Option<Arc<crate::extension::registry::PluginRegistry>>,
    /// B3 — stall watchdog config inherited by subagents.
    stall_config: Option<crate::harness::StallConfig>,
    /// B3 — consecutive-failure cap inherited by subagents.
    consecutive_failure_cap: Option<usize>,
    /// B3 — per-turn wall-clock timeout inherited by subagents.
    turn_timeout: Option<std::time::Duration>,
    /// Phase 3 — `provider_hint` → pinned-then-fall-through provider. An empty
    /// map (the `new()` default) means every subagent uses `provider`.
    provider_overrides: HashMap<String, Arc<dyn AiProvider>>,
}

impl SubagentTool {
    /// Create a new SubagentTool.
    ///
    /// - `provider`: the AI provider for the sub-agent's LLM calls
    /// - `chain`: the parent's chain context for depth tracking
    /// - `agent_registry`: registry of available agent definitions
    /// - `background_tracker`: tracker for background sub-agent tasks
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn AiProvider>,
        chain: crate::harness::chain_context::ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<BackgroundAgentTracker>,
        session: Arc<dyn SessionService>,
        parent_tools: Arc<dyn ToolService>,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self {
        Self {
            provider,
            chain,
            agent_registry,
            background_tracker,
            session,
            parent_tools,
            sandbox,
            teammate_manager: None,
            message_router: None,
            inbox: None,
            parent_agent_id: "primary".to_string(),
            raw_memory_writer: None,
            capture_registry: None,
            parent_session_id: None,
            trace_sink: None,
            subagent_semaphore: Arc::new(tokio::sync::Semaphore::new(
                DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            )),
            parent_cancel: None,
            plugin_registry: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            provider_overrides: HashMap::new(),
        }
    }

    /// Phase 3 — wire the per-`provider_hint` override registry. A subagent
    /// whose `AgentDef.provider_hint` matches a key runs on that provider.
    pub fn with_provider_overrides(
        mut self,
        overrides: HashMap<String, Arc<dyn AiProvider>>,
    ) -> Self {
        self.provider_overrides = overrides;
        self
    }

    /// B2 — wire the global plugin registry for per-agent MCP scope.
    pub fn with_plugin_registry(
        mut self,
        registry: Arc<crate::extension::registry::PluginRegistry>,
    ) -> Self {
        self.plugin_registry = Some(registry);
        self
    }

    /// B3 — wire the stall watchdog config inherited by subagents.
    pub fn with_stall_config(mut self, config: crate::harness::StallConfig) -> Self {
        self.stall_config = Some(config);
        self
    }

    /// B3 — wire the consecutive-failure cap inherited by subagents.
    pub fn with_consecutive_failure_cap(mut self, cap: usize) -> Self {
        self.consecutive_failure_cap = Some(cap);
        self
    }

    /// B3 — wire the per-turn wall-clock timeout inherited by subagents.
    pub fn with_turn_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.turn_timeout = Some(timeout);
        self
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

    /// Spec 1 G2 — wire the raw-memory writer for delegation hook emit.
    pub fn with_raw_memory_writer(
        mut self,
        writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
    ) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Spec 1 G2 — wire an optional capture-filter registry alongside the writer.
    pub fn with_capture_registry(
        mut self,
        registry: Arc<crate::memory::extensions::MemoryExtensionRegistry>,
    ) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Spec 1 G2 — set the parent session id stamped onto Delegation rows.
    pub fn with_parent_session_id(mut self, sid: impl Into<String>) -> Self {
        self.parent_session_id = Some(sid.into());
        self
    }

    /// Stage F (P2) — thread the parent trace sink so background subagents can
    /// be observed via ForwardingTraceSink. Only wired on the background path.
    pub fn with_trace_sink(mut self, sink: Arc<dyn crate::harness::TraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    /// A3 — wire the parent run's cancellation token so spawned subagents
    /// stop when the parent is cancelled.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.parent_cancel = Some(token);
        self
    }

    /// A3 — a fresh child token derived from the parent run's token (cancelled
    /// when the parent is). Falls back to a standalone token for tests / direct
    /// callers with no parent token wired.
    fn cancel_for_child(&self) -> CancellationToken {
        self.parent_cancel
            .as_ref()
            .map(|t| t.child_token())
            .unwrap_or_default()
    }

    fn spawn_background(
        &self,
        agent_def: AgentDef,
        task: String,
        context_summary: Option<String>,
        model: Option<String>,
        timeout_secs: u64,
        child_chain: crate::harness::chain_context::ChainContext,
    ) -> String {
        let request_id = uuid::Uuid::new_v4().to_string();
        let cancel_token = self.cancel_for_child();

        self.background_tracker
            .register(request_id.clone(), cancel_token.clone(), task.clone());

        let mut runtime = self.build_runtime(child_chain, cancel_token);
        if let Some(parent_sink) = self.trace_sink.clone() {
            let wrapper: Arc<dyn crate::harness::TraceSink> = Arc::new(
                crate::agents::forwarding_trace_sink::ForwardingTraceSink::new(
                    parent_sink,
                    self.background_tracker.clone(),
                    request_id.clone(),
                ),
            );
            runtime = runtime.with_trace_sink(wrapper);
        }

        let tracker = self.background_tracker.clone();
        let rid = request_id.clone();
        tokio::spawn(async move {
            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task,
                context_summary,
                model,
                timeout_secs,
            };
            let result = AssertUnwindSafe(runtime.run(runtime_config))
                .catch_unwind()
                .await;
            let outcome = match result {
                Ok(Ok(r)) => CompletedOutcome::Ok {
                    final_text: r.final_text.unwrap_or_else(|| "(no output)".to_string()),
                    iterations: r.iterations,
                    tool_calls_made: r.tool_calls_made,
                    total_tokens: r.total_tokens,
                },
                Ok(Err(e)) => CompletedOutcome::Err(e),
                Err(_panic) => CompletedOutcome::Err("Sub-agent panicked".to_string()),
            };
            tracker.mark_completed(&rid, outcome);
        });

        request_id
    }

    /// Build an `AgentRuntime` with every inheritable field this tool carries
    /// applied. Single construction point for the foreground, sync-batch, and
    /// background spawn paths so new wiring lands in one place.
    fn build_runtime(
        &self,
        child_chain: crate::harness::chain_context::ChainContext,
        cancel: CancellationToken,
    ) -> AgentRuntime {
        let mut runtime = AgentRuntime::new(
            self.provider.clone(),
            child_chain,
            cancel,
            self.session.clone(),
            self.parent_tools.clone(),
            self.sandbox.clone(),
        )
        .with_parent_agent_id(self.parent_agent_id.clone());
        if let Some(w) = self.raw_memory_writer.clone() {
            runtime = runtime.with_raw_memory_writer(w);
        }
        if let Some(reg) = self.capture_registry.clone() {
            runtime = runtime.with_capture_registry(reg);
        }
        if let Some(sid) = self.parent_session_id.clone() {
            runtime = runtime.with_parent_session_id(sid);
        }
        runtime = runtime.with_subagent_semaphore(self.subagent_semaphore.clone());
        if let Some(reg) = self.plugin_registry.clone() {
            runtime = runtime.with_plugin_registry(reg);
        }
        if let Some(sink) = self.trace_sink.clone() {
            runtime = runtime.with_trace_sink(sink);
        }
        if let Some(sc) = self.stall_config.clone() {
            runtime = runtime.with_stall_config(sc);
        }
        if let Some(cap) = self.consecutive_failure_cap {
            runtime = runtime.with_consecutive_failure_cap(cap);
        }
        if let Some(tt) = self.turn_timeout {
            runtime = runtime.with_turn_timeout(tt);
        }
        if !self.provider_overrides.is_empty() {
            runtime = runtime.with_provider_overrides(self.provider_overrides.clone());
        }
        runtime
    }
}

/// Parse the input JSON into a SubagentAction.
pub(super) fn parse_args(input: &Value) -> Result<SubagentAction, String> {
    // Determine action from explicit field, falling back to legacy heuristics.
    let action = match input.get("action") {
        Some(v) => match v.as_str() {
            Some(s) => s,
            None => return Err("'action' must be a string".to_string()),
        },
        None => "",
    };

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
        "cancel" => {
            let rid = input
                .get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "cancel requires 'request_id' field".to_string())?;
            return Ok(SubagentAction::Cancel(rid.to_string()));
        }
        "list" => {
            return Ok(SubagentAction::List);
        }
        // "run" or "" (default) — fall through to legacy run/check_status logic
        "run" | "" => {}
        other => {
            return Err(format!("unknown action '{other}'. Expected one of: run, check_status, cancel, list, send_message, read_inbox"));
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

    // Parse batch_tasks early — when present, top-level `task` is optional
    // since each sub-task carries its own.
    let batch_tasks = input
        .get("batch_tasks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let task = item.get("task")?.as_str()?.to_string();
                    if task.trim().is_empty() {
                        return None;
                    }
                    Some(BatchTask {
                        task,
                        agent_type: item
                            .get("agent_type")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        model: item
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        timeout_secs: item.get("timeout_secs").and_then(|v| v.as_u64()),
                    })
                })
                .collect::<Vec<_>>()
        });
    let has_batch = batch_tasks.as_ref().map(|v| !v.is_empty()).unwrap_or(false);

    // Run action — top-level `task` is required UNLESS batch_tasks supplies
    // the actual sub-task descriptions.
    let task =
        match task {
            Some(t) if !t.trim().is_empty() => t,
            Some(_) if has_batch => String::new(),
            Some(_) => return Err("task must not be empty".to_string()),
            None if has_batch => String::new(),
            None => return Err(
                "missing required field: task (or provide request_id to check background status)"
                    .to_string(),
            ),
        };

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

    // batch_tasks honors `run_in_background` exactly as the user provides it:
    // - false (default / explicit): run all sub-tasks in parallel, await all,
    //   return aggregated results. This matches the natural Think→Act loop
    //   expectation that a tool call returns its result.
    // - true: fire-and-forget — spawn all sub-tasks in background and return
    //   a list of request_ids. The caller is then responsible for polling
    //   `check_status` on each one. (Useful for very long-running batches.)

    Ok(SubagentAction::Run(RunArgs {
        task,
        agent_type,
        model,
        timeout_secs,
        run_in_background,
        context_summary,
        name,
        team_name,
        batch_tasks,
    }))
}
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
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::tools::runtime::LoopTool;
    use crate::tools::service::{ToolDefinition, ToolError, ToolService};

    /// Noop tool service stub — never resolves any tool. Used by test helpers
    /// that need a `parent_tools` dep but don't exercise tool execution.
    struct NoopTestToolService;

    #[async_trait::async_trait]
    impl ToolService for NoopTestToolService {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, ToolError> {
            Err(ToolError::NotFound {
                name: "test".into(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            vec![]
        }
        async fn describe(&self, _: &str) -> Option<ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    fn in_mem_session() -> Arc<dyn crate::session::service::SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

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
        let chain = crate::harness::chain_context::ChainContext::new();
        SubagentTool::new(
            provider,
            chain,
            make_registry(),
            make_tracker(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
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
        tracker.mark_completed("test-id", CompletedOutcome::ok_text("the result"));

        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            tracker,
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
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

    #[test]
    fn with_plugin_registry_builder_smoke() {
        use crate::extension::registry::PluginRegistry;
        let tool = make_tool().with_plugin_registry(Arc::new(PluginRegistry::new()));
        let _ = tool;
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

    /// Stage F — check_status returns a `progress` array for running agents.
    #[tokio::test]
    async fn check_status_returns_progress_array_when_running() {
        use crate::agents::progress::{ProgressKind, SubagentProgress};
        use std::time::SystemTime;
        use tokio_util::sync::CancellationToken;

        let tracker = make_tracker();
        let token = CancellationToken::new();
        tracker.register("rid".into(), token, "task".into());
        tracker.push_progress(
            "rid",
            SubagentProgress {
                step: 1,
                timestamp: SystemTime::now(),
                kind: ProgressKind::ToolCalled,
                tool_name: Some("read_file".into()),
                latency_ms: None,
                preview: None,
            },
        );

        // Build a tool wired to the same tracker.
        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            tracker,
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        );

        let result = tool
            .execute(serde_json::json!({"action": "check_status", "request_id": "rid"}))
            .await;
        let output = match result {
            crate::tools::runtime::ToolResult::Success { output } => output,
            other => panic!("expected Success, got {other:?}"),
        };
        let progress = output.get("progress").expect("progress field present");
        assert!(progress.is_array());
        assert_eq!(progress.as_array().unwrap().len(), 1);
    }

    // -------------------------------------------------------------------------
    // batch_tasks: parse + execute (sync vs background)
    // -------------------------------------------------------------------------

    /// `batch_tasks` must NOT silently force run_in_background=true.
    /// Regression for an earlier behavior where batches always ran async,
    /// causing the parent LLM to receive request_ids it didn't poll and
    /// hallucinate sub-task results.
    #[test]
    fn batch_tasks_default_keeps_foreground() {
        let action = parse_args(&json!({
            "batch_tasks": [{"task": "a"}, {"task": "b"}]
        }))
        .unwrap();
        match action {
            SubagentAction::Run(args) => {
                assert!(
                    !args.run_in_background,
                    "batch_tasks must respect default run_in_background=false"
                );
                assert_eq!(args.batch_tasks.as_ref().unwrap().len(), 2);
            }
            _ => panic!("expected SubagentAction::Run"),
        }
    }

    /// Explicit `run_in_background=true` is preserved alongside batch_tasks.
    #[test]
    fn batch_tasks_explicit_background_preserved() {
        let action = parse_args(&json!({
            "batch_tasks": [{"task": "a"}],
            "run_in_background": true
        }))
        .unwrap();
        match action {
            SubagentAction::Run(args) => assert!(args.run_in_background),
            _ => panic!("expected SubagentAction::Run"),
        }
    }

    /// Sync batch path: tasks fan out in parallel, all complete, response
    /// carries an aggregated `results` array (no request_ids, no polling).
    #[tokio::test]
    async fn execute_batch_sync_returns_aggregated_results() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "batch_tasks": [
                    {"task": "first task", "agent_type": "explore"},
                    {"task": "second task", "agent_type": "explore"}
                ]
            }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["status"], "batch_completed");
                assert_eq!(output["count"], 2);
                let results = output["results"].as_array().expect("results is array");
                assert_eq!(results.len(), 2);
                for (i, r) in results.iter().enumerate() {
                    assert_eq!(r["status"], "completed", "task {i} should be completed");
                    assert_eq!(r["index"], i);
                    assert!(r["result"].is_string(), "task {i} result must be string");
                }
            }
            ToolResult::Error { error, .. } => panic!("expected success, got error: {error}"),
            _ => panic!("expected ToolResult::Success"),
        }
    }

    /// Provider whose `process` never resolves. Only the harness's cancellation
    /// race (fed by the request's CancellationToken) can end the turn — so this
    /// proves the *parent-derived* token actually reaches the child harness.
    struct PendingProvider;
    impl AiProvider for PendingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async move {
                std::future::pending::<()>().await;
                unreachable!()
            })
        }
        fn name(&self) -> &str {
            "pending"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    #[tokio::test]
    async fn foreground_subagent_cancels_on_parent_token() {
        let parent = CancellationToken::new();
        let provider: Arc<dyn AiProvider> = Arc::new(PendingProvider);
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            make_tracker(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_cancel_token(parent.clone());

        let handle = tokio::spawn(async move {
            tool.execute(serde_json::json!({ "task": "hang", "timeout_secs": 30 }))
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        parent.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
            .await
            .expect("foreground subagent did not honor parent cancel within 3s")
            .expect("task join");
        assert!(
            matches!(result, ToolResult::Error { .. }),
            "cancelled subagent must surface an error"
        );
    }

    struct UsageMockProvider;
    impl AiProvider for UsageMockProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(ProviderResponse {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: crate::providers::adapter::StopReason::EndTurn,
                    usage: Some(crate::providers::adapter::TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: None,
                        cache_creation_tokens: None,
                        thinking_tokens: None,
                        cost: None,
                    }),
                })
            })
        }
        fn name(&self) -> &str {
            "usage-mock"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    struct CapturingSink(std::sync::Mutex<Vec<crate::harness::trace::LoopTraceEvent>>);
    impl crate::harness::TraceSink for CapturingSink {
        fn on_trace(&self, e: &crate::harness::trace::LoopTraceEvent) {
            self.0.lock().unwrap().push(e.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn foreground_subagent_inherits_trace_sink() {
        let sink = Arc::new(CapturingSink(std::sync::Mutex::new(vec![])));
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            Arc::new(UsageMockProvider),
            chain,
            make_registry(),
            make_tracker(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_trace_sink(sink.clone() as Arc<dyn crate::harness::TraceSink>);

        let _ = tool.execute(serde_json::json!({ "task": "hi" })).await;

        let events = sink.0.lock().unwrap();
        assert!(
            !events.is_empty(),
            "subagent run must emit trace events into the inherited sink"
        );
    }

    #[tokio::test]
    async fn background_subagent_forwards_trace_to_parent_sink() {
        let sink = Arc::new(CapturingSink(std::sync::Mutex::new(vec![])));
        let chain = crate::harness::chain_context::ChainContext::new();
        let tracker = make_tracker();
        let tool = SubagentTool::new(
            Arc::new(UsageMockProvider),
            chain,
            make_registry(),
            tracker.clone(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_trace_sink(sink.clone() as Arc<dyn crate::harness::TraceSink>);

        let out = tool
            .execute(serde_json::json!({ "task": "bg", "run_in_background": true }))
            .await;
        let rid = match out {
            ToolResult::Success { output } => output["request_id"].as_str().unwrap().to_string(),
            other => panic!("expected background success, got {other:?}"),
        };

        // Poll until the background task completes (bounded).
        for _ in 0..100 {
            if tracker.list_running().iter().all(|(id, _, _)| id != &rid) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let events = sink.0.lock().unwrap();
        assert!(
            !events.is_empty(),
            "background subagent must forward trace events to the parent sink \
             via ForwardingTraceSink"
        );
    }

    /// Async batch path: explicit run_in_background=true returns request_ids
    /// without awaiting sub-task completion.
    #[tokio::test]
    async fn execute_batch_background_returns_request_ids() {
        let tool = make_tool();
        let result = tool
            .execute(json!({
                "batch_tasks": [{"task": "x"}, {"task": "y"}],
                "run_in_background": true
            }))
            .await;

        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["status"], "batch_running_in_background");
                assert_eq!(output["count"], 2);
                let ids = output["request_ids"]
                    .as_array()
                    .expect("request_ids is array");
                assert_eq!(ids.len(), 2);
                for id in ids {
                    let s = id.as_str().expect("request_id is string");
                    assert!(!s.is_empty());
                }
            }
            ToolResult::Error { error, .. } => panic!("expected success, got error: {error}"),
            _ => panic!("expected ToolResult::Success"),
        }
    }

    // -------------------------------------------------------------------------
    // list action + non-destructive completed reads
    // -------------------------------------------------------------------------

    #[test]
    fn parse_args_list_action() {
        let action = parse_args(&json!({ "action": "list" })).unwrap();
        assert!(
            matches!(action, SubagentAction::List),
            "action=list must parse to SubagentAction::List"
        );
    }

    /// `list` enumerates running and completed background sub-agents so the
    /// parent can recover request_ids it no longer holds.
    #[tokio::test]
    async fn execute_list_enumerates_background_agents() {
        let tracker = make_tracker();
        let token = CancellationToken::new();
        tracker.register("run-1".into(), token, "still going".into());
        tracker.mark_completed("done-1", CompletedOutcome::ok_text("finished"));

        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            tracker,
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        );

        let result = tool.execute(json!({ "action": "list" })).await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["running_count"], 1);
                assert_eq!(output["completed_count"], 1);
                assert_eq!(output["running"][0]["request_id"], "run-1");
                assert_eq!(output["completed"][0]["request_id"], "done-1");
                assert_eq!(output["completed"][0]["status"], "completed");
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    /// Re-checking a completed background sub-agent must keep returning the
    /// result — the read is non-destructive (regression: `take_result` used
    /// to consume the entry, so the second poll said "not found").
    #[tokio::test]
    async fn check_status_completed_is_repeatable() {
        let tracker = make_tracker();
        tracker.mark_completed("rid", CompletedOutcome::ok_text("the answer"));

        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            tracker,
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        );

        for poll in 1..=2 {
            let result = tool
                .execute(json!({ "action": "check_status", "request_id": "rid" }))
                .await;
            match result {
                ToolResult::Success { output } => {
                    assert_eq!(output["status"], "completed", "poll {poll}");
                    assert_eq!(output["result"], "the answer", "poll {poll}");
                }
                other => panic!("poll {poll}: expected Success, got {other:?}"),
            }
        }
    }

    /// Completed-agent `check_status` reports the same run metrics the
    /// foreground spawn path returns (parity — background no longer drops
    /// `iterations` / `tool_calls_made` / `total_tokens`).
    #[tokio::test]
    async fn check_status_completed_reports_run_metrics() {
        let tracker = make_tracker();
        tracker.mark_completed(
            "rid",
            CompletedOutcome::Ok {
                final_text: "done".into(),
                iterations: 4,
                tool_calls_made: 9,
                total_tokens: 555,
            },
        );

        let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            tracker,
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        );

        let result = tool
            .execute(json!({ "action": "check_status", "request_id": "rid" }))
            .await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["iterations"], 4);
                assert_eq!(output["tool_calls_made"], 9);
                assert_eq!(output["total_tokens"], 555);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }
}
