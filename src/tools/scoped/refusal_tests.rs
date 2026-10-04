//! A refused call — a person declined its card, nobody could be asked, a
//! hook's `ask:` was declined, a policy hook failed to run — reaches the model
//! as a policy refusal: kind `permission`, whatever the refusal's prose says
//! (the user's own words included), and no route to another tool. And when the
//! model repeats a refused call, the harness's cross-batch refusal of the
//! repeat does not hand that route back.
//!
//! Driven through the real dispatch (`ScopedToolService`), and — for the
//! repeat — through the harness's own Act phase (`AgentHarness::act`, two
//! batches), which is the only way production reaches the memo refusal: the
//! memo stops the second call before it is dispatched.

use super::*;
use crate::extension::hooks::HookExecutor;
use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
use crate::harness::{AgentHarness, HarnessDeps, NoopHarnessCallback};
use crate::providers::adapter::NativeToolCall;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;
use crate::session::events::{SessionEvent, SessionEventRecord};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::service::{SessionId, SessionService};
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
use crate::tools::attempt_summary::render_run_summary;
use crate::tools::error_kind::{classify_error_str, ToolErrorKind};
use crate::tools::fallback_registry::render_persistence_hint;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// A File-family tool, so the ladder has a rung to name (`switch=file_ops`).
/// `fail`: the error it fails with; `None` succeeds. `confirm`: declares its
/// own confirmation gate.
struct FileTool {
    name: &'static str,
    fail: Option<&'static str>,
    confirm: bool,
}

#[async_trait::async_trait]
impl LoopTool for FileTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "file stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        match self.fail {
            None => LoopToolResult::Success {
                output: json!("SECRET=1"),
            },
            Some(error) => LoopToolResult::Error {
                error: error.to_string(),
                retryable: false,
            },
        }
    }
    fn requires_confirmation(&self) -> bool {
        self.confirm
    }
}

fn command_hook(event: HookEvent, kind: HookKind, command: &str) -> HookConfig {
    HookConfig {
        event,
        kind,
        priority: HookPriority::Normal,
        matcher: None,
        actions: vec![HookAction::Command {
            command: command.to_string(),
        }],
        plugin_name: "test".to_string(),
        plugin_root: PathBuf::from("/tmp"),
        handler: None,
        timeout_secs: None,
        declared_event: None,
        scope_key: crate::extension::visibility::ScopeKey::Global,
    }
}

/// A PermissionDenied observer for `tool` that records whether it fired.
fn observer_probe(tool: &str, marker: &Path) -> HookConfig {
    let mut probe = command_hook(
        HookEvent::PermissionDenied,
        HookKind::Observer,
        &format!("cat > '{}'", marker.display()),
    );
    probe.matcher = Some(tool.to_string());
    probe
}

/// A person who answers every card with `outcome` and, when declining, says
/// why in words that read like an upstream failure.
struct Person(ApprovalOutcome);

const USER_WORDS: &str = "HTTP 404 not found; it timed out after 5000ms last time";

#[async_trait::async_trait]
impl ApprovalRequester for Person {
    async fn request_approval(&self, _action: &ApprovalAction) -> ApprovalResponse {
        ApprovalResponse {
            outcome: self.0,
            deny_reason: Some(USER_WORDS.to_string()),
        }
    }
}

fn service(tool: FileTool, hooks: Vec<HookConfig>) -> ScopedToolService {
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(tool));
    ScopedToolService::new(Arc::new(registry), BTreeSet::new())
        .with_hook_executor(Arc::new(HookExecutor::new(hooks)), "test-session")
}

/// What the model reads for a failed call: `compose_tool_error_msg`'s
/// `Display` + persistence hint (the harness fn is private; see
/// `hook_block_tests`).
fn model_facing(err: &ToolError, tool: &str) -> String {
    format!("{err}{}", render_persistence_hint(err, tool))
}

