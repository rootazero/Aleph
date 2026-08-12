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
    )
}

/// W11 — the tool's ADDRESSING verbs must be session-scoped, not just its
/// enumeration verbs. Before the chokepoint, a request_id belonging to another
/// session (learned from an announce echo, a log line, or a paste) could be
/// read via `check_status` and killed via `cancel`, and `wait`'s
/// `unknown_request_ids` annotation — the sentence the model is shown — omitted
/// it, so the tool actively told the model the id was fine.
#[tokio::test]
async fn foreign_request_id_is_unreachable_from_another_session() {
    let tracker = make_tracker();
    let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
    let chain = crate::harness::chain_context::ChainContext::new();
    let tool = SubagentTool::new(
        provider,
        chain,
        make_registry(),
        tracker.clone(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
    .with_parent_session_id("s-mine".to_string());

    // A background sub-agent owned by a DIFFERENT session, already finished.
    tracker.register_with_meta(
        "theirs".to_string(),
        CancellationToken::new(),
        "their task".to_string(),
        crate::agents::background_tracker::SpawnMeta {
            root_session: "s-other".to_string(),
            depth: 1,
            ..Default::default()
        },
    );
    tracker.mark_completed(
        "theirs",
        crate::agents::background_tracker::CompletedOutcome::ok_text("their secret output"),
    );

    // check_status must not hand over the other session's output.
    let status = tool
        .execute(
            json!({ "action": "check_status", "request_id": "theirs" }),
            CancellationToken::new(),
        )
        .await;
    match status {
        ToolResult::Error { error, .. } => assert!(
            error.contains("No background sub-agent found"),
            "an out-of-scope id must read exactly like an unknown one, got: {error}"
        ),
        ToolResult::Success { output } => {
            panic!("check_status leaked another session's result: {}", output)
        }
    }

    // cancel must not kill the other session's run.
    let cancelled = tool
        .execute(
            json!({ "action": "cancel", "request_id": "theirs" }),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(cancelled, ToolResult::Error { .. }),
        "cancel must refuse an out-of-scope request_id, got {cancelled:?}"
    );

    // ...and `wait` must NAME it as unknown rather than silently parking on it.
    let waited = tool
        .execute(
            json!({
                "action": "wait",
                "request_ids": ["theirs"],
                "timeout_secs": 1
            }),
            CancellationToken::new(),
        )
        .await;
    match waited {
        ToolResult::Error { error, .. } => assert!(
            error.contains("theirs"),
            "wait must name the unreachable id so the model can fix the call, got: {error}"
        ),
        other => panic!("wait must not resolve an out-of-scope id, got {other:?}"),
    }
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
        _ => unreachable!("expected SubagentAction::Run"),
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
        _ => unreachable!("expected SubagentAction::Run"),
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
        _ => unreachable!("expected SubagentAction::CheckStatus"),
    }
}

#[test]
fn test_parse_args_request_id_with_task_is_run() {
    // When both task and request_id are provided, it's a Run action
    let action = parse_args(&json!({ "task": "do work", "request_id": "abc" })).unwrap();
    match action {
        SubagentAction::Run(args) => assert_eq!(args.task, "do work"),
        _ => unreachable!("expected SubagentAction::Run when both task and request_id given"),
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
    // W10 honest-trim: `name` is gone from the run surface — ephemeral spawns
    // are not addressable teammates. `team_name` survives for the messaging
    // actions only.
    assert!(props["name"].is_null());
    assert!(props["team_name"].is_object());
    assert!(props["to"].is_object());
    assert!(props["text"].is_object());
}

/// W10 honest-trim regression — `name`/`team_name` on a run are rejected
/// loudly (the old registration produced an unreadable dead-letter roster
/// row), never silently accepted or ignored.
#[test]
fn test_parse_args_name_on_run_is_rejected() {
    for input in [
        json!({ "task": "build feature", "name": "builder-1", "team_name": "alpha" }),
        json!({ "task": "build feature", "name": "builder-1" }),
        json!({ "task": "build feature", "team_name": "alpha" }),
    ] {
        let result = parse_args(&input);
        assert!(result.is_err(), "expected rejection for {input}");
        let err = result.unwrap_err();
        assert!(
            err.contains("not addressable teammates"),
            "unexpected error: {err}"
        );
    }
}

/// Review C3 — schema-completing providers emit explicit `null` for every
/// advertised property; null `name`/`team_name` must read as absent, not
/// trip the addressable-teammates rejection.
#[test]
fn test_parse_args_null_name_team_name_read_as_absent() {
    let action = parse_args(&json!({ "task": "build feature", "name": null, "team_name": null }))
        .expect("null name/team_name must parse as absent");
    assert!(matches!(action, SubagentAction::Run(_)));
}

/// The `model` argument is stamped onto the child's requests verbatim, so the
/// schema must not advertise abstract tiers. It used to suggest "'fast', 'deep'",
/// neither of which resolves anywhere — a model that followed the example sent
/// `model: "fast"` to the API and the whole spawn failed.
#[test]
fn schema_model_description_advertises_no_phantom_tiers() {
    let schema = make_tool().schema();
    let desc = schema["properties"]["model"]["description"]
        .as_str()
        .expect("model description present");
    assert!(
        !desc.contains("'fast'") && !desc.contains("'deep'"),
        "model description must not advertise tier vocabulary that resolves nowhere: {desc}"
    );
    assert!(
        desc.contains("provider/model"),
        "the qualified form is the cross-vendor route — it must be discoverable: {desc}"
    );
}

/// A near-miss key must be rejected, not silently ignored: `agent` instead of
/// `agent_type` used to run the DEFAULT role while reporting success, so the
/// caller never learned its role selection was dropped.
#[test]
fn parse_args_rejects_unknown_keys() {
    for (input, offender) in [
        (json!({ "task": "t", "agent": "explore" }), "agent"),
        (json!({ "prompt": "t" }), "prompt"),
        (json!({ "task": "t", "background": true }), "background"),
    ] {
        let err = parse_args(&input).expect_err("unknown key must be rejected");
        assert!(
            err.contains("unknown argument(s)") && err.contains(offender),
            "unexpected error for {input}: {err}"
        );
        assert!(
            err.contains("agent_type"),
            "error must list the accepted arguments: {err}"
        );
    }
}

/// Explicit `null` on an unknown key carries no intent (schema-completing
/// providers emit it) — it must not trip the rejection.
#[test]
fn parse_args_ignores_null_unknown_keys() {
    let action = parse_args(&json!({ "task": "t", "reasoning": null }))
        .expect("null unknown key must read as absent");
    assert!(matches!(action, SubagentAction::Run(_)));
}

/// Drift guard: the parser's accepted-key set and the hand-written schema are
/// two halves of one contract. Every advertised property must be accepted, and
/// every accepted key must be advertised — except `name`, which is deliberately
/// unadvertised so its dedicated rejection message can fire.
#[test]
fn subagent_schema_properties_match_accepted_keys() {
    let schema = make_tool().schema();
    let props = schema["properties"]
        .as_object()
        .expect("schema advertises an object of properties");
    for key in props.keys() {
        assert!(
            super::types::ACCEPTED_ARG_KEYS.contains(&key.as_str()),
            "schema advertises '{key}' but the parser rejects it as unknown"
        );
    }
    for key in super::types::ACCEPTED_ARG_KEYS {
        if *key == "name" {
            continue;
        }
        assert!(
            props.contains_key(*key),
            "parser accepts '{key}' but the schema never advertises it"
        );
    }
}

/// A model-supplied `timeout_secs` is clamped so the child's own wall-clock
/// timeout always fires before the `subagent` tool budget, and `0` can never
/// mean "die before the first turn".
#[test]
fn parse_args_clamps_run_timeout_into_range() {
    let max = crate::tools::budget::builtin_tool_budget_ms("subagent")
        .expect("subagent has a budget row")
        / 1000;

    let action = parse_args(&json!({ "task": "t", "timeout_secs": 0 })).unwrap();
    match action {
        SubagentAction::Run(args) => assert_eq!(args.timeout_secs, 1),
        _ => unreachable!("expected Run"),
    }

    let action = parse_args(&json!({ "task": "t", "timeout_secs": 999_999 })).unwrap();
    match action {
        SubagentAction::Run(args) => {
            assert!(
                args.timeout_secs < max,
                "clamped run timeout {} must leave headroom under the {max}s tool budget",
                args.timeout_secs
            );
        }
        _ => unreachable!("expected Run"),
    }

    // Per-entry batch overrides are clamped by the same ceiling.
    let action = parse_args(&json!({
        "task": "t",
        "batch_tasks": [{ "task": "a", "timeout_secs": 999_999 }, { "task": "b", "timeout_secs": 0 }],
    }))
    .unwrap();
    match action {
        SubagentAction::Run(args) => {
            let batch = args.batch_tasks.expect("batch parsed");
            assert!(batch[0].timeout_secs.expect("clamped") < max);
            assert_eq!(batch[1].timeout_secs, Some(1));
        }
        _ => unreachable!("expected Run"),
    }
}

/// W19 ① — the parse-time clamp is per CHILD and silently assumes one child per
/// call. A batch runs in `ceil(rows / permits)` waves (plus one serial reduce
/// round when synthesizing), so a batch clamped only per-child is arithmetically
/// guaranteed to overrun the tool budget and be discarded whole: five full-length
/// children against four permits is two waves, i.e. twice the share.
#[test]
fn wave_aware_cap_keeps_the_whole_batch_inside_the_tool_share() {
    use super::types::{max_run_timeout_secs, wave_aware_child_timeout_cap};
    let share = max_run_timeout_secs();

    // 5 rows / 4 permits = 2 waves.
    let cap = wave_aware_child_timeout_cap(5, 4, 0);
    assert!(
        cap < share,
        "a multi-wave batch must not hand each child the whole share ({cap} vs {share})"
    );
    assert!(
        cap * 2 <= share,
        "two waves of {cap}s must fit the {share}s share"
    );

    // One wave fits flat — no artificial shortening.
    assert_eq!(
        wave_aware_child_timeout_cap(4, 4, 0),
        share,
        "a single-wave batch must keep the full per-child share"
    );

    // The MoA reduce is one more serial round, and it is charged for.
    let with_reduce = wave_aware_child_timeout_cap(5, 4, 1);
    assert!(
        with_reduce * 3 <= share,
        "two waves + one reduce round of {with_reduce}s must fit the {share}s share"
    );

    // Degenerate inputs must never divide by zero nor produce a 0s timeout
    // ("timed out after 0s" before the child ever thinks).
    assert!(wave_aware_child_timeout_cap(0, 0, 0) >= 1);
    assert!(wave_aware_child_timeout_cap(100_000, 1, 0) >= 1);
}

/// W2 defense-in-depth — the ghost 'result' action (coached by old announce
/// prompts) parses as check_status instead of erroring.
#[test]
fn test_parse_args_result_alias_reads_as_check_status() {
    let action = parse_args(&json!({ "action": "result", "request_id": "abc-123" })).unwrap();
    match action {
        SubagentAction::CheckStatus(rid) => assert_eq!(rid, "abc-123"),
        _ => unreachable!("expected SubagentAction::CheckStatus for 'result' alias"),
    }
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
        _ => unreachable!("expected SubagentAction::SendMessage"),
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
        _ => unreachable!("expected SubagentAction::ReadInbox"),
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
        _ => unreachable!("expected error for unknown request_id"),
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
    );

    let result = tool
        .execute(json!({ "request_id": "test-id" }), CancellationToken::new())
        .await;
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "completed");
            assert_eq!(output["result"], "the result");
        }
        _ => unreachable!("expected success with completed status"),
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
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {}", error),
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
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {}", error),
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
        _ => unreachable!("expected ToolResult::Error"),
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
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {}", error),
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
        _ => unreachable!("expected ToolResult::Error"),
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
    let tool =
        make_tool().with_plugin_registry(Arc::new(tokio::sync::RwLock::new(PluginRegistry::new())));
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
        _ => unreachable!("expected error when message router not configured"),
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
        _ => unreachable!("expected error when inbox not configured"),
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
            // W6 — the cap signal is always present (false on a clean run).
            assert_eq!(output["hit_iteration_limit"], json!(false));
        }
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {}", error),
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
    );

    let result = tool
        .execute(
            serde_json::json!({"action": "check_status", "request_id": "rid"}),
            CancellationToken::new(),
        )
        .await;
    let output = match result {
        ToolResult::Success { output } => output,
        other => unreachable!("expected Success, got {other:?}"),
    };
    let progress = output.get("progress").expect("progress field present");
    assert!(progress.is_array());
    assert_eq!(progress.as_array().unwrap().len(), 1);
}

