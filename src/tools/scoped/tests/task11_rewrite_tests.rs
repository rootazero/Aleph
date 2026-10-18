use super::*;

#[cfg(unix)]
use crate::extension::hooks::HookExecutor;
#[cfg(unix)]
use crate::extension::{HookEvent, HookKind};
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sandbox::exec_approval::ApprovalAction;
use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use crate::tools::service::RefusedBy;
use crate::tools::turn_context::TurnContext;
use serde_json::{json, Value};
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

struct CountedTool {
    name: &'static str,
    confirmation: bool,
    calls: Arc<AtomicUsize>,
    inputs: Arc<Mutex<Vec<Value>>>,
}

#[async_trait::async_trait]
impl LoopTool for CountedTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Task11-D counted tool"
    }

    fn schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, input: Value, _cancel: CancellationToken) -> LoopToolResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inputs.lock().unwrap().push(input.clone());
        LoopToolResult::Success { output: input }
    }

    fn concurrency_claim(&self, input: &Value) -> crate::tools::concurrency::ConcurrencyClaim {
        use crate::tools::concurrency::ConcurrencyClaim;
        if self.name == "file_ops" && input.get("operation").and_then(Value::as_str) == Some("list")
        {
            ConcurrencyClaim::Shared
        } else {
            ConcurrencyClaim::global()
        }
    }

    fn requires_confirmation(&self) -> bool {
        self.confirmation
    }
}

fn counted_registry(
    name: &'static str,
    confirmation: bool,
) -> (
    Arc<LoopToolRegistry>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<Value>>>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let mut registry = LoopToolRegistry::new();
    registry.register(Box::new(CountedTool {
        name,
        confirmation,
        calls: calls.clone(),
        inputs: inputs.clone(),
    }));
    (Arc::new(registry), calls, inputs)
}

fn unique_context(name: &str) -> TurnContext {
    let mut ctx = turn_ctx(name);
    ctx.caller_role = Some("guest".to_string());
    ctx
}

struct RecordingRequester {
    answers: Mutex<VecDeque<ApprovalOutcome>>,
    seen: Mutex<Vec<ApprovalAction>>,
}

impl RecordingRequester {
    fn new(answers: impl IntoIterator<Item = ApprovalOutcome>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl ApprovalRequester for RecordingRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse {
        self.seen.lock().unwrap().push(action.clone());
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ApprovalOutcome::Denied)
            .into()
    }
}

#[cfg(unix)]
fn rewrite_hook(final_input: Value) -> Arc<HookExecutor> {
    let encoded = serde_json::to_string(&final_input).unwrap();
    Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        &format!("printf '%s\\n' 'update_input: {encoded}'"),
    )]))
}

#[tokio::test]
#[cfg(unix)]
async fn task11_file_ops_rewrite_rechecks_final_claim_before_handler() {
    let (registry, calls, inputs) = counted_registry("file_ops", false);
    let original = json!({"operation": "list", "path": "/tmp/task11"});
    let final_input = json!({"operation": "delete", "path": "/tmp/task11"});
    let original_copy = original.clone();
    let svc = ScopedToolService::new(registry, BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Plan)
        .with_turn_context(unique_context("task11-file-ops-rewrite"))
        .with_hook_executor(rewrite_hook(final_input.clone()), "task11-file-ops-rewrite");

    let (rewritten, effective) = svc
        .execute_with_cancel_effective("file_ops", original, CancellationToken::new())
        .await;
    match rewritten {
        Err(ToolError::PermissionDenied { name, reason }) => {
            assert_eq!(name, "file_ops");
            assert!(
                reason.contains("PLANNING") && reason.contains("does not run yet"),
                "rewritten write must name the Plan gate: {reason}"
            );
        }
        other => panic!("rewritten exclusive operation must be Plan-denied: {other:?}"),
    }
    assert_eq!(effective, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        original_copy,
        json!({"operation": "list", "path": "/tmp/task11"})
    );
    assert!(inputs.lock().unwrap().is_empty());

    let (control_registry, control_calls, control_inputs) = counted_registry("file_ops", false);
    let control = ScopedToolService::new(control_registry, BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Plan)
        .with_turn_context(unique_context("task11-file-ops-control"));
    control
        .execute(
            "file_ops",
            json!({"operation": "list", "path": "/tmp/task11"}),
        )
        .await
        .expect("unrewritten read-only operation is the control");
    assert_eq!(control_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        control_inputs.lock().unwrap().as_slice(),
        &[json!({"operation": "list", "path": "/tmp/task11"})]
    );
}

