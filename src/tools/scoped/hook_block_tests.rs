//! A BeforeToolCall hook that BLOCKS (exit 2, `decision: "block"`, `block:`)
//! reaches the model as a policy refusal: kind `permission` whatever the hook
//! author wrote on stderr, the reason verbatim, and no route to another tool.
//!
//! Driven through the real dispatch (`ScopedToolService::execute` running a
//! real shell hook). The model-facing text is what
//! `harness::agent::act::compose_tool_error_msg` renders for a non-`NotFound`
//! error: the error's `Display` followed by `render_persistence_hint`.

use super::*;
use crate::extension::hooks::HookExecutor;
use crate::extension::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};
use crate::session::events::{now_ms, SessionEvent, SessionEventRecord};
use crate::tools::attempt_summary::render_run_summary;
use crate::tools::error_kind::{classify_error_str, ToolErrorKind};
use crate::tools::fallback_registry::render_persistence_hint;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// A File-family tool, so the ladder has a rung to name: an `Execution`
/// failure of `file_read` is answered with `switch=file_ops`. `Some(error)`
/// makes it fail with that error; `None` returns the file's content.
struct FileRead(Option<&'static str>);

#[async_trait::async_trait]
impl LoopTool for FileRead {
    fn name(&self) -> &str {
        "file_read"
    }
    fn description(&self) -> &str {
        "reads a file"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        match self.0 {
            None => LoopToolResult::Success {
                output: json!("SECRET=1"),
            },
            Some(error) => LoopToolResult::Error {
                error: error.to_string(),
                retryable: false,
            },
        }
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

fn service(tool: FileRead, hooks: Vec<HookConfig>) -> ScopedToolService {
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(tool));
    ScopedToolService::new(Arc::new(registry), BTreeSet::new())
        .with_hook_executor(Arc::new(HookExecutor::new(hooks)), "test-session")
}

/// Run `file_read` under one BeforeToolCall hook, plus a PermissionDenied
/// observer that records whether it fired (into `marker`).
async fn blocked_by(command: &str, marker: &Path) -> ToolError {
    let mut probe = command_hook(
        HookEvent::PermissionDenied,
        HookKind::Observer,
        &format!("cat > '{}'", marker.display()),
    );
    probe.matcher = Some("file_read".to_string());
    let block = command_hook(HookEvent::BeforeToolCall, HookKind::Interceptor, command);
    service(FileRead(None), vec![block, probe])
        .execute("file_read", json!({ "path": ".env" }))
        .await
        .expect_err("the hook blocks the call")
}

/// What the model reads for this failed call (see the module doc).
fn model_facing(err: &ToolError) -> String {
    format!("{err}{}", render_persistence_hint(err, "file_read"))
}

fn assert_policy_refusal(err: &ToolError, reason: &str, marker: &Path) {
    let text = model_facing(err);
    assert_eq!(err.kind(), ToolErrorKind::Permission, "{text}");
    assert!(!err.kind().is_transient(), "{text}");
    assert!(text.contains(reason), "the hook's reason, verbatim: {text}");
    assert!(
        text.contains("refused by a policy hook"),
        "says a policy hook refused the call: {text}"
    );
    assert!(
        !text.contains("switch="),
        "no route to another tool: {text}"
    );
    assert!(!text.contains("ladder"), "no ladder doctrine: {text}");
    // The persisted string is re-read by the run-level attempt summary.
    assert_eq!(
        classify_error_str(&text),
        ToolErrorKind::Permission,
        "{text}"
    );
    assert!(
        !matches!(err, ToolError::PermissionDenied { .. }),
        "a block is not a PermissionDenied: {err:?}"
    );
    assert!(
        !marker.exists(),
        "a hook block does not fire the PermissionDenied observer"
    );
}

#[tokio::test]
async fn an_exit_2_worded_like_an_upstream_404_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let reason = "HTTP 404: .env not found upstream";
    let err = blocked_by(&format!("echo '{reason}' >&2; exit 2"), &marker).await;
    assert_policy_refusal(&err, reason, &marker);
}

#[tokio::test]
async fn an_exit_2_worded_like_a_timeout_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let reason = "tool file_read timed out after 5000ms";
    let err = blocked_by(&format!("echo '{reason}' >&2; exit 2"), &marker).await;
    assert_policy_refusal(&err, reason, &marker);
}

