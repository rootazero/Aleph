//! The read-only planning floor, at the enforcement chokepoint.
//!
//! Its own file rather than more of `tests.rs` (already 3k lines): these ask
//! three questions and the third is the one the feature exists for.
//!
//!   1. does the floor hold — and hold **above** an explicit `allow`?
//!   2. does it hold on the argument-dependent tools a name rule cannot see?
//!   3. does an approved handoff lift it for **this** run, not the next one?

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::config::types::policies::{plan_phase::HANDOFF_ACTION, ExecTier, PlanPhase};
use crate::extension::PermissionAction;
use crate::sandbox::exec_approval::gate::{ApprovalOutcome, ApprovalRequester, ApprovalResponse};
use crate::sync_primitives::Arc;
use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult as LoopToolResult};
use crate::tools::service::ToolService;

use super::{PlanGate, ScopedToolService};

/// A tool that declares nothing but its name — which is all the floor reads
/// (declaring nothing means "not idempotent", the fail-closed side).
struct NamedStub(String);

#[async_trait::async_trait]
impl LoopTool for NamedStub {
    fn name(&self) -> &str {
        &self.0
    }
    fn description(&self) -> &str {
        "stub"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        LoopToolResult::Success { output: json!({}) }
    }
}

/// Records every approval request and answers with a fixed outcome.
struct Requester {
    outcome: ApprovalOutcome,
    calls: std::sync::atomic::AtomicUsize,
    seen: std::sync::Mutex<Vec<crate::sandbox::exec_approval::ApprovalAction>>,
}

