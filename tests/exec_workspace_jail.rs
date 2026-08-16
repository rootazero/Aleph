//! Where a shell call lands — asserted with the tool layer and the sandbox in
//! **one process**.
//!
//! That combination is the whole point. For four rounds two subsystems answered
//! "where does this session work" differently and nothing noticed, because every
//! test drove one half with a stand-in for the other:
//!
//! * sandbox tests built a `SandboxCommand` by hand — no tool, so nothing ever
//!   put a path into `working_dir`;
//! * tool tests ran against a fake sandbox — a `working_dir` went in, but no
//!   containment check ever read it.
//!
//! Meanwhile the gateway stamped the run's effective workspace into every
//! `bash` call that omitted `working_dir`, and `WorkspaceSandbox` jailed the
//! session to `workspaces/<sha256(session)[..16]>`. A 32-hex directory is never
//! an agent id nor a project path, so on a default install (`[sandbox] enabled`
//! is `true` out of the box, and disabling it swaps in a `NoopSandbox` that
//! refuses everything) *every* such call died with
//! `Capability denied: cwd outside workspace root`.
//!
//! The driver here records the cwd it is handed. That is the effect, not the
//! call: the path the OS driver receives IS the child's working directory, so a
//! test that reads it cannot pass while the child runs somewhere else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use alephcore::builtin_tools::bash_exec::{BashExecArgs, BashExecTool};
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::exec_approval::{ApprovalGate, ApprovalRequester, ApprovalResponse};
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
use alephcore::sandbox::{
    build_sandbox, OsSandboxDriverTrait, OsSandboxProfile, Sandbox, SandboxCapabilities,
    SandboxConfig, SandboxError, SandboxOutput,
};
use alephcore::AlephTool;

/// Records every cwd the sandbox hands down, in order. Never spawns anything —
/// the question under test is *which directory*, which is settled before a
/// child would exist, and a recording driver keeps the test hermetic and
/// identical on all three platforms.
struct CwdRecordingDriver {
    seen: Arc<RwLock<Vec<PathBuf>>>,
}