/// B18 — a *failed* background sub-agent must not reach the parent as a bare
/// error string. The trajectory (what it was doing, how far it got) is compacted
/// into the error the model reads, and the completed entry still answers
/// `progress`. Without the tail carried into `mark_completed`, the parent could
/// only see "Background sub-agent failed: boom" and had no way to tell a
/// first-step crash from a nineteen-step dead end.
#[tokio::test]
async fn check_status_failed_error_carries_progress_trail() {
    use crate::agents::progress::{ProgressKind, SubagentProgress};
    use std::time::SystemTime;

    let tracker = make_tracker();
    tracker.register("rid".into(), CancellationToken::new(), "explore".into());
    for (step, tool) in [(1, "read_file"), (2, "grep"), (3, "bash")] {
        tracker.push_progress(
            "rid",
            SubagentProgress {
                step,
                timestamp: SystemTime::now(),
                kind: ProgressKind::ToolCalled,
                tool_name: Some(tool.into()),
                latency_ms: None,
                preview: None,
            },
        );
    }
    tracker.mark_completed(
        "rid",
        CompletedOutcome::Err("Sub-agent timed out after 120s".into()),
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
    );

    let result = tool
        .execute(
            json!({"action": "check_status", "request_id": "rid"}),
            CancellationToken::new(),
        )
        .await;
    let error = match result {
        ToolResult::Error { error, .. } => error,
        other => unreachable!("a failed child must stay a ToolResult::Error, got {other:?}"),
    };
    assert!(
        error.contains("Sub-agent timed out after 120s"),
        "the original cause must survive: {error}"
    );
    assert!(
        error.contains("bash"),
        "the last tool the child ran must be in the error: {error}"
    );
    assert!(
        error.contains("3 steps"),
        "how far the child got must be in the error: {error}"
    );
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
        _ => unreachable!("expected SubagentAction::Run"),
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
        _ => unreachable!("expected SubagentAction::Run"),
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
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {error}"),
    }
}

