//! End-to-end capability approval flow.
//!
//! Covers the spec §8 six-step pipeline with real `ApprovalGate` routing,
//! proving that capability escalations (network, subprocess-spawn) are
//! denied unless the approval path approves them. These tests drive the
//! sandbox through its public `Sandbox` trait + `build_sandbox` factory —
//! no internal mocks, no crate-private items.
//!
//! Spec: `docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md` §8.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use alephcore::agent_loop::exec_approval::{
    ApprovalConfig, ApprovalGate, ApprovalOutcome, ApprovalRequester,
};
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::{
    build_sandbox, NetworkPolicy, OsSandboxDriverTrait, OsSandboxProfile, Sandbox,
    SandboxCapabilities, SandboxCommand, SandboxConfig, SandboxError, SandboxOutput,
};

/// Recording driver — every `run` call increments `run_count` and returns a
/// canned successful `SandboxOutput`. Used so tests can assert whether the
/// pipeline reached the OS driver stage (step 5) without touching the real
/// `sandbox-exec` binary.
struct RecordingDriver {
    run_count: Arc<RwLock<u32>>,
}

impl RecordingDriver {
    fn new() -> (Arc<Self>, Arc<RwLock<u32>>) {
        let run_count = Arc::new(RwLock::new(0u32));
        let driver = Arc::new(Self {
            run_count: run_count.clone(),
        });
        (driver, run_count)
    }
}

#[async_trait]
impl OsSandboxDriverTrait for RecordingDriver {
    fn profile_for(
        &self,
        _capabilities: &SandboxCapabilities,
        _cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        Ok(OsSandboxProfile {
            contents: String::new(),
        })
    }

    async fn run(
        &self,
        _program: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<&[u8]>,
        _cwd: &Path,
        _profile: &OsSandboxProfile,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        *self.run_count.write().await += 1;
        Ok(SandboxOutput {
            stdout: b"ok".to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            signal: None,
            truncated: false,
            duration_ms: 1,
        })
    }
}

/// Approval requester that returns a fixed outcome and counts invocations.
/// This is the seam `ApprovalGate` uses to ask the user (step 3 in the
/// pipeline).
struct FixedRequester {
    outcome: ApprovalOutcome,
    calls: Arc<RwLock<u32>>,
}

impl FixedRequester {
    fn new(outcome: ApprovalOutcome) -> (Self, Arc<RwLock<u32>>) {
        let calls = Arc::new(RwLock::new(0u32));
        (
            Self {
                outcome,
                calls: calls.clone(),
            },
            calls,
        )
    }
}

#[async_trait]
impl ApprovalRequester for FixedRequester {
    async fn request_approval(&self, _tool_name: &str, _reason: &str) -> ApprovalOutcome {
        *self.calls.write().await += 1;
        self.outcome
    }
}

/// Compose a ready-to-use `Arc<dyn Sandbox>` via the public factory and
/// return the handles needed for assertions.
fn build_test_sandbox(
    outcome: ApprovalOutcome,
) -> (
    Arc<dyn Sandbox>,
    Arc<RwLock<u32>>, // driver run_count
    Arc<RwLock<u32>>, // approval call count
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = SandboxConfig {
        workspace_root: tmp.path().to_path_buf(),
        enabled: true,
        default_timeout_seconds: 30,
        max_output_bytes: 4096,
    };
    let (driver, run_count) = RecordingDriver::new();
    let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver;
    let (requester, calls) = FixedRequester::new(outcome);
    let gate = Arc::new(ApprovalGate::new(
        ApprovalConfig::default(),
        Some(Box::new(requester)),
    ));
    let sandbox = build_sandbox(&cfg, driver_trait, gate);
    (sandbox, run_count, calls, tmp)
}

/// Single session id reused across a test — `SessionKey::ephemeral` mints a
/// fresh UUID per call, so reusing one instance is required for session-
/// scoped assertions (cache hits, granted elevations).
fn test_session() -> SessionKey {
    SessionKey::ephemeral("capability-approval-integration")
}