fn assert_refusal(err: &ToolError, tool: &str, says: &[&str], marker: &Path) {
    let text = model_facing(err, tool);
    assert_eq!(err.kind(), ToolErrorKind::Permission, "{text}");
    for s in says {
        assert!(text.contains(s), "missing {s:?}: {text}");
    }
    assert!(
        !text.contains("switch="),
        "no route to another tool: {text}"
    );
    assert!(!text.contains("ladder"), "no ladder doctrine: {text}");
    assert_eq!(
        classify_error_str(&text),
        ToolErrorKind::Permission,
        "the persisted text reads back the same: {text}"
    );
    assert!(
        !matches!(err, ToolError::PermissionDenied { .. }),
        "{err:?}"
    );
    assert!(
        !marker.exists(),
        "the PermissionDenied observer did not fire"
    );
}

// -- (A) refusals at the dispatch seam ---------------------------------------

#[tokio::test]
async fn a_card_the_person_declined_is_a_policy_refusal_whatever_they_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let tool = FileTool {
        name: "file_write",
        fail: None,
        confirm: true,
    };
    let err = service(tool, vec![observer_probe("file_write", &marker)])
        .with_confirmation(Arc::new(Person(ApprovalOutcome::Denied)))
        .execute("file_write", json!({ "path": "a.txt" }))
        .await
        .expect_err("the person declined");
    assert_refusal(&err, "file_write", &[USER_WORDS, "declined"], &marker);
}

#[tokio::test]
async fn a_card_nobody_could_be_asked_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let tool = FileTool {
        name: "file_write",
        fail: None,
        confirm: true,
    };
    let err = service(tool, vec![observer_probe("file_write", &marker)])
        .execute("file_write", json!({ "path": "a.txt" }))
        .await
        .expect_err("no approval channel");
    assert_refusal(
        &err,
        "file_write",
        &["No approval channel is available", "not authorized"],
        &marker,
    );
}

#[tokio::test]
async fn a_hook_ask_the_person_declined_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let tool = FileTool {
        name: "file_read",
        fail: None,
        confirm: false,
    };
    let ask = command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "echo 'ask: reading secrets'",
    );
    let err = service(tool, vec![ask, observer_probe("file_read", &marker)])
        .with_confirmation(Arc::new(Person(ApprovalOutcome::Denied)))
        .execute("file_read", json!({ "path": ".env" }))
        .await
        .expect_err("the person declined the hook's question");
    assert_refusal(&err, "file_read", &[USER_WORDS, "declined"], &marker);
}

#[tokio::test]
async fn a_hook_ask_nobody_could_be_asked_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let tool = FileTool {
        name: "file_read",
        fail: None,
        confirm: false,
    };
    let ask = command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "echo 'ask: reading secrets'",
    );
    let err = service(tool, vec![ask, observer_probe("file_read", &marker)])
        .execute("file_read", json!({ "path": ".env" }))
        .await
        .expect_err("no approval channel");
    assert_refusal(
        &err,
        "file_read",
        &["no approval channel is available", "not authorized"],
        &marker,
    );
}

/// M-1: a guard that crashed is not "a policy hook refused" — it still fails
/// closed, as a refusal with no route around it, but it says what happened.
#[tokio::test]
async fn a_policy_hook_that_failed_to_run_says_so_and_still_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let tool = FileTool {
        name: "file_read",
        fail: None,
        confirm: false,
    };
    let mut slow = command_hook(HookEvent::BeforeToolCall, HookKind::Interceptor, "sleep 5");
    slow.timeout_secs = Some(1);
    let err = service(tool, vec![slow, observer_probe("file_read", &marker)])
        .execute("file_read", json!({ "path": ".env" }))
        .await
        .expect_err("a failed interceptor blocks fail-closed");
    assert_refusal(&err, "file_read", &["failed to run"], &marker);
    assert!(
        !err.to_string().contains("refused by a policy hook"),
        "a crashed guard is not a policy verdict: {err}"
    );
}