/// W12 — sync batch children must hold running-only tracker entries: visible to
/// the gateway's session child-walk (`running_runs_of_session`, so a leader
/// cancel reaches them; `session_has_running` reports the parent busy) while in
/// flight, and fully delisted afterwards with NO completed retention (the
/// results are returned inline, so a completed entry would only feed the
/// proactive announce / `list` with duplicates).
#[tokio::test]
async fn sync_batch_registers_running_only_entries() {
    /// Provider that parks until the test releases it, so the test can
    /// observe tracker state while the sync batch is in flight.
    struct GatedProvider {
        gate: Arc<tokio::sync::Semaphore>,
    }
    impl AiProvider for GatedProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            let gate = self.gate.clone();
            Box::pin(async move {
                let _permit = gate.acquire().await.expect("gate is never closed");
                Ok(ProviderResponse::text_only("gated response".to_string()))
            })
        }
        fn name(&self) -> &str {
            "gated"
        }
        fn color(&self) -> &str {
            "#000000"
        }
    }

    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(GatedProvider { gate: gate.clone() });
    let tracker = make_tracker();
    let root = "agent:w12-batch:peer:user";
    let tool = SubagentTool::new(
        provider,
        crate::harness::chain_context::ChainContext::new(),
        make_registry(),
        tracker.clone(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
    .with_parent_session_id(root);

    let exec = tokio::spawn(async move {
        tool.execute(
            json!({ "batch_tasks": [{"task": "a"}, {"task": "b"}] }),
            CancellationToken::new(),
        )
        .await
    });

    // While the children are parked on the gate the tracker must read the
    // parent session as having running children — so a leader cancel / status
    // query reaches them (this is the whole point of W12).
    let mut saw_running = false;
    for _ in 0..400 {
        if tracker.session_has_running(root) {
            saw_running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        saw_running,
        "sync batch children must register in the running set"
    );

    // Release the children and let the batch finish.
    gate.add_permits(64);
    let result = exec.await.expect("execute task must not panic");
    match result {
        ToolResult::Success { output } => assert_eq!(output["status"], "batch_completed"),
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {error}"),
    }

    // RAII delist: nothing left running, and — running-only — nothing
    // retained as completed.
    assert!(
        tracker.list_running(None).is_empty(),
        "sync batch entries must delist when the fan-out settles"
    );
    assert!(
        tracker.all_completed(None).is_empty(),
        "sync path must not retain completed entries (no announce source)"
    );
}

/// Provider that never answers. Children built on it only ever end by a clock
/// or a cancel — which is exactly the shape the batch deadline exists for.
struct NeverAnsweringProvider;

impl AiProvider for NeverAnsweringProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
    fn name(&self) -> &str {
        "never"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// W19 ① end-to-end: five 1500 s children against four permits are two waves,
/// so each child must be re-clamped to the share that actually fits the tool
/// budget. Before the fix every child kept the full single-child ceiling and the
/// batch was arithmetically certain to blow the tool budget — at which point the
/// dispatch-level `ToolError::Timeout` threw the entire call away.
///
/// Observable through the child's own timeout prose, which names the number it
/// was actually given.
#[tokio::test(start_paused = true)]
async fn sync_batch_reclamps_each_child_to_its_wave_share() {
    use super::types::{max_run_timeout_secs, wave_aware_child_timeout_cap};

    let tool = SubagentTool::new(
        Arc::new(NeverAnsweringProvider),
        crate::harness::chain_context::ChainContext::new(),
        make_registry(),
        make_tracker(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    );
    let requested = max_run_timeout_secs();
    // W27 — read the permit count off the tool's own semaphore rather than the
    // compile-time default: the cap is configurable now, and production divides
    // by what the semaphore actually holds.
    let permits = tool.subagent_semaphore.available_permits();
    let rows = permits + 1;
    let expected_cap = wave_aware_child_timeout_cap(rows, permits, 0);
    assert!(
        expected_cap < requested,
        "test premise: one row more than the permit count is two waves"
    );

    let batch: Vec<_> = (0..rows)
        .map(|i| json!({ "task": format!("row {i}"), "timeout_secs": requested }))
        .collect();
    let result = tool
        .execute(
            json!({ "batch_tasks": batch, "run_in_background": false }),
            CancellationToken::new(),
        )
        .await;

    let ToolResult::Success { output } = result else {
        unreachable!("expected an aggregated batch result, got {result:?}");
    };
    let result_rows = output["results"].as_array().expect("results is array");
    assert_eq!(result_rows.len(), rows);
    for (i, row) in result_rows.iter().enumerate() {
        assert_eq!(row["index"], i, "row order is part of the contract");
        let err = row["error"].as_str().unwrap_or_default();
        assert!(
            err.contains(&format!("timed out after {expected_cap}s")),
            "row {i} must have been given its WAVE share, not the whole \
             single-child ceiling; got: {err}"
        );
    }
}

/// W19 ② + ③: when the batch's wall-clock share elapses, the parent gets the
/// results that DID land plus the indices that did not — not a `ToolError` that
/// discards twenty minutes of finished work (A2: compress the failure into
/// context and let the model decide) — and nothing is left running behind it
/// (a dropped `JoinHandle` DETACHES; a `JoinSet` aborts).
///
/// The stall is built with every permit already held: the spawner's permit wait
/// happens BEFORE its own `tokio::time::timeout` arms, so a queued child is
/// invisible to every per-child clock in the system. That is precisely the case
/// only a batch-level deadline can end.
#[tokio::test(start_paused = true)]
async fn sync_batch_returns_partial_results_and_leaves_nothing_running() {
    let tracker = make_tracker();
    let root = "agent:w19-partial:peer:user";
    let tool = SubagentTool::new(
        Arc::new(NeverAnsweringProvider),
        crate::harness::chain_context::ChainContext::new(),
        make_registry(),
        tracker.clone(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
    .with_parent_session_id(root);

    // Starve the fan-out: every concurrency permit is held for the whole call.
    let permits = u32::try_from(tool.subagent_semaphore.available_permits()).unwrap();
    let _held = tool
        .subagent_semaphore
        .clone()
        .acquire_many_owned(permits)
        .await
        .expect("semaphore is never closed");

    let batch: Vec<_> = (0..5)
        .map(|i| json!({ "task": format!("row {i}"), "timeout_secs": 1500 }))
        .collect();
    // Outer guard: before W19 the join loop had no deadline at all, so this
    // await never returned. The guard turns that hang into a failed assertion.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(7200),
        tool.execute(
            json!({ "batch_tasks": batch, "run_in_background": false }),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("the batch must give up on its own share, not park forever");

    let ToolResult::Success { output } = result else {
        unreachable!("a stalled batch must return partial results, not an error: {result:?}");
    };
    assert_eq!(
        output["status"], "batch_partial",
        "a batch that ran out of wall clock must not read as completed"
    );
    let incomplete = output["incomplete_indices"]
        .as_array()
        .expect("partial return must name the rows that did not finish");
    assert_eq!(incomplete.len(), 5, "no row could have finished");
    let rows = output["results"].as_array().expect("results is array");
    assert_eq!(rows.len(), 5, "every row keeps a slot, finished or not");
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["index"], i);
        assert_ne!(row["status"], "completed", "row {i} cannot be completed");
    }

    // W19 ③ — no detach: the tool does not return until its children are gone.
    assert!(
        !tracker.session_has_running(root),
        "unfinished children must be aborted before the tool returns, not left \
         detached to burn tokens with nobody reading the result"
    );
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
                Some(
                    &[
                        "claude-opus-4-8".to_string(),
                        "gpt-5".to_string(),
                        "deepseek-v3".to_string()
                    ][..]
                )
            );
            assert!(args.synthesize);
            assert_eq!(args.aggregator_model.as_deref(), Some("claude-opus-4-8"));
            assert_eq!(
                args.synthesis_instruction.as_deref(),
                Some("favour correctness over brevity")
            );
        }
        _ => unreachable!("expected SubagentAction::Run"),
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
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {error}"),
    }
}

/// Provider whose `process` always fails, so every proposer in a fan-out dies.
struct FailingProvider;
impl AiProvider for FailingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async {
            Err(crate::error::AlephError::Other {
                message: "provider down".to_string(),
                suggestion: None,
            })
        })
    }
    fn name(&self) -> &str {
        "failing"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// A requested MoA reduce that never ran must not report as a plain
/// `batch_completed`: the model asked for one synthesized answer, and silently
/// handing back N raw failures let it believe the synthesis had happened.
#[tokio::test]
async fn execute_moa_reports_when_no_proposal_survived() {
    let provider: Arc<dyn AiProvider> = Arc::new(FailingProvider);
    let chain = crate::harness::chain_context::ChainContext::new();
    let tool = SubagentTool::new(
        provider,
        chain,
        make_registry(),
        make_tracker(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    );

    let result = tool
        .execute(
            json!({
                "task": "what is the answer",
                "proposer_models": ["model-a", "model-b"],
                "synthesize": true,
                "agent_type": "explore",
                "timeout_secs": 10
            }),
            CancellationToken::new(),
        )
        .await;

    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "moa_no_proposals");
            assert!(
                output["note"].is_string(),
                "must explain the skipped reduce"
            );
            let results = output["results"].as_array().expect("results is array");
            assert_eq!(results.len(), 2, "raw per-proposal failures are preserved");
        }
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {error}"),
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
        other => unreachable!("expected background success, got {other:?}"),
    };

    // Poll until the background task completes (bounded).
    for _ in 0..100 {
        if tracker
            .list_running(None)
            .iter()
            .all(|(id, _, _)| id != &rid)
        {
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

/// P1 data isolation: `spawn_background`'s `tokio::spawn` must re-seed the
/// scope and project-root task-locals inside the spawned task — otherwise a
/// background subagent's memory reads silently fall back to the unscoped /
/// wrong-project namespace regardless of what the parent run was scoped to.
#[tokio::test]
async fn background_subagent_reseeds_scope_and_project_root() {
    use crate::scope::ScopeAttribution;
    use std::path::PathBuf;

    /// Captures the ambient scope + project root observed at provider-call
    /// time — i.e. from inside the task `spawn_background` spawns.
    struct ScopeCapturingProvider {
        observed: Arc<std::sync::Mutex<Option<(Option<ScopeAttribution>, Option<PathBuf>)>>>,
    }

    impl AiProvider for ScopeCapturingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            let observed = self.observed.clone();
            Box::pin(async move {
                *observed.lock().unwrap() = Some((
                    crate::scope::current_scope(),
                    crate::projects::current_project_root(),
                ));
                Ok(ProviderResponse::text_only("mock response".to_string()))
            })
        }

        fn name(&self) -> &str {
            "scope-capture"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    let observed: Arc<std::sync::Mutex<Option<(Option<ScopeAttribution>, Option<PathBuf>)>>> =
        Arc::new(std::sync::Mutex::new(None));
    let chain = crate::harness::chain_context::ChainContext::new();
    let tracker = make_tracker();
    let tool = SubagentTool::new(
        Arc::new(ScopeCapturingProvider {
            observed: observed.clone(),
        }),
        chain,
        make_registry(),
        tracker.clone(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    );

    let dir = std::env::temp_dir().join("aleph-p1-scope-test");
    let attr = ScopeAttribution::personal("u-alice");

    let rid = crate::scope::with_scope(
        Some(attr),
        crate::projects::with_project_root(Some(dir.clone()), async {
            let out = tool
                .execute(
                    json!({ "task": "bg", "run_in_background": true }),
                    CancellationToken::new(),
                )
                .await;
            match out {
                ToolResult::Success { output } => {
                    output["request_id"].as_str().unwrap().to_string()
                }
                other => unreachable!("expected background success, got {other:?}"),
            }
        }),
    )
    .await;

    // Poll until the background task completes (bounded).
    for _ in 0..100 {
        if tracker
            .list_running(None)
            .iter()
            .all(|(id, _, _)| id != &rid)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let (seen_scope, seen_root) = observed
        .lock()
        .unwrap()
        .clone()
        .expect("provider must have been invoked inside the spawned task");
    assert_eq!(
        seen_scope.map(|a| a.owner_user_id),
        Some("u-alice".to_string()),
        "scope must be re-seeded inside the tokio::spawn boundary"
    );
    assert_eq!(
        seen_root,
        Some(dir),
        "project root must be re-seeded inside the tokio::spawn boundary"
    );
}

/// W1 — the SYNC parallel fan-out spawns one `tokio::spawn` per batch row and
/// must re-seed the same task-locals `spawn_background` does. Before the fix
/// the sync branch captured nothing, so every batch subagent's memory writes
/// landed in the unscoped default partition instead of the parent run's room /
/// personal one — silently, with the batch result looking perfectly normal.
#[tokio::test]
async fn sync_batch_subagents_reseed_scope_and_project_root() {
    use crate::scope::ScopeAttribution;
    use std::path::PathBuf;

    /// Captures the ambient scope + project root observed at provider-call
    /// time — i.e. from inside the tasks the sync fan-out spawns.
    struct ScopeCapturingProvider {
        observed: Arc<std::sync::Mutex<Vec<(Option<ScopeAttribution>, Option<PathBuf>)>>>,
    }

    impl AiProvider for ScopeCapturingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            let observed = self.observed.clone();
            Box::pin(async move {
                observed.lock().unwrap().push((
                    crate::scope::current_scope(),
                    crate::projects::current_project_root(),
                ));
                Ok(ProviderResponse::text_only("mock response".to_string()))
            })
        }

        fn name(&self) -> &str {
            "scope-capture-batch"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    let observed: Arc<std::sync::Mutex<Vec<(Option<ScopeAttribution>, Option<PathBuf>)>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let chain = crate::harness::chain_context::ChainContext::new();
    let tool = SubagentTool::new(
        Arc::new(ScopeCapturingProvider {
            observed: observed.clone(),
        }),
        chain,
        make_registry(),
        make_tracker(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    );

    let dir = std::env::temp_dir().join("aleph-w1-sync-batch-scope");
    let attr = ScopeAttribution::personal("u-batch");

    let result = crate::scope::with_scope(
        Some(attr),
        crate::projects::with_project_root(Some(dir.clone()), async {
            tool.execute(
                json!({
                    "batch_tasks": [
                        { "task": "one" },
                        { "task": "two" }
                    ],
                    "run_in_background": false
                }),
                CancellationToken::new(),
            )
            .await
        }),
    )
    .await;
    assert!(
        matches!(result, ToolResult::Success { .. }),
        "sync batch must succeed, got {result:?}"
    );

    let seen = observed.lock().unwrap().clone();
    assert!(
        !seen.is_empty(),
        "provider must have been invoked inside the spawned fan-out tasks"
    );
    for (scope, root) in seen {
        assert_eq!(
            scope.map(|a| a.owner_user_id),
            Some("u-batch".to_string()),
            "scope must be re-seeded inside each sync fan-out tokio::spawn"
        );
        assert_eq!(
            root,
            Some(dir.clone()),
            "project root must be re-seeded inside each sync fan-out tokio::spawn"
        );
    }
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
        ToolResult::Error { error, .. } => unreachable!("expected success, got error: {error}"),
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
        other => unreachable!("expected Success, got {other:?}"),
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
            other => unreachable!("poll {poll}: expected Success, got {other:?}"),
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
        other => unreachable!("expected Success, got {other:?}"),
    }
}

// -------------------------------------------------------------------------
// wait action — event-driven blocking on background sub-agents
// -------------------------------------------------------------------------

/// Build a tool over a caller-supplied tracker so tests can pre-populate it.
fn tool_with_tracker(tracker: Arc<BackgroundAgentTracker>) -> SubagentTool {
    let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
    let chain = crate::harness::chain_context::ChainContext::new();
    SubagentTool::new(
        provider,
        chain,
        make_registry(),
        tracker,
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
}

#[test]
fn parse_wait_single_multi_and_empty() {
    // Single request_id → one-element set, default timeout.
    match parse_args(&json!({ "action": "wait", "request_id": "r1" })).unwrap() {
        SubagentAction::Wait {
            request_ids,
            timeout_secs,
        } => {
            assert_eq!(request_ids, vec!["r1".to_string()]);
            assert_eq!(timeout_secs, 120);
        }
        other => unreachable!("expected Wait, got {other:?}"),
    }
    // request_ids array → multi set (blanks dropped); timeout clamped to ceiling.
    match parse_args(&json!({
        "action": "wait",
        "request_ids": ["a", "b", "  ", "c"],
        "timeout_secs": 100_000
    }))
    .unwrap()
    {
        SubagentAction::Wait {
            request_ids,
            timeout_secs,
        } => {
            assert_eq!(
                request_ids,
                vec!["a".to_string(), "b".to_string(), "c".to_string()]
            );
            assert_eq!(timeout_secs, 600);
        }
        other => unreachable!("expected Wait, got {other:?}"),
    }
    // Neither field → error.
    assert!(parse_args(&json!({ "action": "wait" })).is_err());
}

#[tokio::test]
async fn wait_action_returns_completed_and_consumes() {
    let tracker = make_tracker();
    tracker.mark_completed("rid", CompletedOutcome::ok_text("the answer"));
    let tool = tool_with_tracker(tracker.clone());

    let result = tool
        .execute(
            json!({ "action": "wait", "request_id": "rid", "timeout_secs": 5 }),
            CancellationToken::new(),
        )
        .await;
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "completed");
            assert_eq!(output["result"], "the answer");
        }
        other => unreachable!("expected Success, got {other:?}"),
    }
    // wait consumed the result → the announce would now skip re-delivering it.
    assert!(tracker.is_consumed("rid"));
}

/// W9 regression — cancelling an already-finished child delivers its result,
/// so it must mark the result consumed (like check_status / wait) or the
/// proactive announce burns a fresh parent turn re-delivering it.
#[tokio::test]
async fn cancel_on_already_completed_marks_consumed() {
    let tracker = make_tracker();
    tracker.mark_completed("rid", CompletedOutcome::ok_text("done early"));
    let tool = tool_with_tracker(tracker.clone());

    let result = tool
        .execute(
            json!({ "action": "cancel", "request_id": "rid" }),
            CancellationToken::new(),
        )
        .await;
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "already_completed");
            assert_eq!(output["result"], "done early");
        }
        other => unreachable!("expected already_completed Success, got {other:?}"),
    }
    assert!(
        tracker.is_consumed("rid"),
        "cancel's already-completed branch must dedup with the announce"
    );
}

