use super::parse::parse_args;
use super::types::SubagentAction;
use super::*;
use crate::agents::background_tracker::CompletedOutcome;
use crate::agents::AgentRegistry;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::session::in_process::InProcessActorSessionService;
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use crate::tools::runtime::{LoopTool, ToolResult};
use crate::tools::service::{ToolDefinition, ToolError, ToolService};
use serde_json::json;
use std::future::Future;
use std::pin::Pin;

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
    fn metadata_schema(&self) -> std::sync::Arc<[crate::tool_metadata::ToolDefinition]> {
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
    ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>> {
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
    let result = tool
        .execute(
            json!({ "request_id": "nonexistent" }),
            CancellationToken::new(),
        )
        .await;
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

    let result = tool
        .execute(json!({ "request_id": "test-id" }), CancellationToken::new())
        .await;
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
        .execute(
            json!({
                "task": "explore the codebase",
                "agent_type": "explore"
            }),
            CancellationToken::new(),
        )
        .await;

    match result {
        ToolResult::Success { output } => {
            assert!(output["result"].is_string());
            assert!(output["iterations"].is_number());
            assert!(output["tool_calls_made"].is_number());
        }
        ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
    }
}

#[tokio::test]
async fn test_execute_with_aliased_agent_type() {
    // A model emitting Claude Code vocabulary ("Explore" capitalized) must
    // resolve to the builtin `explore` agent instead of hard-erroring.
    let tool = make_tool();
    let result = tool
        .execute(
            json!({
                "task": "explore the codebase",
                "agent_type": "Explore"
            }),
            CancellationToken::new(),
        )
        .await;

    match result {
        ToolResult::Success { output } => {
            assert!(output["result"].is_string());
        }
        ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
    }
}

#[tokio::test]
async fn test_execute_unknown_agent_type() {
    let tool = make_tool();
    let result = tool
        .execute(
            json!({
                "task": "do something",
                "agent_type": "nonexistent_agent"
            }),
            CancellationToken::new(),
        )
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
        .execute(
            json!({
                "task": "background work",
                "run_in_background": true
            }),
            CancellationToken::new(),
        )
        .await;

    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "running_in_background");
            assert!(output["request_id"].is_string());
            assert!(!output["request_id"].as_str().unwrap().is_empty());
            assert!(output["message"].is_string());
        }
        ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
    }
}

#[tokio::test]
async fn test_execute_missing_task() {
    let tool = make_tool();
    let result = tool.execute(json!({}), CancellationToken::new()).await;

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
        .execute(
            json!({
                "action": "send_message",
                "to": "agent-b",
                "text": "hello",
                "team_name": "alpha"
            }),
            CancellationToken::new(),
        )
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
        .execute(
            json!({
                "action": "read_inbox",
                "team_name": "alpha"
            }),
            CancellationToken::new(),
        )
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
    let result = tool
        .execute(json!({ "task": "say hello" }), CancellationToken::new())
        .await;

    match result {
        ToolResult::Success { output } => {
            assert!(output["result"].is_string());
            assert!(output["iterations"].is_number());
            assert!(output["tool_calls_made"].is_number());
        }
        ToolResult::Error { error, .. } => panic!("expected success, got error: {}", error),
    }
}

/// Stage F — check_status returns a `progress` array for running agents.
#[tokio::test]
async fn check_status_returns_progress_array_when_running() {
    use crate::agents::progress::{ProgressKind, SubagentProgress};
    use std::time::SystemTime;

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
        .execute(
            serde_json::json!({"action": "check_status", "request_id": "rid"}),
            CancellationToken::new(),
        )
        .await;
    let output = match result {
        ToolResult::Success { output } => output,
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
        .execute(
            json!({
                "batch_tasks": [
                    {"task": "first task", "agent_type": "explore"},
                    {"task": "second task", "agent_type": "explore"}
                ]
            }),
            CancellationToken::new(),
        )
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
    }
}

// -------------------------------------------------------------------------
// Mixture-of-Agents (proposer_models + synthesize)
// -------------------------------------------------------------------------