#[tokio::test]
#[cfg(unix)]
async fn task11_config_rewrite_records_original_and_final_approval_actions() {
    let (registry, calls, _inputs) = counted_registry("cron_manage", false);
    let requester = RecordingRequester::new([ApprovalOutcome::Approved, ApprovalOutcome::Denied]);
    let svc = ScopedToolService::new(registry, BTreeSet::new())
        .with_turn_context(unique_context("task11-config-rewrite-denied"))
        .with_config_approval(requester.clone())
        .with_hook_executor(
            rewrite_hook(json!({"operation": "delete", "job": "rewritten"})),
            "task11-config-rewrite-denied",
        );
    let (result, effective) = svc
        .execute_with_cancel_effective(
            "cron_manage",
            json!({"operation": "list", "job": "original"}),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(ToolError::PermissionDenied { .. })));
    assert_eq!(effective, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    {
        let seen = requester.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_ne!(seen[0].grant_key, seen[1].grant_key);
        assert!(seen[0].summary.contains("original"));
        assert!(seen[1].summary.contains("rewritten"));
    }

    let (approved_registry, approved_calls, approved_inputs) =
        counted_registry("cron_manage", false);
    let approved_requester =
        RecordingRequester::new([ApprovalOutcome::Approved, ApprovalOutcome::Approved]);
    let approved_final = json!({"operation": "delete", "job": "final"});
    let approved = ScopedToolService::new(approved_registry, BTreeSet::new())
        .with_turn_context(unique_context("task11-config-final-approved"))
        .with_config_approval(approved_requester.clone())
        .with_hook_executor(
            rewrite_hook(approved_final.clone()),
            "task11-config-final-approved",
        );
    let (result, effective) = approved
        .execute_with_cancel_effective(
            "cron_manage",
            json!({"operation": "list", "job": "original"}),
            CancellationToken::new(),
        )
        .await;
    result.expect("approved final action reaches the handler");
    assert_eq!(effective, Some(approved_final.clone()));
    assert_eq!(approved_calls.load(Ordering::SeqCst), 1);
    let approved_seen = approved_requester.seen.lock().unwrap();
    assert_eq!(approved_seen.len(), 2);
    assert_ne!(approved_seen[0].grant_key, approved_seen[1].grant_key);
    assert!(approved_seen[0].summary.contains("original"));
    assert!(approved_seen[1].summary.contains("final"));
    assert_eq!(
        approved_inputs.lock().unwrap().as_slice(),
        &[approved_final]
    );
}

#[tokio::test]
#[cfg(unix)]
async fn task11_confirmation_rewrite_requires_final_bytes_and_approval() {
    let (registry, calls, _inputs) = counted_registry("file_ops", true);
    let svc = ScopedToolService::new(registry, BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Auto)
        .with_turn_context(unique_context("task11-confirm-no-channel"))
        .with_hook_executor(
            rewrite_hook(json!({"operation": "delete", "path": "/tmp/final"})),
            "task11-confirm-no-channel",
        );
    let (result, effective) = svc
        .execute_with_cancel_effective(
            "file_ops",
            json!({"operation": "list", "path": "/tmp/original"}),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(ToolError::Refused {
            by: RefusedBy::NobodyAsked,
            ..
        })
    ));
    assert_eq!(effective, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let (registry, calls, inputs) = counted_registry("file_ops", true);
    let requester = RecordingRequester::new([ApprovalOutcome::Approved, ApprovalOutcome::Approved]);
    let final_input = json!({"operation": "delete", "path": "/tmp/final-approved"});
    let svc = ScopedToolService::new(registry, BTreeSet::new())
        .with_exec_tier(crate::config::types::policies::ExecTier::Auto)
        .with_turn_context(unique_context("task11-confirm-final-approved"))
        .with_confirmation(requester)
        .with_hook_executor(
            rewrite_hook(final_input.clone()),
            "task11-confirm-final-approved",
        );
    let (result, effective) = svc
        .execute_with_cancel_effective(
            "file_ops",
            json!({"operation": "list", "path": "/tmp/original"}),
            CancellationToken::new(),
        )
        .await;
    result.expect("approved final action reaches the handler");
    assert_eq!(effective, Some(final_input.clone()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(inputs.lock().unwrap().as_slice(), &[final_input]);
}

#[tokio::test]
#[cfg(unix)]
async fn task11_hook_ask_without_transport_is_nobody_asked() {
    let (registry, calls, _inputs) = counted_registry("plain", false);
    let hook = Arc::new(HookExecutor::new(vec![make_command_hook(
        HookEvent::BeforeToolCall,
        HookKind::Interceptor,
        "printf '%s\\n' 'ask: task11 requires a person'",
    )]));
    let svc = ScopedToolService::new(registry, BTreeSet::new())
        .with_turn_context(unique_context("task11-hook-ask-no-transport"))
        .with_hook_executor(hook, "task11-hook-ask-no-transport");
    let err = svc
        .execute("plain", json!({"benign": true}))
        .await
        .expect_err("Ask without an approval transport is fail-closed");
    assert!(matches!(
        err,
        ToolError::Refused {
            by: RefusedBy::NobodyAsked,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
