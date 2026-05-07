//! WorkspaceSandbox — lazy per-session workspace directory + capability enforcement.
//!
//! Implements the `Sandbox` trait by materializing `~/.aleph/workspaces/{hash(session_id)}/`
//! on first exec-class call and routing commands through the 6-step pipeline:
//! 1. resolve session workspace (lazy create dir)
//! 2. validate cwd (None → workspace root; Some(p) must live under root)
//! 3. capability check: within baseline → pass; else consult granted cache then
//!    `ApprovalGate::request_approval_for_tool`
//! 4. `OsSandboxDriverTrait::profile_for`
//! 5. `OsSandboxDriverTrait::run`
//! 6. emit `capability_ledger` tracing audit record

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::exec_approval::gate::{ApprovalGate, ApprovalOutcome};
use crate::sandbox::hooks::{SandboxHookContext, SandboxHookResult, SandboxHooks};
use crate::sandbox::Sandbox;
use crate::session::service::SessionId;

/// Lazy per-session workspace + capability-aware sandbox implementation.
pub struct WorkspaceSandbox {
    workspace_root: PathBuf,
    sessions: Arc<RwLock<HashMap<SessionId, Arc<SessionWorkspace>>>>,
    os_driver: Arc<dyn OsSandboxDriverTrait>,
    approval_gate: Arc<ApprovalGate>,
    default_timeout: Duration,
    max_output_bytes: usize,
    hooks: SandboxHooks,
}

/// Per-session workspace state: cwd on disk plus the capability baseline and
/// the cache of elevations the user already approved this session.
struct SessionWorkspace {
    #[allow(dead_code)]
    session_id: SessionId,
    cwd: PathBuf,
    baseline: SandboxCapabilities,
    granted_elevations: RwLock<HashSet<SandboxCapabilities>>,
}

impl WorkspaceSandbox {
    /// Construct a new `WorkspaceSandbox`. The `workspace_root` is the parent
    /// directory under which per-session workspaces are materialised lazily.
    pub fn new(
        workspace_root: PathBuf,
        os_driver: Arc<dyn OsSandboxDriverTrait>,
        approval_gate: Arc<ApprovalGate>,
        hooks: SandboxHooks,
    ) -> Self {
        Self {
            workspace_root,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            os_driver,
            approval_gate,
            default_timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024, // 1 MB total budget (512 KB per stream)
            hooks,
        }
    }

    /// Override the default 60s per-command timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Override the default 1 MB combined stdout+stderr truncation budget.
    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    /// Override the default hooks (empty by default).
    pub fn with_hooks(mut self, hooks: SandboxHooks) -> Self {
        self.hooks = hooks;
        self
    }

    /// Resolve the per-session workspace, creating the directory on first call.
    /// Idempotent — subsequent calls return the cached `Arc<SessionWorkspace>`.
    async fn for_session(&self, sid: &SessionId) -> Result<Arc<SessionWorkspace>, SandboxError> {
        // Fast path — already provisioned.
        if let Some(ws) = self.sessions.read().await.get(sid).cloned() {
            return Ok(ws);
        }

        // Slow path — take write lock, double-check, then create.
        let mut sessions = self.sessions.write().await;
        if let Some(ws) = sessions.get(sid).cloned() {
            return Ok(ws);
        }

        let cwd = self.workspace_root.join(session_key_to_filename(sid));
        tokio::fs::create_dir_all(&cwd)
            .await
            .map_err(|e| SandboxError::Io(format!("create workspace dir: {e}")))?;

        let ws = Arc::new(SessionWorkspace {
            session_id: sid.clone(),
            cwd,
            baseline: SandboxCapabilities::strict(),
            granted_elevations: RwLock::new(HashSet::new()),
        });
        sessions.insert(sid.clone(), ws.clone());
        Ok(ws)
    }
}

/// Normalize a path, resolving `..` and `.` segments, then check if it stays
/// within the workspace root.
fn normalize_path(path: &std::path::Path, workspace_root: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let base = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };
    let mut normalized = std::path::PathBuf::new();
    for component in base.components() {
        match component {
            Component::Normal(c) => normalized.push(c),
            Component::RootDir => normalized.push("/"),
            Component::CurDir => {} // skip .
            Component::ParentDir => {
                // pop the last component for ..
                normalized.pop();
            }
            _ => {}
        }
    }
    normalized
}