#[async_trait]
impl OsSandboxDriverTrait for CwdRecordingDriver {
    fn platform(&self) -> &'static str {
        "test/cwd-recording"
    }

    fn is_supported(&self) -> bool {
        true
    }

    fn profile_for(
        &self,
        _capabilities: &SandboxCapabilities,
        _cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        Ok(OsSandboxProfile {
            contents: String::new(),
            max_memory_mb: None,
            linux_init_policy: None,
            windows_init_policy: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        _program: &str,
        _args: &[String],
        _env: &HashMap<String, String>,
        _stdin: Option<&[u8]>,
        cwd: &Path,
        _profile: &OsSandboxProfile,
        _timeout: Duration,
        _max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        self.seen.write().await.push(cwd.to_path_buf());
        Ok(SandboxOutput {
            stdout: b"ok".to_vec(),
            exit_code: Some(0),
            duration_ms: 1,
            ..Default::default()
        })
    }
}

/// Approves nothing and is never consulted: every command here stays inside the
/// `strict()` baseline, so step 3 does not run.
struct NeverAsked;

#[async_trait]
impl ApprovalRequester for NeverAsked {
    async fn request_approval(
        &self,
        _action: &alephcore::sandbox::exec_approval::ApprovalAction,
    ) -> ApprovalResponse {
        panic!("no command in this file escalates; the approval gate must not be reached");
    }
}

/// A `bash` tool wired to a real `WorkspaceSandbox` via the public factory —
/// the same composition boot performs.
fn bash_on_real_sandbox() -> (BashExecTool, Arc<RwLock<Vec<PathBuf>>>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = SandboxConfig {
        workspace_root: tmp.path().join("workspaces"),
        enabled: true,
        default_timeout_seconds: 30,
        max_output_bytes: 4096,
        ..Default::default()
    };
    let seen = Arc::new(RwLock::new(Vec::new()));
    let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(CwdRecordingDriver { seen: seen.clone() });
    let gate = Arc::new(ApprovalGate::new(Some(
        Arc::new(NeverAsked) as Arc<dyn ApprovalRequester>
    )));
    let sandbox: Arc<dyn Sandbox> = build_sandbox(
        &cfg,
        driver,
        gate,
        SandboxRateLimitConfig::default(),
        &alephcore::ShellSecurityConfig::default(),
    );
    (BashExecTool::new().with_sandbox(sandbox), seen, tmp)
}

/// Build args the way a tool call actually does — from the JSON the model
/// emits. Every field is `#[serde(default)]`, so an omitted `working_dir` here
/// is byte-for-byte the omission the gateway used to fill in.
fn args(v: serde_json::Value) -> BashExecArgs {
    serde_json::from_value(v).expect("BashExecArgs")
}

fn plain(cmd: &str) -> BashExecArgs {
    args(serde_json::json!({ "cmd": cmd }))
}

/// Run `fut` the way a gateway run does: a session in scope and an authorised
/// workspace published on the channel the model cannot write.
async fn as_run<F: std::future::Future>(workspace: Option<PathBuf>, fut: F) -> F::Output {
    let sid = SessionKey::ephemeral("exec-workspace-jail");
    alephcore::sandbox::context::SESSION_ID
        .scope(
            sid,
            alephcore::sandbox::context::with_exec_workspace(workspace, fut),
        )
        .await
}

/// The flagship. A `bash` call that names no `working_dir` — which is what the
/// model emits for nearly every command — must execute in the directory the run
/// was authorised for, and it must actually reach the driver to do so.
#[tokio::test]
async fn bash_without_a_working_dir_lands_in_the_runs_authorised_workspace() {
    let (bash, seen, tmp) = bash_on_real_sandbox();
    let project = tmp.path().join("some-project");
    std::fs::create_dir_all(&project).expect("mkdir project");

    let out = as_run(Some(project.clone()), bash.call(plain("pwd"))).await;
    let out = out.expect("bash call");

    assert_eq!(
        out.exit_code, 0,
        "the command must run, not be refused. stderr: {}",
        out.stderr
    );
    let seen = seen.read().await;
    assert_eq!(
        seen.len(),
        1,
        "the pipeline must reach the OS driver exactly once"
    );
    assert_eq!(
        seen[0].canonicalize().ok(),
        project.canonicalize().ok(),
        "shell calls must land in the run's authorised workspace, not in a \
         per-session hash directory the prompt never told the model about"
    );
}

/// The jail is still a jail. Widening its ANCHOR to the authorised workspace
/// must not turn it into an allowlist of one plus everything else.
#[tokio::test]
async fn an_explicit_cwd_outside_the_authorised_workspace_is_still_denied() {
    let (bash, seen, tmp) = bash_on_real_sandbox();
    let project = tmp.path().join("some-project");
    let elsewhere = tmp.path().join("not-the-project");
    std::fs::create_dir_all(&project).expect("mkdir project");
    std::fs::create_dir_all(&elsewhere).expect("mkdir elsewhere");

    let call = args(serde_json::json!({
        "cmd": "pwd",
        "working_dir": elsewhere.to_string_lossy(),
    }));
    let out = as_run(Some(project), bash.call(call))
        .await
        .expect("bash call");

    assert_ne!(out.exit_code, 0, "a cwd outside the jail must fail");
    assert!(
        out.stderr.contains("cwd outside workspace root"),
        "the refusal must name the containment check, got: {}",
        out.stderr
    );
    assert!(
        seen.read().await.is_empty(),
        "a denied cwd must never reach the OS driver"
    );
}

/// A relative `working_dir` now means what it says. The removed injection used
/// to *replace* it with the workspace root, so `working_dir: "src"` silently
/// ran in the parent — a wrong directory reported as success.
#[tokio::test]
async fn a_relative_working_dir_resolves_under_the_authorised_workspace() {
    let (bash, seen, tmp) = bash_on_real_sandbox();
    let project = tmp.path().join("some-project");
    let nested = project.join("src");
    std::fs::create_dir_all(&nested).expect("mkdir nested");

    let call = args(serde_json::json!({ "cmd": "pwd", "working_dir": "src" }));
    let out = as_run(Some(project), bash.call(call))
        .await
        .expect("bash call");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    let seen = seen.read().await;
    assert_eq!(
        seen[0].canonicalize().ok(),
        nested.canonicalize().ok(),
        "a relative working_dir must resolve UNDER the authorised root, not be \
         swallowed by it"
    );
}

/// Guards the task-local re-entry in `spawn_background` — behaviourally, not by
/// listing names. Task-locals do not cross `tokio::spawn`, so a forgotten
/// re-entry sends the detached job to a different directory than the foreground
/// call that spawned it, and every single-path test still passes.
#[tokio::test]
async fn a_background_job_lands_in_the_same_directory_as_a_foreground_one() {
    let (bash, seen, tmp) = bash_on_real_sandbox();
    let project = tmp.path().join("some-project");
    std::fs::create_dir_all(&project).expect("mkdir project");

    let out = as_run(Some(project.clone()), async {
        let fg = bash.call(plain("pwd")).await.expect("foreground");
        assert_eq!(fg.exit_code, 0, "stderr: {}", fg.stderr);

        let spawned = bash
            .call(args(
                serde_json::json!({ "cmd": "pwd", "background": true }),
            ))
            .await
            .expect("spawn");
        assert!(
            spawned.stdout.contains(r#""status":"running""#),
            "expected a background handle, got: {}",
            spawned.stdout
        );
        spawned
    })
    .await;
    let _ = out;

    // The detached task runs on its own; wait for its cwd to show up rather
    // than sleeping a fixed amount.
    for _ in 0..200 {
        if seen.read().await.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let seen = seen.read().await;
    assert_eq!(
        seen.len(),
        2,
        "the background job must have reached the driver too"
    );
    assert_eq!(
        seen[1].canonicalize().ok(),
        seen[0].canonicalize().ok(),
        "background and foreground must agree on where this session works"
    );
}

/// What the prompt tells the model must be what the sandbox enforces.
///
/// `SandboxSummary::writable_roots` is rendered into the model's operating
/// envelope every turn. It used to name the `~/.aleph/workspaces` PARENT, which
/// in project mode would tell a model working in `/Users/me/proj` that its own
/// project is off-limits — the same class of lie as the `cwd=` advertisement
/// this file exists to pin, one field over.
#[tokio::test]
async fn the_advertised_writable_root_is_the_one_the_sandbox_enforces() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("some-project");
    std::fs::create_dir_all(&project).expect("mkdir project");

    let cfg = SandboxConfig {
        workspace_root: tmp.path().join("workspaces"),
        enabled: true,
        ..Default::default()
    };
    let seen = Arc::new(RwLock::new(Vec::new()));
    let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(CwdRecordingDriver { seen: seen.clone() });
    let gate = Arc::new(ApprovalGate::new(Some(
        Arc::new(NeverAsked) as Arc<dyn ApprovalRequester>
    )));
    let sandbox: Arc<dyn Sandbox> = build_sandbox(
        &cfg,
        driver,
        gate,
        SandboxRateLimitConfig::default(),
        &alephcore::ShellSecurityConfig::default(),
    );
    let bash = BashExecTool::new().with_sandbox(sandbox.clone());

    let advertised = as_run(Some(project.clone()), async {
        // Read the summary INSIDE the run, which is when prompt assembly reads
        // it; outside one it is the unchanged process-wide default.
        let advertised = sandbox.summary().expect("summary").writable_roots;
        let out = bash.call(plain("pwd")).await.expect("bash call");
        assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
        advertised
    })
    .await;

    let enforced = seen.read().await[0].clone();
    assert_eq!(
        advertised
            .iter()
            .map(|p| p.canonicalize().ok())
            .collect::<Vec<_>>(),
        vec![enforced.canonicalize().ok()],
        "the writable root the prompt advertises must be the directory commands \
         actually run in"
    );
}

/// The fallback contract. Callers with no run in scope — cluster node file
/// commands, direct callers, tests — keep the historical per-session hash
/// directory, so this change is additive for them rather than a redirect.
#[tokio::test]
async fn outside_a_run_the_jail_falls_back_to_the_session_hash_directory() {
    let (bash, seen, tmp) = bash_on_real_sandbox();
    let root = tmp.path().join("workspaces");
    let sid = SessionKey::ephemeral("exec-workspace-jail-fallback");
    let expected = alephcore::sandbox::workspace::session_workspace_dir(&root, &sid);

    let out = alephcore::sandbox::context::SESSION_ID
        .scope(sid, bash.call(plain("pwd")))
        .await
        .expect("bash call");

    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(
        seen.read().await[0].canonicalize().ok(),
        expected.canonicalize().ok(),
        "with no authorised workspace published, the per-session directory is \
         still the jail root"
    );
}