fn strict_cmd(session: SessionKey) -> SandboxCommand {
    SandboxCommand {
        session_id: session,
        program: "echo".into(),
        args: vec!["hi".into()],
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities::strict(),
        timeout: None,
    }
}

fn network_cmd(session: SessionKey) -> SandboxCommand {
    SandboxCommand {
        session_id: session,
        program: "curl".into(),
        args: vec!["https://example.com".into()],
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        },
        timeout: None,
    }
}

fn spawn_cmd(session: SessionKey) -> SandboxCommand {
    SandboxCommand {
        session_id: session,
        program: "bash".into(),
        args: vec!["-c".into(), "true".into()],
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities {
            spawn_subprocess: true,
            ..SandboxCapabilities::strict()
        },
        timeout: None,
    }
}

#[tokio::test]
async fn strict_capabilities_execute_without_approval() {
    // Strict caps are always ⊆ baseline, so the pipeline should skip step 3
    // (approval) entirely and reach the driver with an exit code of 0.
    let (sandbox, runs, approvals, _tmp) = build_test_sandbox(ApprovalOutcome::Denied);

    let output = sandbox
        .execute(strict_cmd(test_session()))
        .await
        .expect("strict command should execute without approval");

    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        *runs.read().await,
        1,
        "driver should be invoked exactly once for a strict-cap command"
    );
    assert_eq!(
        *approvals.read().await,
        0,
        "approval gate must not be consulted when capabilities are within baseline"
    );
}

#[tokio::test]
async fn elevated_network_triggers_approval_and_proceeds_on_approve() {
    // Network escalation is outside the strict baseline, so step 3 must
    // consult the gate. Approval → driver runs.
    let (sandbox, runs, approvals, _tmp) = build_test_sandbox(ApprovalOutcome::Approved);

    let output = sandbox
        .execute(network_cmd(test_session()))
        .await
        .expect("approved network cap should execute");

    assert_eq!(output.exit_code, Some(0));
    assert_eq!(*approvals.read().await, 1, "approval should be requested");
    assert_eq!(*runs.read().await, 1, "driver should run after approval");
}

#[tokio::test]
async fn elevated_subprocess_denied_by_approval_returns_error() {
    // Subprocess spawn is outside the strict baseline. Gate denies → the
    // sandbox must refuse with `CapabilityDenied` and never call the driver.
    let (sandbox, runs, approvals, _tmp) = build_test_sandbox(ApprovalOutcome::Denied);

    let err = sandbox
        .execute(spawn_cmd(test_session()))
        .await
        .expect_err("denied spawn capability must surface an error");

    assert!(
        matches!(err, SandboxError::CapabilityDenied { .. }),
        "expected CapabilityDenied, got {err:?}"
    );
    assert_eq!(
        *approvals.read().await,
        1,
        "approval gate must be consulted for elevated caps"
    );
    assert_eq!(
        *runs.read().await,
        0,
        "driver must not run when approval is denied"
    );
}

#[tokio::test]
async fn approval_outcome_is_cached_per_session() {
    // First elevated call triggers approval; second call with same cap in
    // same session uses the cached grant and does not ask again.
    let (sandbox, runs, approvals, _tmp) = build_test_sandbox(ApprovalOutcome::Approved);

    let session = test_session();

    sandbox
        .execute(network_cmd(session.clone()))
        .await
        .expect("first elevated call should execute");
    assert_eq!(*approvals.read().await, 1, "first call asks for approval");

    sandbox
        .execute(network_cmd(session))
        .await
        .expect("second elevated call should reuse cached grant");
    assert_eq!(
        *approvals.read().await,
        1,
        "cached grant must suppress the second approval request"
    );
    assert_eq!(
        *runs.read().await,
        2,
        "both calls must reach the driver"
    );
}