#[tokio::test]
async fn wait_action_reports_still_running_on_timeout() {
    let tracker = make_tracker();
    tracker.register("rid".into(), CancellationToken::new(), "long job".into());
    let tool = tool_with_tracker(tracker);

    // timeout_secs clamps to a 1s minimum; the job never finishes in-window.
    let result = tool
        .execute(
            json!({ "action": "wait", "request_id": "rid", "timeout_secs": 1 }),
            CancellationToken::new(),
        )
        .await;
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "still_running");
            assert_eq!(output["request_id"], "rid");
        }
        other => unreachable!("expected still_running Success, got {other:?}"),
    }
}

#[tokio::test]
async fn wait_action_unknown_id_errors() {
    let tool = make_tool();
    let result = tool
        .execute(
            json!({ "action": "wait", "request_id": "ghost", "timeout_secs": 1 }),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, ToolResult::Error { .. }));
}

#[tokio::test]
async fn wait_action_multi_returns_first_completion() {
    let tracker = make_tracker();
    tracker.register("a".into(), CancellationToken::new(), "slow".into());
    tracker.mark_completed("b", CompletedOutcome::ok_text("b-first"));
    let tool = tool_with_tracker(tracker.clone());

    let result = tool
        .execute(
            json!({ "action": "wait", "request_ids": ["a", "b"], "timeout_secs": 5 }),
            CancellationToken::new(),
        )
        .await;
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "completed");
            assert_eq!(output["request_id"], "b");
            assert_eq!(output["result"], "b-first");
        }
        other => unreachable!("expected Success, got {other:?}"),
    }
    // Only the finished id is consumed; the still-running sibling is not.
    assert!(tracker.is_consumed("b"));
    assert!(!tracker.is_consumed("a"));
}