/// Deterministic filesystem-safe directory name derived from a `SessionId`.
///
/// Uses SHA-256 of the JSON-serialised key, truncated to 16 bytes (32 hex
/// chars). Keeps the path short and avoids slashes / special chars that the
/// various `SessionKey` variants may carry.
fn session_key_to_filename(sid: &SessionId) -> String {
    let json = serde_json::to_string(sid).unwrap_or_else(|_| format!("{:?}", sid));
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

#[async_trait]
impl Sandbox for WorkspaceSandbox {
    async fn execute(&self, cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        let ctx = SandboxHookContext::new(&cmd.program, &cmd);

        if let SandboxHookResult::Deny { reason } = self.hooks.run_before(&ctx).await {
            return Err(SandboxError::Other(format!("hook denied: {reason}")));
        }

        let ws = self.for_session(&cmd.session_id).await?;

        let cwd = match &cmd.cwd {
            None => ws.cwd.clone(),
            Some(p) => {
                let normalized = normalize_path(p, &ws.cwd);
                if normalized.starts_with(&ws.cwd) {
                    normalized
                } else {
                    let err = SandboxError::CapabilityDenied {
                        reason: "cwd outside workspace root".into(),
                    };
                    self.hooks
                        .run_after(&ctx, Err("cwd outside workspace root"))
                        .await;
                    return Err(err);
                }
            }
        };

        if !cmd.capabilities.is_within(&ws.baseline) {
            let already_granted = {
                let granted = ws.granted_elevations.read().await;
                granted.iter().any(|g| cmd.capabilities.is_within(g))
            };
            if !already_granted {
                let reason = format_capability_request(&cmd.program, &cmd.capabilities);
                let outcome = self
                    .approval_gate
                    .request_approval_for_tool(&cmd.program, &reason)
                    .await;
                match outcome {
                    ApprovalOutcome::Approved => {
                        ws.granted_elevations
                            .write()
                            .await
                            .insert(cmd.capabilities.clone());
                    }
                    ApprovalOutcome::Denied | ApprovalOutcome::Timeout => {
                        let err = SandboxError::CapabilityDenied {
                            reason: "user denied elevated capability request".into(),
                        };
                        self.hooks.run_after(&ctx, Err("user denied")).await;
                        return Err(err);
                    }
                }
            }
        }

        let profile = self.os_driver.profile_for(&cmd.capabilities, &cwd)?;

        let timeout = cmd.timeout.unwrap_or(self.default_timeout);
        let output = self
            .os_driver
            .run(
                &cmd.program,
                &cmd.args,
                &cmd.env,
                cmd.stdin.as_deref(),
                &cwd,
                &profile,
                timeout,
                self.max_output_bytes,
            )
            .await;

        let outcome = match &output {
            Ok(out) => {
                tracing::info!(
                    target: "capability_ledger",
                    session_id = ?cmd.session_id,
                    program = %cmd.program,
                    caps = ?cmd.capabilities,
                    exit = ?out.exit_code,
                    signal = ?out.signal,
                    duration_ms = out.duration_ms,
                    "sandbox.execute"
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    target: "capability_ledger",
                    session_id = ?cmd.session_id,
                    program = %cmd.program,
                    caps = ?cmd.capabilities,
                    error = %e,
                    "sandbox.execute failed"
                );
                Err("sandbox execution failed")
            }
        };

        self.hooks.run_after(&ctx, outcome).await;

        output
    }
}

/// Format a user-facing capability elevation request string. Used as the
/// `reason` argument to `ApprovalGate::request_approval_for_tool`.
fn format_capability_request(program: &str, caps: &SandboxCapabilities) -> String {
    let mut parts = Vec::new();
    if !caps.fs_read.is_empty() {
        parts.push(format!("fs_read={:?}", caps.fs_read));
    }
    if !caps.fs_write.is_empty() {
        parts.push(format!("fs_write={:?}", caps.fs_write));
    }
    if caps.network != crate::sandbox::capabilities::NetworkPolicy::None {
        parts.push(format!("network={:?}", caps.network));
    }
    if caps.spawn_subprocess {
        parts.push("spawn=true".into());
    }
    format!(
        "{program} requests elevated capabilities: {}",
        parts.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::sandbox::capabilities::NetworkPolicy;
    use crate::sandbox::driver::OsSandboxProfile;
    use crate::sandbox::exec_approval::gate::ApprovalRequester;
    use crate::sandbox::exec_approval::types::ApprovalConfig;

    /// A no-op driver for tests — avoids invoking the real macOS sandbox-exec.
    struct FakeDriver {
        run_count: Arc<RwLock<u32>>,
    }

    impl FakeDriver {
        fn new() -> Self {
            Self {
                run_count: Arc::new(RwLock::new(0)),
            }
        }
    }

    #[async_trait]
    impl OsSandboxDriverTrait for FakeDriver {
        fn platform(&self) -> &'static str {
            "fake"
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
                duration_ms: 5,
            })
        }
    }

    /// Approval requester that returns a fixed outcome and counts invocations.
    struct FixedRequester {
        outcome: ApprovalOutcome,
        calls: Arc<RwLock<u32>>,
    }

    impl FixedRequester {
        fn new(outcome: ApprovalOutcome) -> Self {
            Self {
                outcome,
                calls: Arc::new(RwLock::new(0)),
            }
        }
    }

    #[async_trait]
    impl ApprovalRequester for FixedRequester {
        async fn request_approval(&self, _tool_name: &str, _reason: &str) -> ApprovalOutcome {
            *self.calls.write().await += 1;
            self.outcome
        }
    }

    fn sid() -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral("ws-test")
    }

    fn build_gate_auto_deny() -> Arc<ApprovalGate> {
        // No requester → ApprovalGate::request_approval_for_tool returns Denied.
        Arc::new(ApprovalGate::new(ApprovalConfig::default(), None))
    }

    fn build_gate_with(outcome: ApprovalOutcome) -> Arc<ApprovalGate> {
        Arc::new(ApprovalGate::new(
            ApprovalConfig::default(),
            Some(Arc::new(FixedRequester::new(outcome))),
        ))
    }

    fn build_sandbox(
        tmp: &tempfile::TempDir,
        driver: Arc<dyn OsSandboxDriverTrait>,
        gate: Arc<ApprovalGate>,
        hooks: SandboxHooks,
    ) -> WorkspaceSandbox {
        WorkspaceSandbox::new(tmp.path().to_path_buf(), driver, gate, hooks)
    }

    #[tokio::test]
    async fn workspace_is_created_on_first_execute() {
        let tmp = tempfile::tempdir().unwrap();
        let driver = Arc::new(FakeDriver::new());
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver.clone();
        let sandbox = build_sandbox(
            &tmp,
            driver_trait,
            build_gate_auto_deny(),
            SandboxHooks::new(),
        );
        let session = sid();
        let expected_dir = tmp.path().join(session_key_to_filename(&session));

        // Dir does not exist before first execute.
        assert!(!expected_dir.exists());

        let cmd = SandboxCommand {
            session_id: session.clone(),
            program: "echo".into(),
            args: vec!["hi".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::strict(),
            timeout: None,
        };
        let output = sandbox.execute(cmd.clone()).await.expect("first execute");
        assert_eq!(output.exit_code, Some(0));
        assert!(expected_dir.exists(), "workspace dir should be created");

        // Second execute reuses the same directory (idempotent) and still runs.
        let _ = sandbox.execute(cmd).await.expect("second execute");
        assert!(expected_dir.exists());
        // Driver saw both invocations.
        assert_eq!(*driver.run_count.read().await, 2);
    }

    #[tokio::test]
    async fn cwd_outside_workspace_root_is_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
        let sandbox = build_sandbox(&tmp, driver, build_gate_auto_deny(), SandboxHooks::new());
        let err = sandbox
            .execute(SandboxCommand {
                session_id: sid(),
                program: "echo".into(),
                args: vec![],
                env: HashMap::new(),
                stdin: None,
                cwd: Some("/usr/bin".into()),
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect_err("cwd outside root must be denied");
        assert!(matches!(err, SandboxError::CapabilityDenied { .. }));
    }

    #[tokio::test]
    async fn approval_denied_returns_capability_denied_error() {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
        // Elevated request (network) + gate that denies → expect CapabilityDenied.
        let sandbox = build_sandbox(
            &tmp,
            driver,
            build_gate_with(ApprovalOutcome::Denied),
            SandboxHooks::new(),
        );
        let elevated = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        };
        let err = sandbox
            .execute(SandboxCommand {
                session_id: sid(),
                program: "curl".into(),
                args: vec!["https://example.com".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: elevated,
                timeout: None,
            })
            .await
            .expect_err("denied approval must surface CapabilityDenied");
        assert!(matches!(err, SandboxError::CapabilityDenied { .. }));
    }

    #[tokio::test]
    async fn approval_approved_caches_grant_for_session() {
        let tmp = tempfile::tempdir().unwrap();
        let driver = Arc::new(FakeDriver::new());
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver.clone();
        let requester = FixedRequester::new(ApprovalOutcome::Approved);
        let calls = requester.calls.clone();
        let gate = Arc::new(ApprovalGate::new(
            ApprovalConfig::default(),
            Some(Arc::new(requester)),
        ));
        let sandbox = build_sandbox(&tmp, driver_trait, gate, SandboxHooks::new());
        let elevated = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            ..SandboxCapabilities::strict()
        };
        // Pin the session id — SessionKey::ephemeral generates a fresh UUID on
        // each call, so the test must reuse a single value for the cache-hit
        // check to exercise the same SessionWorkspace.
        let session = sid();
        let mk = || SandboxCommand {
            session_id: session.clone(),
            program: "curl".into(),
            args: vec!["https://example.com".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: elevated.clone(),
            timeout: None,
        };
        // First call asks for approval.
        sandbox.execute(mk()).await.expect("first approved");
        assert_eq!(*calls.read().await, 1);
        // Second call with same-or-narrower caps uses the cached grant.
        sandbox.execute(mk()).await.expect("second cached");
        assert_eq!(*calls.read().await, 1, "grant should be cached");
        assert_eq!(*driver.run_count.read().await, 2);
    }

    #[test]
    fn session_key_filename_is_deterministic_and_hex() {
        // Reuse a single SessionId — SessionKey::ephemeral embeds a fresh UUID
        // per call, which is exactly why we hash to produce the filename.
        let session = sid();
        let a = session_key_to_filename(&session);
        let b = session_key_to_filename(&session);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32, "16-byte digest hex = 32 chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
