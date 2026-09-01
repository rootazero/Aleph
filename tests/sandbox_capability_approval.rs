//! End-to-end capability approval flow.
//!
//! Covers the spec §8 six-step pipeline with real `ApprovalGate` routing,
//! proving that capability escalations (network, subprocess-spawn) are
//! denied unless the approval path approves them. These tests drive the
//! sandbox through its public `Sandbox` trait + `build_sandbox` factory —
//! no internal mocks, no crate-private items.
//!
//! Spec: `docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md` §8.

// test-only tuple return type reads clearer inline.
#![allow(clippy::type_complexity)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::exec_approval::{
    ApprovalGate, ApprovalOutcome, ApprovalRequester, ApprovalResponse,
};
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
use alephcore::sandbox::{
    build_sandbox, NetworkPolicy, OsSandboxDriverTrait, OsSandboxProfile, Sandbox,
    SandboxCapabilities, SandboxCommand, SandboxConfig, SandboxError, SandboxOutput,
};

/// Recording driver — every `run` call increments `run_count` and stashes
/// the `profile.max_memory_mb` it received. Tests can assert whether the
/// pipeline reached the OS driver stage (step 5) and what policy fields
/// threaded all the way through, without touching the real `sandbox-exec`.
struct RecordingDriver {
    run_count: Arc<RwLock<u32>>,
    last_max_memory_mb: Arc<RwLock<Option<u64>>>,
    last_network: Arc<RwLock<Option<NetworkPolicy>>>,
}

impl RecordingDriver {
    fn new() -> (
        Arc<Self>,
        Arc<RwLock<u32>>,
        Arc<RwLock<Option<u64>>>,
        Arc<RwLock<Option<NetworkPolicy>>>,
    ) {
        let run_count = Arc::new(RwLock::new(0u32));
        let last_max_memory_mb = Arc::new(RwLock::new(None));
        let last_network = Arc::new(RwLock::new(None));
        let driver = Arc::new(Self {
            run_count: run_count.clone(),
            last_max_memory_mb: last_max_memory_mb.clone(),
            last_network: last_network.clone(),
        });
        (driver, run_count, last_max_memory_mb, last_network)
    }
}