#[tokio::test]
async fn an_exit_2_with_nothing_on_stderr_is_the_same_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let err = blocked_by("exit 2", &marker).await;
    assert_policy_refusal(&err, crate::extension::hooks::EXIT2_DEFAULT_REASON, &marker);
}

#[tokio::test]
async fn a_json_block_decision_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let reason = "429 rate limit on secrets";
    let err = blocked_by(
        &format!(r#"echo '{{"decision":"block","reason":"{reason}"}}'"#),
        &marker,
    )
    .await;
    assert_policy_refusal(&err, reason, &marker);
}

#[tokio::test]
async fn a_block_line_is_a_policy_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("denied.json");
    let reason = "503 service unavailable";
    let err = blocked_by(&format!("echo 'block: {reason}'"), &marker).await;
    assert_policy_refusal(&err, reason, &marker);
}

/// The control: a genuine execution failure (no hook) keeps the kind its
/// error text says and the ladder hint that goes with it.
#[tokio::test]
async fn a_genuine_execution_failure_keeps_its_kind_and_its_hint() {
    let err = service(FileRead(Some("HTTP 404 page not found")), Vec::new())
        .execute("file_read", json!({ "path": "missing.txt" }))
        .await
        .expect_err("the tool fails");
    let text = model_facing(&err);
    assert!(matches!(err, ToolError::Execution { .. }), "{err:?}");
    assert_eq!(err.kind(), ToolErrorKind::UpstreamNotFound, "{text}");
    assert!(text.contains("switch=file_ops"), "{text}");
    assert!(
        text.contains("doctrine=ladder_must_be_climbed_before_fail"),
        "{text}"
    );
}

fn record(seq: u64, event: SessionEvent) -> SessionEventRecord {
    SessionEventRecord {
        seq,
        event,
        created_at_ms: now_ms(),
    }
}

/// The failed calls as the transcript persists them: a request, then the
/// model-facing error text.
fn failures(errors: &[String]) -> Vec<SessionEventRecord> {
    let mut events = Vec::new();
    for (i, error) in errors.iter().enumerate() {
        let call_id = format!("c{i}");
        let seq = u64::try_from(i).unwrap() * 2;
        events.push(record(
            seq + 1,
            SessionEvent::ToolCallRequested {
                identity: None,
                turn_id: uuid::Uuid::nil(),
                call_id: call_id.clone(),
                name: "file_read".to_string(),
                input: json!({}),
                at: now_ms(),
            },
        ));
        events.push(record(
            seq + 2,
            SessionEvent::ToolError {
                turn_id: uuid::Uuid::nil(),
                call_id,
                error: error.clone(),
                at: now_ms(),
            },
        ));
    }
    events
}

/// The run-level face of the ladder: three blocks by a policy hook are not
/// three failures to climb away from. Three genuine failures still are.
///
/// This pins how the summary reads three block RENDERINGS; it dispatches
/// straight into the service, three times with the same input — a sequence
/// the harness memo refuses after the first in production. That production
/// path is `refusal_tests::repeating_a_hook_blocked_call_is_not_pointed_around_the_hook`.
#[tokio::test]
async fn repeated_hook_blocks_do_not_raise_the_run_level_ladder() {
    let dir = tempfile::tempdir().unwrap();
    let mut blocks = Vec::new();
    for reason in ["HTTP 404 not found", "timed out after 5ms", "cloudflare"] {
        let marker = dir.path().join(format!("{}.json", blocks.len()));
        let err = blocked_by(&format!("echo '{reason}' >&2; exit 2"), &marker).await;
        blocks.push(model_facing(&err));
    }
    assert_eq!(render_run_summary(&failures(&blocks)), None);

    let mut genuine = Vec::new();
    for _ in 0..3 {
        let err = service(FileRead(Some("HTTP 404 page not found")), Vec::new())
            .execute("file_read", json!({}))
            .await
            .expect_err("the tool fails");
        genuine.push(model_facing(&err));
    }
    let summary = render_run_summary(&failures(&genuine)).expect("three genuine failures");
    assert!(
        summary.contains("file_read × 3 (upstream_not_found)"),
        "{summary}"
    );
}