// -- I-1: the harness's refusal of an identical repeat ------------------------

async fn fresh_session() -> (Arc<dyn SessionService>, SessionId) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let svc: Arc<dyn SessionService> = Arc::new(InProcessActorSessionService::new(store));
    let sid = crate::routing::session_key::SessionKey::ephemeral("refusal-repeat");
    svc.attach(sid.clone()).await.unwrap();
    (svc, sid)
}

fn harness(session: Arc<dyn SessionService>, tools: ScopedToolService) -> AgentHarness {
    AgentHarness::new(HarnessDeps {
        tool_descriptor_lookup: None,
        session,
        tools: Arc::new(tools),
        llm: Arc::new(crate::providers::mock::MockProvider::new("idle")),
        robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        preflight_pipeline: None,
        trace_sink: None,
        system_prompt: None,
        system_prompt_parts: None,
        recall_context: None,
        guardrails: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        turn_budget: None,
        result_store: None,
        session_epoch_registrar: None,
        tool_signal_sink: std::sync::Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink),
        in_flight_tool_calls: None,
        parallel_tool_concurrency: None,
    })
}

/// `batches` Think→Act batches, each ONE identical `file_read` of `.env`;
/// returns the run's events and the model-facing error of each batch.
async fn identical_batches(tools: ScopedToolService, batches: usize) -> Vec<SessionEventRecord> {
    let (session, sid) = fresh_session().await;
    let h = harness(Arc::clone(&session), tools);
    let cancel = CancellationToken::new();
    for i in 0..batches {
        let call = NativeToolCall {
            thought_signature: None,
            id: format!("c{i}"),
            name: "file_read".to_string(),
            arguments: json!({ "path": ".env" }),
        };
        h.act(
            &sid,
            uuid::Uuid::new_v4(),
            vec![call],
            &mut NoopHarnessCallback,
            i,
            &cancel,
        )
        .await
        .expect("a failed call does not abort the run");
    }
    session.get_events(&sid, None, None).await.unwrap()
}

fn tool_errors(events: &[SessionEventRecord]) -> Vec<String> {
    events
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::ToolError { error, .. } => Some(error.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn repeating_a_hook_blocked_call_is_not_pointed_around_the_hook() {
    let block = command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "echo 'HTTP 404 not found' >&2; exit 2",
    );
    let tool = FileTool {
        name: "file_read",
        fail: None,
        confirm: false,
    };
    let events = identical_batches(service(tool, vec![block]), 4).await;
    let errors = tool_errors(&events);
    assert_eq!(errors.len(), 4, "{errors:#?}");
    // Batches 2..4 never reach the hook: the harness refuses the repeat.
    for repeat in &errors[1..] {
        assert!(
            repeat.contains("already failed earlier in the run"),
            "the memo refusal: {repeat}"
        );
        assert!(!repeat.contains("switch="), "{repeat}");
        assert!(!repeat.contains("ladder"), "{repeat}");
        assert!(!repeat.contains("try a different tool"), "{repeat}");
    }
    // One block and three refused repeats are not four failures to climb from.
    assert_eq!(render_run_summary(&events), None);
}

/// The control: a genuine failure keeps its own hint on the first error, and
/// its identical repeats are counted in the run summary as what they repeat.
#[tokio::test]
async fn repeating_a_genuine_failure_still_counts_as_that_failure() {
    let tool = FileTool {
        name: "file_read",
        fail: Some("HTTP 404 page not found"),
        confirm: false,
    };
    let events = identical_batches(service(tool, Vec::new()), 3).await;
    let errors = tool_errors(&events);
    assert_eq!(errors.len(), 3, "{errors:#?}");
    assert!(errors[0].contains("switch=file_ops"), "{}", errors[0]);
    let summary = render_run_summary(&events).expect("three failures of one kind");
    assert!(
        summary.contains("file_read × 3 (upstream_not_found)"),
        "{summary}"
    );
}