/// Wiring proof — the foreground / aggregator / batch / background
/// scopes all build the same shape against `SubagentTool::cancel_for_child_with`:
///
/// ```ignore
/// let token = self.cancel_for_child_with(harness);
/// let _guard = CancelGuard::new(token.clone());
/// ```
///
/// Without the guard, `CancellationToken::drop` does NOT auto-cancel, so a
/// panic in the body of `runtime.run(...).await` (foreground / aggregator —
/// no `catch_unwind`) or after `catch_unwind` (batch / background) leaks the
/// bridge watcher parked on `harness.cancelled() | token.cancelled()`.
///
/// This test exercises the EXACT shape against the real
/// `cancel_for_child_with` API and asserts both halves of the contract:
///   - WITHOUT guard on panic: token stays live, watcher stays parked
///   - WITH    guard on panic: token is cancelled, watcher exits
///
/// Sequential arms — the first arm's leaked task must not influence the
/// second arm's observation, so they run in distinct scopes with their
/// own cancellation tokens.
#[tokio::test]
async fn wiring_proof_guard_terminates_bridge_on_unwind() {
    use super::spawn::CancelGuard;

    let tool = make_tool();

    // ─── Arm 1: WITHOUT guard → watcher must stay parked ─────────────
    {
        let harness = CancellationToken::new();
        let token = tool.cancel_for_child_with(&harness);
        let probe = token.clone();
        let token_w = token.clone();
        let harness_w = harness.clone();
        let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exited_w = exited.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = harness_w.cancelled() => {}
                _ = token_w.cancelled() => {}
            }
            exited_w.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async move {
            let _hold = token;
            panic!("simulated runtime.run panic (no guard)");
        }))
        .await;
        assert!(result.is_err(), "arm1: scope must have panicked");

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert!(
            !probe.is_cancelled(),
            "arm1 control: without guard, token must NOT be cancelled on unwind"
        );
        assert!(
            !exited.load(std::sync::atomic::Ordering::SeqCst),
            "arm1 control: without guard, bridge watcher must remain parked"
        );

        harness.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_millis(100), watcher).await;
    }

    // ─── Arm 2: WITH guard → watcher must exit ───────────────────────
    {
        let harness = CancellationToken::new();
        let token = tool.cancel_for_child_with(&harness);
        let probe = token.clone();
        let token_w = token.clone();
        let harness_w = harness.clone();
        let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exited_w = exited.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = harness_w.cancelled() => {}
                _ = token_w.cancelled() => {}
            }
            exited_w.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        let token_g = token;
        let result = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async move {
            let _guard = CancelGuard::new(token_g);
            panic!("simulated runtime.run panic (with guard)");
        }))
        .await;
        assert!(result.is_err(), "arm2: scope must have panicked");

        let _ = tokio::time::timeout(std::time::Duration::from_millis(200), watcher)
            .await
            .expect("arm2: watcher must exit well before the timeout");
        assert!(
            probe.is_cancelled(),
            "arm2: guard must have cancelled the bridge token on unwind"
        );
        assert!(
            exited.load(std::sync::atomic::Ordering::SeqCst),
            "arm2: bridge watcher must have exited"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// §4.11 round-8 — wait cancellability, fan-out drain terminal, announce
// dedup at cancel, and the scoped/compact `list` directory.
// ─────────────────────────────────────────────────────────────────────────

/// Build a tool whose background sub-agents are owned by `session`.
fn tool_for_session(tracker: Arc<BackgroundAgentTracker>, session: &str) -> SubagentTool {
    let provider: Arc<dyn AiProvider> = Arc::new(MockAiProvider);
    SubagentTool::new(
        provider,
        crate::harness::chain_context::ChainContext::new(),
        make_registry(),
        tracker,
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
    .with_parent_session_id(session)
}

/// Register a background agent owned by `session`.
fn register_owned(tracker: &BackgroundAgentTracker, id: &str, session: &str) {
    tracker.register_with_meta(
        id.to_string(),
        CancellationToken::new(),
        format!("task {id}"),
        crate::agents::background_tracker::SpawnMeta {
            root_session: session.to_string(),
            depth: 1,
            ..Default::default()
        },
    );
}

/// A parked `wait` must observe the harness cancel token.
///
/// It used to ignore it entirely, so a `/stop` landing on a run that was inside
/// `wait(timeout_secs=600)` left that run wedged in the sleep for up to ten more
/// minutes. Asserted against a wall clock, because "the token is passed in" is
/// exactly the kind of wiring that can be present and still not connected.
#[tokio::test]
async fn wait_returns_promptly_when_the_harness_cancels() {
    let tracker = make_tracker();
    register_owned(&tracker, "slow-child", "sess-cancel");
    let tool = tool_for_session(tracker, "sess-cancel");

    let cancel = CancellationToken::new();
    let fire = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        fire.cancel();
    });

    let started = std::time::Instant::now();
    let result = tool
        .execute(
            json!({ "action": "wait", "request_id": "slow-child", "timeout_secs": 600 }),
            cancel,
        )
        .await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "a cancelled wait must not run out its window (took {elapsed:?})"
    );
    match result {
        // Not an Error: nothing failed. Reporting a failure would feed the
        // harness failure counter and the cross-batch memo a verdict about a
        // call that was merely interrupted.
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "wait_interrupted");
            assert_eq!(output["still_running"][0]["request_id"], "slow-child");
        }
        other => unreachable!("expected an interrupted-wait report, got {other:?}"),
    }
}

/// Round-8 — an interrupted `wait` over a set that contains unknown ids
/// must surface them via `unknown_request_ids`, mirroring the success
/// path's `annotate_unknown` so a typo'd id is diagnosed even when the
/// parent cancels the wait instead of waiting it out. Without this the
/// only signal of a typo is the absence of an entry in `still_running`,
/// which reads to the model as "no children left" rather than "you got
/// the request_id wrong".
#[tokio::test]
async fn wait_cancelled_carries_unknown_request_ids() {
    let tracker = make_tracker();
    register_owned(&tracker, "real", "sess-cancel-unknown");
    let tool = tool_for_session(tracker, "sess-cancel-unknown");

    let cancel = CancellationToken::new();
    let fire = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        fire.cancel();
    });

    let result = tool
        .execute(
            json!({
                "action": "wait",
                "request_ids": ["real", "typo-a", "typo-b"],
                "timeout_secs": 600,
            }),
            cancel,
        )
        .await;
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "wait_interrupted");
            let still: Vec<&str> = output["still_running"]
                .as_array()
                .expect("still_running is an array")
                .iter()
                .map(|r| r["request_id"].as_str().expect("request_id is a string"))
                .collect();
            assert_eq!(still, vec!["real"], "live ids stay listed as still_running");
            let unknown: Vec<&str> = output["unknown_request_ids"]
                .as_array()
                .expect("unknown_request_ids is an array")
                .iter()
                .map(|v| v.as_str().expect("id is a string"))
                .collect();
            let mut got = unknown.clone();
            got.sort_unstable();
            assert_eq!(
                got,
                vec!["typo-a", "typo-b"],
                "typo'd ids must surface on the interrupted-wait report"
            );
        }
        other => unreachable!("expected an interrupted-wait report, got {other:?}"),
    }
}