impl Requester {
    fn new(outcome: ApprovalOutcome) -> Self {
        Self {
            outcome,
            calls: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ApprovalRequester for Requester {
    async fn request_approval(
        &self,
        action: &crate::sandbox::exec_approval::ApprovalAction,
    ) -> ApprovalResponse {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(action.clone());
        self.outcome.into()
    }
}

/// Registry over the names these tests assert about: two wholly-refused, two
/// argument-dependent, two always-admitted.
fn registry() -> Arc<LoopToolRegistry> {
    let mut r = LoopToolRegistry::new();
    for name in [
        "bash",
        "file_write",
        "file_ops",
        "scratchpad",
        "file_read",
        "ask_user",
    ] {
        r.register(Box::new(NamedStub(name.to_string())));
    }
    Arc::new(r)
}

fn turn_ctx(agent: &str) -> crate::tools::turn_context::TurnContext {
    crate::tools::turn_context::TurnContext {
        session_key: crate::routing::session_key::SessionKey::main(agent),
        run_id: String::new(),
        channel_id: "telegram".to_string(),
        conversation_id: "c1".to_string(),
        caller_role: None,
        channel_tool_permissions: None,
        unattended: false,
    }
}

/// A service whose run started in the planning phase.
///
/// `ExecTier::Full` throughout, on purpose: the tier that documents itself as
/// gating nothing must not be able to speak over the floor. If these tests
/// passed at `Ask` they would prove nothing.
fn planning_svc() -> (ScopedToolService, Arc<PlanGate>) {
    let gate = Arc::new(PlanGate::planning(None));
    let svc = ScopedToolService::new(registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_plan_gate(Arc::clone(&gate));
    (svc, gate)
}

#[test]
fn the_floor_denies_mutating_tools_at_the_chokepoint() {
    let (svc, _gate) = planning_svc();
    for name in ["bash", "file_write"] {
        assert_eq!(
            svc.permission_for(name),
            PermissionAction::Deny,
            "{name} must be denied while planning, even at exec_tier=full"
        );
    }
    for name in ["file_read", "ask_user", "scratchpad", "file_ops"] {
        assert_ne!(
            svc.permission_for(name),
            PermissionAction::Deny,
            "{name} must stay reachable while planning"
        );
    }
}

#[test]
fn an_explicit_allow_does_not_outrank_the_floor() {
    use crate::config::types::policies::ToolPermissionsConfig;

    // The configuration that makes the floor a promise rather than a default:
    // the operator has explicitly allowed `bash`. Everywhere else in this
    // subsystem an explicit entry wins.
    let mut perms = ToolPermissionsConfig::default();
    perms
        .overrides
        .insert("bash".to_string(), PermissionAction::Allow);

    let svc = ScopedToolService::new(registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_tool_permissions(perms.clone())
        .with_plan_gate(Arc::new(PlanGate::planning(None)));
    assert_eq!(svc.permission_for("bash"), PermissionAction::Deny);

    // Control: same entry, no gate. Without this the assertion above could be
    // passing because the config was ignored rather than outranked.
    let building = ScopedToolService::new(registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_tool_permissions(perms);
    assert_eq!(building.permission_for("bash"), PermissionAction::Allow);
}

#[tokio::test]
async fn planning_refuses_a_destructive_file_ops_call_but_runs_a_search() {
    let (svc, _gate) = planning_svc();
    // The same tool, two verdicts. A name-keyed rule cannot express this,
    // which is why the floor is asked again with the arguments.
    let err = svc
        .execute("file_ops", json!({"operation": "delete", "path": "/tmp/x"}))
        .await
        .expect_err("delete must not run while planning");
    let rendered = err.to_string();
    assert!(
        rendered.contains("read-only planning phase"),
        "refusal must name the phase, got: {rendered}"
    );
    assert!(
        rendered.contains(HANDOFF_ACTION),
        "refusal must name the exit, got: {rendered}"
    );

    svc.execute(
        "file_ops",
        json!({"operation": "search", "query": "fn main"}),
    )
    .await
    .expect("a search only observes");
}

#[tokio::test]
async fn an_approved_handoff_lifts_the_floor_inside_the_same_run() {
    let requester = Arc::new(Requester::new(ApprovalOutcome::Approved));
    let gate = Arc::new(PlanGate::planning(None));
    let svc = ScopedToolService::new(registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_turn_context(turn_ctx("agent-plan-handoff"))
        .with_confirmation(Arc::clone(&requester) as _)
        .with_plan_gate(Arc::clone(&gate));

    assert_eq!(gate.phase(), PlanPhase::Planning);
    assert_eq!(svc.permission_for("bash"), PermissionAction::Deny);

    svc.execute("scratchpad", json!({"action": HANDOFF_ACTION}))
        .await
        .expect("an approved handoff runs");

    // Asked exactly once, and the card offered no standing grant.
    assert_eq!(requester.calls.load(Ordering::SeqCst), 1);
    let offered = requester.seen.lock().unwrap()[0].allowed_decisions.clone();
    assert_eq!(
        offered,
        crate::exec::allowed_decisions::once_only(),
        "a plan is approved once; a standing grant would consent to plans not yet written"
    );

    // The whole feature: unlocked in THIS run. A floor that only lifted on the
    // next user message would turn one gesture into two.
    assert_eq!(gate.phase(), PlanPhase::Building);
    assert_eq!(svc.permission_for("bash"), PermissionAction::Allow);
    svc.execute("bash", json!({"command": "echo hi"}))
        .await
        .expect("execution is unlocked");
}

/// A tool that records what `current_plan_phase()` said from inside its own
/// body — i.e. after every gate in `execute_inner` has run. The registry takes
/// a `Box`, so the recording buffer is shared out of band.
struct PhaseProbe {
    seen: Arc<std::sync::Mutex<Vec<PlanPhase>>>,
}

#[async_trait::async_trait]
impl LoopTool for PhaseProbe {
    fn name(&self) -> &str {
        "scratchpad"
    }
    fn description(&self) -> &str {
        "probe"
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    async fn execute(&self, _input: Value, _cancel: CancellationToken) -> LoopToolResult {
        self.seen
            .lock()
            .unwrap()
            .push(crate::tools::turn_context::current_plan_phase());
        LoopToolResult::Success { output: json!({}) }
    }
}

#[tokio::test]
async fn the_handoff_tool_body_sees_the_floor_already_lifted() {
    // The tool that reports "the user approved" vs "this session was never
    // planning" reads the phase from a task-local. If that task-local carried a
    // SNAPSHOT taken when `execute` was entered, it would say `Planning` at the
    // exact moment that stopped being true — and the model would receive the
    // tool's "still read-only, wait for the user" alongside the dispatch
    // layer's "approved, execution unlocked", in the same result.
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut reg = LoopToolRegistry::new();
    reg.register(Box::new(PhaseProbe {
        seen: Arc::clone(&seen),
    }));

    let requester = Arc::new(Requester::new(ApprovalOutcome::Approved));
    let gate = Arc::new(PlanGate::planning(None));
    let svc = ScopedToolService::new(Arc::new(reg), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_turn_context(turn_ctx("agent-phase-probe"))
        .with_confirmation(Arc::clone(&requester) as _)
        .with_plan_gate(Arc::clone(&gate));

    svc.execute("scratchpad", json!({"action": HANDOFF_ACTION}))
        .await
        .expect("an approved handoff runs");

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[PlanPhase::Building],
        "the handoff tool body must see the floor already lifted"
    );
}

#[tokio::test]
async fn a_refused_handoff_leaves_the_floor_engaged() {
    let requester = Arc::new(Requester::new(ApprovalOutcome::Denied));
    let gate = Arc::new(PlanGate::planning(None));
    let svc = ScopedToolService::new(registry(), BTreeSet::new())
        .with_exec_tier(ExecTier::Full)
        .with_turn_context(turn_ctx("agent-plan-refused"))
        .with_confirmation(Arc::clone(&requester) as _)
        .with_plan_gate(Arc::clone(&gate));

    svc.execute("scratchpad", json!({"action": HANDOFF_ACTION}))
        .await
        .expect_err("a refused handoff does not run");
    assert_eq!(gate.phase(), PlanPhase::Planning);
    assert_eq!(svc.permission_for("bash"), PermissionAction::Deny);
}

#[tokio::test]
async fn an_unattended_run_cannot_hand_itself_off() {
    // No approval requester wired = no channel to ask on, which is what an
    // unattended continuation looks like from here. Nobody is reading the plan,
    // so the handoff must fail closed rather than assume consent.
    let (svc, gate) = planning_svc();
    svc.execute("scratchpad", json!({"action": HANDOFF_ACTION}))
        .await
        .expect_err("no approval channel means no handoff");
    assert_eq!(gate.phase(), PlanPhase::Planning);
}

#[tokio::test]
async fn ordinary_scratchpad_work_needs_no_approval_while_planning() {
    // The plan writer must not be gated by the gate that exists to make the
    // plan readable — otherwise writing the plan costs an approval per step.
    let requester = Arc::new(Requester::new(ApprovalOutcome::Approved));
    let (svc, _gate) = planning_svc();
    let svc = svc
        .with_turn_context(turn_ctx("agent-plan-write"))
        .with_confirmation(Arc::clone(&requester) as _);

    svc.execute("scratchpad", json!({"action": "set_plan", "items": ["a"]}))
        .await
        .expect("writing the plan is planning");
    assert_eq!(requester.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_wholly_refused_tool_is_hidden_and_reappears_after_the_release() {
    // Hiding, not refusing, for the wholly-refused class: a model that can see
    // `file_write` in its list will reach for it and read the refusal as an
    // obstacle to route around. The argument-dependent ones stay visible
    // because they have admissible calls.
    let (svc, gate) = planning_svc();
    let names = |svc: &ScopedToolService| -> Vec<String> {
        svc.metadata_schema()
            .iter()
            .map(|d| d.name.clone())
            .collect()
    };

    let before = names(&svc);
    assert!(!before.contains(&"bash".to_string()));
    assert!(!before.contains(&"file_write".to_string()));
    assert!(before.contains(&"file_ops".to_string()));
    assert!(before.contains(&"scratchpad".to_string()));

    // The surface must rebuild after a release. A schema cache still serving
    // the planning list would tell the model the approval did nothing.
    gate.release().await.expect("release");
    let after = names(&svc);
    assert!(after.contains(&"bash".to_string()));
    assert!(after.contains(&"file_write".to_string()));
}