/// `proposer_models` + `synthesize` parse into the run args verbatim.
#[test]
fn moa_parse_proposer_models_and_synthesize() {
    let action = parse_args(&json!({
        "task": "solve the hard problem",
        "proposer_models": ["claude-opus-4-8", "gpt-5", "  ", "deepseek-v3"],
        "synthesize": true,
        "aggregator_model": "claude-opus-4-8",
        "synthesis_instruction": "favour correctness over brevity"
    }))
    .unwrap();
    match action {
        SubagentAction::Run(args) => {
            // blank entries are filtered out
            assert_eq!(
                args.proposer_models.as_deref(),
                Some(&["claude-opus-4-8".to_string(), "gpt-5".to_string(), "deepseek-v3".to_string()][..])
            );
            assert!(args.synthesize);
            assert_eq!(args.aggregator_model.as_deref(), Some("claude-opus-4-8"));
            assert_eq!(
                args.synthesis_instruction.as_deref(),
                Some("favour correctness over brevity")
            );
        }
        _ => panic!("expected SubagentAction::Run"),
    }
}

/// `synthesize` with a fire-and-forget batch is rejected — the aggregator
/// would have nothing to fold.
#[test]
fn moa_synthesize_rejects_background() {
    let result = parse_args(&json!({
        "task": "x",
        "proposer_models": ["a", "b"],
        "synthesize": true,
        "run_in_background": true
    }));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("foreground"));
}

/// `proposer_models` replicates the top-level `task`, so the task is required.
#[test]
fn moa_proposer_models_requires_task() {
    let result = parse_args(&json!({
        "proposer_models": ["a", "b"]
    }));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("task"));
}

/// End-to-end MoA: proposers fan out, the aggregator folds them, and the
/// response carries `moa_completed` with a `synthesis` plus the raw results.
#[tokio::test]
async fn execute_moa_returns_synthesis() {
    let tool = make_tool();
    let result = tool
        .execute(
            json!({
                "task": "what is the answer",
                "proposer_models": ["model-a", "model-b"],
                "synthesize": true,
                "agent_type": "explore"
            }),
            CancellationToken::new(),
        )
        .await;

    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "moa_completed");
            assert_eq!(output["proposer_count"], 2);
            assert!(output["synthesis"].is_string(), "synthesis must be present");
            let results = output["results"].as_array().expect("results is array");
            assert_eq!(results.len(), 2, "raw proposals are preserved");
        }
        ToolResult::Error { error, .. } => panic!("expected success, got error: {error}"),
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
    ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>> {
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
        tool.execute(
            serde_json::json!({ "task": "hang", "timeout_secs": 30 }),
            CancellationToken::new(),
        )
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

/// Gap B follow-up — `execute()`'s per-call `cancel` arg should stop a
/// running subagent even when the run-level `parent_cancel` is still
/// alive. Proves the harness→subagent token bridge actually fires.
#[tokio::test]
async fn foreground_subagent_cancels_on_harness_per_call_token() {
    let parent = CancellationToken::new(); // run-level cancel — stays unfired
    let harness_call = CancellationToken::new(); // per-call cancel — fired below
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

    let cancel_for_task = harness_call.clone();
    let handle = tokio::spawn(async move {
        tool.execute(
            serde_json::json!({ "task": "hang", "timeout_secs": 30 }),
            cancel_for_task,
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    // Cancel ONLY the per-call token. Run-level token stays alive.
    harness_call.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("subagent did not honour per-call harness cancel within 3s")
        .expect("task join");
    assert!(
        matches!(result, ToolResult::Error { .. }),
        "harness-cancelled subagent must surface an error"
    );
    // parent stays unfired — proves the cancellation came from the
    // per-call bridge, not from a transitive run-level fire.
    assert!(!parent.is_cancelled());
}

struct UsageMockProvider;
impl AiProvider for UsageMockProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async {
            Ok(ProviderResponse {
                text: Some("done".into()),
                tool_calls: vec![],
                thinking: None,
                thinking_signature: None,
                stop_reason: crate::providers::adapter::StopReason::EndTurn,
                truncated_tool_call: None,
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

    let _ = tool
        .execute(
            serde_json::json!({ "task": "hi" }),
            CancellationToken::new(),
        )
        .await;

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
        .execute(
            serde_json::json!({ "task": "bg", "run_in_background": true }),
            CancellationToken::new(),
        )
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
        .execute(
            json!({
                "batch_tasks": [{"task": "x"}, {"task": "y"}],
                "run_in_background": true
            }),
            CancellationToken::new(),
        )
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

    let result = tool
        .execute(json!({ "action": "list" }), CancellationToken::new())
        .await;
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
            .execute(
                json!({ "action": "check_status", "request_id": "rid" }),
                CancellationToken::new(),
            )
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
        .execute(
            json!({ "action": "check_status", "request_id": "rid" }),
            CancellationToken::new(),
        )
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