/// Scope `body` inside a `TURN_CONTEXT` for `session`, the way
/// `ScopedToolService::execute` does in production — which is where the wait's
/// steer watch reads its session from.
async fn in_turn_on<T>(session: &crate::routing::session_key::SessionKey, body: T) -> T::Output
where
    T: std::future::Future,
{
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    TURN_CONTEXT
        .scope(
            TurnContext {
                session_key: session.clone(),
                run_id: String::new(),
                channel_id: String::new(),
                conversation_id: String::new(),
                caller_role: None,
                channel_tool_permissions: None,
                unattended: false,
            },
            body,
        )
        .await
}

/// Round-10 — a parked `wait` must also observe a **mid-loop steer**, not only
/// the cancel token (codex `WaitOutcome::Steered` parity).
///
/// The message is already durably in the session log and the caller was told
/// the send succeeded; what the steer cannot do on its own is shorten this
/// park, and the park is the turn. Without the arm this call sits for its full
/// 600 s window while the user's correction goes unread — no error, no failing
/// test, just an agent that ignores its user for ten minutes.
///
/// Asserted against a wall clock and on the **consumer** end (the wait really
/// came back, and came back saying *why*): "the signal is published" is exactly
/// the kind of wiring that can be present and still not connected. Deleting the
/// `steer.steered()` arm from `loop_tool.rs` makes this hang to the 10 s bound
/// and fail.
#[tokio::test]
async fn wait_returns_promptly_when_the_user_steers() {
    use crate::routing::session_key::SessionKey;

    let session = SessionKey::peer("main", "steer-wakes-wait");
    let tracker = make_tracker();
    register_owned(&tracker, "slow-child", "sess-steer");
    let tool = tool_for_session(tracker, "sess-steer");

    let steered = session.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        crate::session::steer_signal::note_steer(&steered);
    });

    let started = std::time::Instant::now();
    // Bounded, not merely measured: with the arm removed this call parks for
    // its full 600 s, and a test that WEDGES for ten minutes costs the whole
    // suite its signal — a red test has to be red quickly.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        in_turn_on(
            &session,
            tool.execute(
                json!({ "action": "wait", "request_id": "slow-child", "timeout_secs": 600 }),
                CancellationToken::new(),
            ),
        ),
    )
    .await
    .expect("a steered wait must not run out its window");
    let elapsed = started.elapsed();
    assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");
    match result {
        // Success, not Error: nothing failed and nothing was cancelled.
        ToolResult::Success { output } => {
            assert_eq!(
                output["status"], "wait_interrupted_by_user",
                "a steer and a stop ask for opposite things, so they must not \
                 report the same status"
            );
            assert_eq!(output["still_running"][0]["request_id"], "slow-child");
        }
        other => unreachable!("expected a steered-wait report, got {other:?}"),
    }
}

/// The fan-out arm of the same rule: `wait` over a SET is a second `select!`,
/// and "I added the arm to the one I was looking at" is how the second half of
/// a two-site fix goes missing.
#[tokio::test]
async fn wait_any_returns_promptly_when_the_user_steers() {
    use crate::routing::session_key::SessionKey;

    let session = SessionKey::peer("main", "steer-wakes-wait-any");
    let tracker = make_tracker();
    register_owned(&tracker, "child-a", "sess-steer-many");
    register_owned(&tracker, "child-b", "sess-steer-many");
    let tool = tool_for_session(tracker, "sess-steer-many");

    let steered = session.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        crate::session::steer_signal::note_steer(&steered);
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        in_turn_on(
            &session,
            tool.execute(
                json!({
                    "action": "wait",
                    "request_ids": ["child-a", "child-b"],
                    "timeout_secs": 600,
                }),
                CancellationToken::new(),
            ),
        ),
    )
    .await
    .expect("a steered fan-out wait must not run out its window");
    match result {
        ToolResult::Success { output } => {
            assert_eq!(output["status"], "wait_interrupted_by_user");
            let mut still: Vec<&str> = output["still_running"]
                .as_array()
                .expect("still_running is an array")
                .iter()
                .map(|r| r["request_id"].as_str().expect("request_id is a string"))
                .collect();
            still.sort_unstable();
            assert_eq!(
                still,
                vec!["child-a", "child-b"],
                "a steer stops the WAIT, never the children"
            );
        }
        other => unreachable!("expected a steered-wait report, got {other:?}"),
    }
}

/// A steer on somebody else's session must not cut this wait short. The watch
/// is keyed by the turn's session, and a shared process-global registry is
/// exactly where that keying quietly degrades into "wake everyone".
#[tokio::test]
async fn a_steer_on_another_session_does_not_cut_this_wait_short() {
    use crate::routing::session_key::SessionKey;

    let mine = SessionKey::peer("main", "steer-scoped-mine");
    let theirs = SessionKey::peer("main", "steer-scoped-theirs");
    let tracker = make_tracker();
    register_owned(&tracker, "slow-child", "sess-steer-scoped");
    let tool = tool_for_session(tracker, "sess-steer-scoped");

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        crate::session::steer_signal::note_steer(&theirs);
    });

    // Short window: the assertion is that we reach the TIMEOUT report rather
    // than the steered one, so the window has to be short enough to wait out.
    let result = in_turn_on(
        &mine,
        tool.execute(
            json!({ "action": "wait", "request_id": "slow-child", "timeout_secs": 1 }),
            CancellationToken::new(),
        ),
    )
    .await;
    match result {
        ToolResult::Success { output } => assert_eq!(
            output["status"], "still_running",
            "another session's steer must leave this wait alone"
        ),
        other => unreachable!("expected a timed-out wait report, got {other:?}"),
    }
}

/// An **inert** watch — no `TURN_CONTEXT`, i.e. every non-gateway run (cron,
/// internal, tests) — must leave the park alone.
///
/// The inert arm is `pending()`, and the tempting one-character alternative
/// ("nothing to watch, so treat it as already steered") would make every
/// headless `wait` return instantly with a report about a user who does not
/// exist, turning the fan-out drain loop into a hot loop. Cheap to write, silent
/// to miss, so it gets its own test rather than a comment.
#[tokio::test]
async fn an_unscoped_wait_still_parks_for_its_window() {
    let tracker = make_tracker();
    register_owned(&tracker, "slow-child", "sess-steer-inert");
    let tool = tool_for_session(tracker, "sess-steer-inert");

    // No `in_turn_on`: this is the shape a cron / internal run has.
    let result = tool
        .execute(
            json!({ "action": "wait", "request_id": "slow-child", "timeout_secs": 1 }),
            CancellationToken::new(),
        )
        .await;
    match result {
        ToolResult::Success { output } => assert_eq!(
            output["status"], "still_running",
            "an unscoped wait must time out normally, not report a steer"
        ),
        other => unreachable!("expected a timed-out wait report, got {other:?}"),
    }
}

/// Cancelling a background sub-agent is the parent deciding its outcome, so the
/// proactive announce must stay quiet about it — otherwise a whole fresh parent
/// turn is spent reporting the death of a child the parent itself ordered.
#[tokio::test]
async fn cancel_marks_the_pending_outcome_consumed() {
    let tracker = make_tracker();
    register_owned(&tracker, "doomed", "sess-cancel-2");
    let tool = tool_for_session(tracker.clone(), "sess-cancel-2");

    let out = tool
        .execute(
            json!({ "action": "cancel", "request_id": "doomed" }),
            CancellationToken::new(),
        )
        .await;
    match out {
        ToolResult::Success { output } => assert_eq!(output["status"], "cancelling"),
        other => unreachable!("expected a cancelling report, got {other:?}"),
    }

    // The child unwinds a moment later, exactly as `spawn_background` does.
    tracker.mark_completed(
        "doomed",
        CompletedOutcome::Err("sub-agent failed: cancelled".to_string()),
    );
    assert!(
        tracker.is_consumed("doomed"),
        "the announce guard must already be set when the cancelled child lands"
    );
}

/// `list` is a directory of THIS session's sub-agents. Unscoped it handed the
/// model live request_ids from every other session — ids it could then
/// `check_status` (reading foreign output) or `cancel`.
#[tokio::test]
async fn list_shows_only_this_sessions_subagents() {
    let tracker = make_tracker();
    for (id, session) in [
        ("mine-live", "sess-a"),
        ("mine-done", "sess-a"),
        ("theirs-live", "sess-b"),
        ("theirs-done", "sess-b"),
    ] {
        register_owned(&tracker, id, session);
    }
    tracker.mark_completed("mine-done", CompletedOutcome::ok_text("m"));
    tracker.mark_completed("theirs-done", CompletedOutcome::ok_text("t"));

    let tool = tool_for_session(tracker, "sess-a");
    let out = tool
        .execute(json!({ "action": "list" }), CancellationToken::new())
        .await;
    let ToolResult::Success { output } = out else {
        unreachable!("list must succeed");
    };
    assert_eq!(output["running_count"], 1);
    assert_eq!(output["running"][0]["request_id"], "mine-live");
    assert_eq!(output["completed_count"], 1);
    assert_eq!(output["completed"][0]["request_id"], "mine-done");
}