#[async_trait]
impl OsSandboxDriverTrait for RecordingDriver {
    fn platform(&self) -> &'static str {
        "test/recording"
    }

    fn is_supported(&self) -> bool {
        true
    }

    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        _cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        // Thread capabilities.max_memory_mb into the profile so the
        // integration test can verify the foundation layer (S1) wiring.
        // SP-4: also snapshot the network policy so the DNS-resolution
        // integration test can confirm hostnames were pre-resolved before
        // reaching the driver.
        let net_snapshot = capabilities.network.clone();
        let max_mem = capabilities.max_memory_mb;
        let net_slot = self.last_network.clone();
        // try_write avoids the async-in-sync constraint of profile_for; the
        // sandbox calls profile_for once per execute() so the slot has no
        // contention.
        if let Ok(mut slot) = net_slot.try_write() {
            *slot = Some(net_snapshot);
        }
        Ok(OsSandboxProfile {
            contents: String::new(),
            max_memory_mb: max_mem,
            linux_init_policy: None,
            windows_init_policy: None,
        })
    }

    async fn run(
        &self,
        _program: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<&[u8]>,
        _cwd: &Path,
        profile: &OsSandboxProfile,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        *self.run_count.write().await += 1;
        *self.last_max_memory_mb.write().await = profile.max_memory_mb;
        Ok(SandboxOutput {
            stdout: b"ok".to_vec(),
            exit_code: Some(0),
            duration_ms: 1,
            ..Default::default()
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
    async fn request_approval(
        &self,
        _action: &alephcore::sandbox::exec_approval::ApprovalAction,
    ) -> ApprovalResponse {
        *self.calls.write().await += 1;
        // A fixed outcome carries no denial reason; `From<ApprovalOutcome>`
        // wraps it (round-4 widened the trait return to `ApprovalResponse`).
        self.outcome.into()
    }
}

/// Compose a ready-to-use `Arc<dyn Sandbox>` via the public factory and
/// return the handles needed for assertions.
fn build_test_sandbox(
    outcome: ApprovalOutcome,
) -> (
    Arc<dyn Sandbox>,
    Arc<RwLock<u32>>,                   // driver run_count
    Arc<RwLock<u32>>,                   // approval call count
    Arc<RwLock<Option<u64>>>,           // last profile.max_memory_mb seen by driver
    Arc<RwLock<Option<NetworkPolicy>>>, // last capabilities.network seen by driver
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = SandboxConfig {
        workspace_root: tmp.path().to_path_buf(),
        enabled: true,
        default_timeout_seconds: 30,
        max_output_bytes: 4096,
        linux: Default::default(),
        windows: Default::default(),
        rate_limit: Default::default(),
        command_policy: Default::default(),
        ..Default::default()
    };
    let (driver, run_count, last_mem, last_net) = RecordingDriver::new();
    let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver;
    let (requester, calls) = FixedRequester::new(outcome);
    let gate = Arc::new(ApprovalGate::new(Some(
        Arc::from(requester) as Arc<dyn ApprovalRequester>
    )));
    let shell_security = alephcore::ShellSecurityConfig::default();
    let sandbox = build_sandbox(
        &cfg,
        driver_trait,
        gate,
        SandboxRateLimitConfig::default(),
        &shell_security,
    );
    (sandbox, run_count, calls, last_mem, last_net, tmp)
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
        tool_name: "bash".into(),
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
        tool_name: "bash".into(),
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
        tool_name: "bash".into(),
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
    let (sandbox, runs, approvals, _last_mem, _last_net, _tmp) =
        build_test_sandbox(ApprovalOutcome::Denied);

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
    let (sandbox, runs, approvals, _last_mem, _last_net, _tmp) =
        build_test_sandbox(ApprovalOutcome::Approved);

    let output = sandbox
        .execute(network_cmd(test_session()))
        .await
        .expect("approved network cap should execute");

    assert_eq!(output.exit_code, Some(0));
    assert_eq!(*approvals.read().await, 1, "approval should be requested");
    assert_eq!(*runs.read().await, 1, "driver should run after approval");
}

#[tokio::test]
async fn subprocess_spawn_is_gated_exactly_where_the_baseline_says_it_is() {
    // Whether spawning a subprocess is an ESCALATION is a platform decision,
    // and it has exactly one owner: `SandboxCapabilities::session_baseline()`,
    // which admits spawn everywhere except Linux. That is deliberate — the
    // macOS fork ban only ever stopped compound shell commands, never
    // `rm -rf`, so it bought no containment while putting an approval card in
    // front of nearly every ordinary command.
    //
    // This test asks that constant rather than restating `cfg!(target_os)`.
    // The version before it hardcoded the Linux answer, so once the ban was
    // lifted it failed on macOS and Windows while describing itself as an
    // approval-gate defect — the test was the stale half, and a second copy of
    // the platform rule is what let the two disagree.
    let baseline_admits_spawn = SandboxCapabilities::session_baseline().spawn_subprocess;
    let (sandbox, runs, approvals, _last_mem, _last_net, _tmp) =
        build_test_sandbox(ApprovalOutcome::Denied);

    let result = sandbox.execute(spawn_cmd(test_session())).await;

    if baseline_admits_spawn {
        // Within baseline ⇒ step 3 is skipped entirely. Note what is asserted:
        // not merely that it succeeded, but that the gate was never asked —
        // a denial that never runs cannot be mistaken for a grant.
        let output = result.expect("spawn is within this platform's baseline");
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(
            *approvals.read().await,
            0,
            "gate consulted for a capability already inside the baseline"
        );
        assert_eq!(*runs.read().await, 1, "driver should have run");
    } else {
        let err = result.expect_err("denied spawn capability must surface an error");
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
}

#[tokio::test]
async fn approval_outcome_is_cached_per_session() {
    // First elevated call triggers approval; second call with same cap in
    // same session uses the cached grant and does not ask again.
    let (sandbox, runs, approvals, _last_mem, _last_net, _tmp) =
        build_test_sandbox(ApprovalOutcome::Approved);

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
    assert_eq!(*runs.read().await, 2, "both calls must reach the driver");
}

#[tokio::test]
async fn max_memory_mb_threads_capabilities_to_driver_profile() {
    // Foundation S1 guarantee: a per-call max_memory_mb on the caller's
    // SandboxCapabilities must reach the platform driver's OsSandboxProfile
    // unchanged. The driver is then responsible for applying it via the
    // OS-specific mechanism (rlimit on macOS/Linux, JobObject on Windows);
    // here we assert only that the value made the trip.
    let (sandbox, runs, _approvals, last_mem, _last_net, _tmp) =
        build_test_sandbox(ApprovalOutcome::Approved);

    let cmd = SandboxCommand {
        session_id: test_session(),
        tool_name: "bash".into(),
        program: "echo".into(),
        args: vec!["hi".into()],
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities {
            max_memory_mb: Some(128),
            ..SandboxCapabilities::strict()
        },
        timeout: None,
    };

    sandbox
        .execute(cmd)
        .await
        .expect("strict-plus-memory command should execute");
    assert_eq!(*runs.read().await, 1, "driver should be invoked");
    assert_eq!(
        *last_mem.read().await,
        Some(128),
        "profile.max_memory_mb must equal capabilities.max_memory_mb"
    );
}

#[tokio::test]
async fn dns_resolution_threads_resolved_ips_to_driver_profile() {
    // SP-4 guarantee: hostnames in AllowHosts must be pre-resolved to IP
    // literals before the OS driver sees the capabilities. Drivers (Seatbelt
    // `(remote ip ...)`, future iptables, future WFP) all expect IP-only
    // input — the workspace DNS layer is what makes hostnames possible.
    //
    // Uses `localhost` because every CI host has it in /etc/hosts and the
    // system resolver returns 127.0.0.1 / ::1 deterministically.
    let (sandbox, runs, _approvals, _last_mem, last_net, _tmp) =
        build_test_sandbox(ApprovalOutcome::Approved);

    let cmd = SandboxCommand {
        session_id: test_session(),
        tool_name: "bash".into(),
        program: "echo".into(),
        args: vec!["hi".into()],
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: vec!["localhost".into()],
            },
            ..SandboxCapabilities::strict()
        },
        timeout: None,
    };

    sandbox
        .execute(cmd)
        .await
        .expect("AllowHosts(localhost) should execute after DNS resolution");
    assert_eq!(*runs.read().await, 1, "driver should be invoked");

    let net = last_net
        .read()
        .await
        .clone()
        .expect("driver must have seen a network policy");
    match net {
        NetworkPolicy::AllowHosts { hosts } => {
            assert!(
                hosts.iter().any(|h| h == "127.0.0.1" || h == "::1"),
                "expected localhost to be resolved to 127.0.0.1 or ::1, got {hosts:?}"
            );
            assert!(
                !hosts.iter().any(|h| h == "localhost"),
                "hostname must not survive past the DNS layer: {hosts:?}"
            );
        }
        other => panic!("expected AllowHosts after DNS, got {other:?}"),
    }
}