/// `list` rows are summaries. Rendering every retained completion's FULL output
/// let one call swamp the parent's context with material it never asked for.
#[tokio::test]
async fn list_rows_preview_the_result_instead_of_inlining_it() {
    let tracker = make_tracker();
    let huge = "x".repeat(50_000);
    register_owned(&tracker, "big", "sess-big");
    tracker.mark_completed("big", CompletedOutcome::ok_text(huge.clone()));

    let tool = tool_for_session(tracker, "sess-big");
    let out = tool
        .execute(json!({ "action": "list" }), CancellationToken::new())
        .await;
    let ToolResult::Success { output } = out else {
        unreachable!("list must succeed");
    };
    let row = &output["completed"][0];
    let preview = row["result_preview"].as_str().unwrap();
    assert!(
        preview.chars().count() <= 201,
        "the row must preview, not inline ({} chars)",
        preview.chars().count()
    );
    // …and it must say how much was withheld, so the model can decide whether
    // to fetch the rest with check_status.
    assert_eq!(row["result_chars"], huge.chars().count());
}

/// Re-issuing the same `request_ids` drains the fan-out one completion at a
/// time and then terminates. Returning the first completed id regardless of
/// delivery made that loop spin forever, one LLM turn per lap.
#[tokio::test]
async fn wait_on_a_set_drains_then_reports_all_delivered() {
    let tracker = make_tracker();
    for id in ["p1", "p2"] {
        register_owned(&tracker, id, "sess-fan");
        tracker.mark_completed(id, CompletedOutcome::ok_text(id));
    }
    let tool = tool_for_session(tracker, "sess-fan");
    let args = json!({
        "action": "wait",
        "request_ids": ["p1", "p2"],
        "timeout_secs": 1
    });

    let mut seen = Vec::new();
    for _ in 0..2 {
        let ToolResult::Success { output } =
            tool.execute(args.clone(), CancellationToken::new()).await
        else {
            unreachable!("each drain step must succeed");
        };
        assert_eq!(output["status"], "completed");
        seen.push(output["request_id"].as_str().unwrap().to_string());
    }
    seen.sort();
    assert_eq!(seen, vec!["p1".to_string(), "p2".to_string()]);

    let ToolResult::Success { output } = tool.execute(args, CancellationToken::new()).await else {
        unreachable!("the terminal step must succeed");
    };
    assert_eq!(
        output["status"], "all_delivered",
        "a drained set must terminate instead of repeating a delivered result"
    );
}

/// A typo'd id used to park the full window and report only on the ids that
/// resolved, so it looked exactly like a slow sub-agent.
#[tokio::test]
async fn wait_names_request_ids_it_has_never_heard_of() {
    let tracker = make_tracker();
    register_owned(&tracker, "real", "sess-unknown");
    tracker.mark_completed("real", CompletedOutcome::ok_text("done"));
    let tool = tool_for_session(tracker, "sess-unknown");

    let ToolResult::Success { output } = tool
        .execute(
            json!({
                "action": "wait",
                "request_ids": ["real", "typo-id"],
                "timeout_secs": 1
            }),
            CancellationToken::new(),
        )
        .await
    else {
        unreachable!("wait must succeed");
    };
    assert_eq!(output["status"], "completed");
    assert_eq!(output["unknown_request_ids"][0], "typo-id");
}

/// Round-8 — a `wait` over a set that is *entirely* unknown must surface
/// every id in the error message. The previous bool-shaped `NotFound`
/// returned `"None of the given request_ids matches ..."` with no list,
/// and the model could only diagnose the typo by trying one id at a
/// time. This test pins the new contract: a fully-unknown set is still
/// an error (the wait had nothing to wait for) but the error names the
/// bad ids.
#[tokio::test]
async fn wait_with_all_unknown_request_ids_lists_them_in_the_error() {
    let tracker = make_tracker();
    let tool = tool_for_session(tracker, "sess-all-unknown");

    let result = tool
        .execute(
            json!({
                "action": "wait",
                "request_ids": ["typo-a", "typo-b"],
                "timeout_secs": 1
            }),
            CancellationToken::new(),
        )
        .await;
    match result {
        ToolResult::Error { error, .. } => {
            // Both ids must appear in the human-readable error string.
            assert!(
                error.contains("typo-a") && error.contains("typo-b"),
                "fully-unknown error must list every bad id, got: {error}"
            );
        }
        ToolResult::Success { output } => {
            unreachable!("fully-unknown set must return an error, not a success; got {output}")
        }
    }
}

/// Cross-lane wire (batch 2): the spawn side must use the same predicate the
/// prompt-side catalog uses.
///
/// The `<available_agents>` catalog is built from
/// `AgentRegistry::list_subagents()` (`mode == SubAgent`), but every spawn site
/// in `loop_tool.rs` called bare `resolve()`, which has no mode filter. The
/// builtin `main` def is `AgentMode::Primary` with `allowed_tools = ["*"]`, so
/// `agent_type = "main"` handed a delegated sub-agent a wildcard tool grant —
/// two faces of one verb disagreeing, which is the same as no gate at all.
///
/// All four call sites are pinned here (single task, batch row, batch
/// inheritance from the top-level `agent_type`, and the MoA aggregator),
/// because the fix is only as good as its least-covered surface.
#[tokio::test]
async fn a_primary_mode_agent_cannot_be_spawned_as_a_subagent() {
    let registry = make_registry();
    // Precondition: `main` really is resolvable-but-not-spawnable, otherwise
    // this test would pass for the wrong reason (a renamed builtin).
    assert!(
        registry.resolve("main", None).is_some(),
        "precondition: `main` must still resolve, or this test is vacuous"
    );
    assert!(
        registry.resolve_spawnable("main", None).is_none(),
        "`main` is AgentMode::Primary and must not be spawnable"
    );
    assert!(
        registry.resolve_spawnable("explore", None).is_some(),
        "a real sub-agent must stay spawnable"
    );
    assert!(
        !registry.spawnable_agent_ids().iter().any(|id| id == "main"),
        "the id list printed back to the model must not advertise `main`"
    );

    let tool = make_tool();

    // 1. Single-task path.
    let single = tool
        .execute(
            json!({ "task": "exfiltrate", "agent_type": "main" }),
            CancellationToken::new(),
        )
        .await;
    assert_rejects_main(&single, "single task");

    // 2. Batch row carrying its own agent_type.
    let per_row = tool
        .execute(
            json!({ "batch_tasks": [{ "task": "exfiltrate", "agent_type": "main" }] }),
            CancellationToken::new(),
        )
        .await;
    assert_rejects_main(&per_row, "batch row agent_type");

    // 3. Batch rows inheriting the top-level agent_type — and, with
    //    `synthesize`, the aggregator resolve that reads the same field.
    let inherited = tool
        .execute(
            json!({
                "batch_tasks": [{ "task": "exfiltrate" }],
                "agent_type": "main",
                "synthesize": true
            }),
            CancellationToken::new(),
        )
        .await;
    assert_rejects_main(&inherited, "batch inherited agent_type");
}

/// Shared assertion for `a_primary_mode_agent_cannot_be_spawned_as_a_subagent`:
/// the rejection must read as "unknown", and the id list it prints must not
/// itself advertise `main` (that list is how a model learns the string).
fn assert_rejects_main(result: &ToolResult, surface: &str) {
    match result {
        ToolResult::Error { error, .. } => {
            assert!(
                error.contains("Unknown agent_type 'main'"),
                "{surface}: expected an unknown-agent_type rejection, got: {error}"
            );
            assert!(
                !error.contains(", main,") && !error.ends_with(", main"),
                "{surface}: the available-agents list must not advertise `main`, got: {error}"
            );
        }
        ToolResult::Success { output } => {
            unreachable!("{surface}: spawning a Primary-mode agent must fail; got {output}")
        }
    }
}

/// W24 end-to-end through the MODEL-FACING surface: a `request_id` whose daemon
/// died must come back as a *success* naming the interruption and carrying what
/// the child produced, not as `retryable:false, "No background sub-agent found"`
/// — a message that cannot tell a typo apart from a restart and throws the
/// partial work away.
///
/// Drives `check_status` (not the persistence module directly), because the
/// module could work perfectly and the tool still return the old error: the
/// not-found arm is the wire.
#[tokio::test]
async fn check_status_reports_a_restart_orphan_instead_of_an_unknown_id() {
    use crate::agents::background_persistence as bp;

    // The sidecar root is process-global, so hold the same gate the module's
    // own tests take before pointing it anywhere.
    let _gate = bp::test_gate();
    let tmp = tempfile::tempdir().unwrap();
    // A previous daemon incarnation registered a background child and got some
    // way into it before dying.
    bp::enable_for_test(tmp.path().to_path_buf());
    bp::record_start("req-restart", "s-mine", "audit the crate", "explore");
    bp::record_activity("req-restart", "grepped 41 files, three suspects left");
    bp::disable_for_test();
    // ...and this process boots against the same store.
    bp::init_and_reconcile(tmp.path().to_path_buf());

    let tool = SubagentTool::new(
        Arc::new(MockAiProvider) as Arc<dyn AiProvider>,
        crate::harness::chain_context::ChainContext::new(),
        make_registry(),
        make_tracker(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
    .with_parent_session_id("s-mine".to_string());

    let result = tool
        .execute(
            json!({ "action": "check_status", "request_id": "req-restart" }),
            CancellationToken::new(),
        )
        .await;

    let ToolResult::Success { output } = result else {
        bp::disable_for_test();
        unreachable!("a restart orphan is not a failure of this call: {result:?}");
    };
    assert_eq!(output["status"], "interrupted_by_restart");
    assert_eq!(output["task"], "audit the crate");
    assert!(
        output["partial_result"]
            .as_str()
            .unwrap_or_default()
            .contains("three suspects left"),
        "the child's work must reach the model: {output}"
    );

    // A genuinely unknown id still reads as an error — the sidecar must not
    // turn every typo into a plausible-looking orphan.
    let unknown = tool
        .execute(
            json!({ "action": "check_status", "request_id": "req-typo" }),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(unknown, ToolResult::Error { .. }),
        "an id nobody ever heard of must still be an error: {unknown:?}"
    );

    // ...and another session still cannot read this one out of the sidecar.
    let stranger = SubagentTool::new(
        Arc::new(MockAiProvider) as Arc<dyn AiProvider>,
        crate::harness::chain_context::ChainContext::new(),
        make_registry(),
        make_tracker(),
        in_mem_session(),
        Arc::new(NoopTestToolService),
    )
    .with_parent_session_id("s-other".to_string())
    .execute(
        json!({ "action": "check_status", "request_id": "req-restart" }),
        CancellationToken::new(),
    )
    .await;
    bp::disable_for_test();
    assert!(
        matches!(stranger, ToolResult::Error { .. }),
        "the sidecar must be scoped like the tracker: {stranger:?}"
    );
}

/// W27 — the operator's cap must actually reach the semaphore a run fans out
/// through. Asserted on `available_permits` of a freshly built tool: dropping
/// the `types::max_concurrent_subagents()` read from `SubagentTool::new` leaves
/// every clamp test green and the knob inert.
#[test]
#[serial_test::serial(subagent_concurrency_cap)]
fn a_new_tool_fans_out_at_the_configured_concurrency() {
    use crate::agents::subagent_tool::{max_concurrent_subagents, set_max_concurrent_subagents};

    let restore = max_concurrent_subagents();
    // Deliberately not the default, so "it happens to be 4" cannot pass.
    let widened = set_max_concurrent_subagents(9);
    let tool = make_tool();
    let observed = tool.subagent_semaphore.available_permits();
    set_max_concurrent_subagents(restore);

    assert_eq!(
        observed, widened,
        "the run's concurrency semaphore must be sized by [execution] max_concurrent_subagents"
    );
}

// ---------------------------------------------------------------------------
// `context` / `fork_turns` — the per-call starting-context axis
// ---------------------------------------------------------------------------

fn run_args_of(v: serde_json::Value) -> super::types::RunArgs {
    match parse_args(&v).expect("parses") {
        SubagentAction::Run(args) => args,
        other => unreachable!("expected Run, got {other:?}"),
    }
}

/// Omitted `context` must stay `None` — that is what defers to the target
/// agent's `context_mode`, and it is what every call written before the
/// argument existed sends.
#[test]
fn an_omitted_context_defers_to_the_agent_default() {
    assert!(run_args_of(json!({ "task": "t" })).spawn_context.is_none());
    // An explicit JSON null is "absent", matching the rest of this parser:
    // schema-completing providers emit it for properties they are not using.
    assert!(run_args_of(json!({ "task": "t", "context": null }))
        .spawn_context
        .is_none());
}

#[test]
fn each_accepted_context_value_parses_to_its_mode() {
    use crate::agents::SpawnContext;
    assert_eq!(
        run_args_of(json!({ "task": "t", "context": "isolated" })).spawn_context,
        Some(SpawnContext::Isolated)
    );
    assert_eq!(
        run_args_of(json!({ "task": "t", "context": "summary" })).spawn_context,
        Some(SpawnContext::Summary)
    );
    assert_eq!(
        run_args_of(json!({ "task": "t", "context": "fork" })).spawn_context,
        Some(SpawnContext::Fork { turns: None })
    );
    // Case-insensitive: a model that types `Isolated` meant `isolated`, and
    // rejecting that would be pedantry with a real cost (a wasted turn).
    assert_eq!(
        run_args_of(json!({ "task": "t", "context": "ISOLATED" })).spawn_context,
        Some(SpawnContext::Isolated)
    );
}

/// A misspelled `context` is REJECTED, never quietly defaulted.
///
/// This is the whole reason the parse returns an error instead of an
/// `unwrap_or_default`: falling back would give the caller a child running
/// under a context policy it did not choose and cannot observe — and the case
/// that matters most is a reviewer the caller believes is isolated, silently
/// reading the parent's framing. The error names the accepted set so the next
/// turn gets it right.
#[test]
fn a_misspelled_context_is_rejected_with_the_accepted_set() {
    let err = parse_args(&json!({ "task": "t", "context": "isolate" }))
        .expect_err("a typo must not fall back to the default");
    assert!(err.contains("isolate"), "{err}");
    for accepted in crate::agents::SpawnContext::ACCEPTED {
        assert!(
            err.contains(accepted),
            "error must list '{accepted}': {err}"
        );
    }

    // Wrong JSON type is the same class of mistake.
    assert!(parse_args(&json!({ "task": "t", "context": 3 })).is_err());
}

/// `fork_turns` only means anything under `context="fork"`. A silently ignored
/// argument is how a caller ends up believing it bounded something it did not.
#[test]
fn fork_turns_is_rejected_where_it_would_do_nothing() {
    for ctx in [json!("isolated"), json!("summary")] {
        let err = parse_args(&json!({ "task": "t", "context": ctx, "fork_turns": 3 }))
            .expect_err("fork_turns outside a fork must be rejected, not dropped");
        assert!(err.contains("fork_turns"), "{err}");
    }
    // ...including when `context` was omitted entirely: the agent default is
    // never `fork` (there is deliberately no such `ContextMode`), so this can
    // only be a mistake.
    assert!(parse_args(&json!({ "task": "t", "fork_turns": 3 })).is_err());
}

#[test]
fn fork_turns_bounds_the_fork_and_rejects_zero() {
    use crate::agents::SpawnContext;
    assert_eq!(
        run_args_of(json!({ "task": "t", "context": "fork", "fork_turns": 4 })).spawn_context,
        Some(SpawnContext::Fork { turns: Some(4) })
    );
    // Zero turns is "carry nothing", which already has a spelling.
    let err = parse_args(&json!({ "task": "t", "context": "fork", "fork_turns": 0 }))
        .expect_err("zero turns must be rejected");
    assert!(
        err.contains("isolated"),
        "the error must name the way to say \"carry nothing\": {err}"
    );
}

/// The schema's `enum` and the parser's accepted set are two halves of one
/// contract, and `SpawnContext::ACCEPTED` is meant to be the single source of
/// both. Pin that they actually agree — otherwise the model is offered a value
/// the parser rejects, or a working value is never advertised.
#[test]
fn the_schema_enum_matches_the_accepted_context_values() {
    let schema = make_tool().schema();
    let advertised: Vec<String> = schema["properties"]["context"]["enum"]
        .as_array()
        .expect("context advertises an enum")
        .iter()
        .map(|v| v.as_str().expect("enum members are strings").to_string())
        .collect();
    let accepted: Vec<String> = crate::agents::SpawnContext::ACCEPTED
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    assert_eq!(advertised, accepted);
}
