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
use crate::sandbox::dns;
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
            Component::Prefix(p) => {
                // Preserve Windows drive/UNC prefix so the path remains
                // valid for canonicalize and stays on the intended volume.
                normalized.push(p.as_os_str());
            }
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
    fn summary(&self) -> Option<crate::sandbox::summary::SandboxSummary> {
        // Static-per-process snapshot: backend mechanism + workspace parent.
        // Per-call capabilities can be tighter than this; the LLM only needs
        // the envelope so it understands which enforcer it's facing. The
        // `os_driver.platform()` tag is the same identifier carried into
        // logs/telemetry, making correlation easy.
        Some(crate::sandbox::summary::SandboxSummary {
            backend: self.os_driver.platform(),
            policy_tier: "workspace-write",
            writable_roots: vec![self.workspace_root.clone()],
            // Default operational posture allows network; per-call
            // capability checks may tighten this. The summary reflects the
            // envelope, not any specific call.
            network: crate::sandbox::summary::NetworkState::AllowAll,
            max_memory_mb: None,
        })
    }

    async fn execute(&self, mut cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        // Hook context is created on-demand at each call site rather than
        // once at the top: it borrows `&cmd`, which would block the SP-4
        // DNS step from acquiring `&mut cmd.capabilities`. Per-call ctx
        // construction is zero-cost (two pointer copies) and matches NLL
        // semantics.
        if let SandboxHookResult::Deny { reason } = self
            .hooks
            .run_before(&SandboxHookContext::new(&cmd.program, &cmd))
            .await
        {
            return Err(SandboxError::Other(format!("hook denied: {reason}")));
        }

        let ws = self.for_session(&cmd.session_id).await?;

        let cwd = match &cmd.cwd {
            None => ws.cwd.clone(),
            Some(p) => {
                let normalized = normalize_path(p, &ws.cwd);
                // Canonicalize before the containment check: a symlink
                // inside the workspace can satisfy a purely lexical
                // `starts_with` check while resolving to a target
                // outside the jail. Both sides are canonicalized so the
                // comparison is symlink-aware; a cwd that cannot be
                // resolved (missing directory / dangling link) is
                // treated as outside and denied.
                let real_root = tokio::fs::canonicalize(&ws.cwd)
                    .await
                    .unwrap_or_else(|_| ws.cwd.clone());
                let resolved = tokio::fs::canonicalize(&normalized)
                    .await
                    .ok()
                    .filter(|real| real.starts_with(&real_root));
                match resolved {
                    Some(real_cwd) => real_cwd,
                    None => {
                        let err = SandboxError::CapabilityDenied {
                            reason: "cwd outside workspace root".into(),
                        };
                        self.hooks
                            .run_after(
                                &SandboxHookContext::new(&cmd.program, &cmd),
                                Err("cwd outside workspace root"),
                            )
                            .await;
                        return Err(err);
                    }
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
                        self.hooks
                            .run_after(
                                &SandboxHookContext::new(&cmd.program, &cmd),
                                Err("user denied"),
                            )
                            .await;
                        return Err(err);
                    }
                }
            }
        }

        // SP-4: pre-resolve any hostnames in AllowHosts to IPs before the
        // driver builds its profile. Driver-level matchers (Seatbelt
        // `(remote ip ...)`) accept IP literals only. Fail-closed on DNS
        // failure.
        if let Err(e) = dns::resolve_hosts_in_capabilities(&mut cmd.capabilities).await {
            self.hooks
                .run_after(
                    &SandboxHookContext::new(&cmd.program, &cmd),
                    Err("dns resolution failed"),
                )
                .await;
            return Err(e);
        }

        let profile = self.os_driver.profile_for(&cmd.capabilities, &cwd)?;

        // Codex-inspired child-process annotations: spawned processes can
        // detect they're running under a sandbox without round-tripping
        // through any API. Useful for scripts that adapt behaviour
        // (e.g. skip network probes when network is denied). We inject
        // here — after capability check / DNS resolution, before driver
        // run — so the env reflects the actual posture this call executes
        // under. Caller-supplied env wins if it set the same keys
        // explicitly; otherwise we annotate.
        let env_tag = sandbox_env_tag(self.os_driver.platform());
        cmd.env
            .entry("ALEPH_SANDBOX".to_string())
            .or_insert_with(|| env_tag.to_string());
        if matches!(
            cmd.capabilities.network,
            crate::sandbox::capabilities::NetworkPolicy::None
        ) {
            cmd.env
                .entry("ALEPH_SANDBOX_NETWORK_DISABLED".to_string())
                .or_insert_with(|| "1".to_string());
        }

        let timeout = cmd.timeout.unwrap_or(self.default_timeout);
        let mut output = self
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

        // Byte-level secret scrub before any downstream consumer touches stdout/stderr.
        // Whitelist is fed via SecurityContext.injected_secrets when threaded; for
        // direct sandbox callers (no security context) this scrubs with an empty
        // whitelist, which is the safe default.
        if let Ok(ref mut out) = output {
            let injected: &[crate::secrets::injection::InjectedSecret] = &[];
            let stdout_scrub = crate::sandbox::scrub_secrets_bytes(&out.stdout, injected);
            let stderr_scrub = crate::sandbox::scrub_secrets_bytes(&out.stderr, injected);
            if !stdout_scrub.hits.is_empty() || !stderr_scrub.hits.is_empty() {
                tracing::warn!(
                    stdout_hits = ?stdout_scrub.hits,
                    stderr_hits = ?stderr_scrub.hits,
                    "sandbox bytes-scrub redacted secrets in command output"
                );
            }
            out.stdout = stdout_scrub.bytes.into_owned();
            out.stderr = stderr_scrub.bytes.into_owned();
        }

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

        self.hooks
            .run_after(&SandboxHookContext::new(&cmd.program, &cmd), outcome)
            .await;

        output
    }
}

/// Codex-inspired env-friendly mechanism tag derived from a driver's
/// `os/mechanism` platform string. We strip the OS prefix so child
/// processes can branch on `ALEPH_SANDBOX=seatbelt|landlock|bwrap|token`
/// without parsing slashes. Falls back to the full string when no slash
/// is present.
pub(crate) fn sandbox_env_tag(platform: &'static str) -> &'static str {
    platform.rsplit('/').next().unwrap_or(platform)
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
                max_memory_mb: None,
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
    async fn cwd_real_subdir_inside_workspace_is_accepted() {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
        let sandbox = build_sandbox(&tmp, driver, build_gate_auto_deny(), SandboxHooks::new());
        let session = sid();
        // Materialise the session workspace dir, then a real subdir in it.
        let ws_dir = tmp.path().join(session_key_to_filename(&session));
        tokio::fs::create_dir_all(ws_dir.join("sub")).await.unwrap();
        sandbox
            .execute(SandboxCommand {
                session_id: session,
                program: "echo".into(),
                args: vec![],
                env: HashMap::new(),
                stdin: None,
                cwd: Some("sub".into()),
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect("a real subdirectory inside the workspace must be accepted");
    }

    /// BUG-3 regression: a symlink that lives inside the workspace but
    /// resolves outside it passes a purely lexical `starts_with` check.
    /// Canonicalisation must reject it.
    #[cfg(unix)]
    #[tokio::test]
    async fn cwd_symlink_escaping_workspace_is_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
        let sandbox = build_sandbox(&tmp, driver, build_gate_auto_deny(), SandboxHooks::new());
        let session = sid();
        let ws_dir = tmp.path().join(session_key_to_filename(&session));
        tokio::fs::create_dir_all(&ws_dir).await.unwrap();
        // `escape` is lexically inside the workspace but points outside it.
        std::os::unix::fs::symlink(outside.path(), ws_dir.join("escape")).unwrap();
        let err = sandbox
            .execute(SandboxCommand {
                session_id: session,
                program: "echo".into(),
                args: vec![],
                env: HashMap::new(),
                stdin: None,
                cwd: Some("escape".into()),
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect_err("a symlink escaping the workspace must be denied");
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

    #[test]
    fn env_tag_strips_os_prefix() {
        assert_eq!(sandbox_env_tag("macos/seatbelt"), "seatbelt");
        assert_eq!(sandbox_env_tag("linux/bwrap"), "bwrap");
        assert_eq!(sandbox_env_tag("windows/token"), "token");
        assert_eq!(sandbox_env_tag("fake"), "fake");
    }

    /// Recording driver that captures the `env` passed to `run()` so tests
    /// can assert on injected `ALEPH_SANDBOX*` keys.
    struct EnvRecordingDriver {
        last_env: Arc<RwLock<HashMap<String, String>>>,
    }

    impl EnvRecordingDriver {
        fn new() -> (Self, Arc<RwLock<HashMap<String, String>>>) {
            let last_env = Arc::new(RwLock::new(HashMap::new()));
            (
                Self {
                    last_env: last_env.clone(),
                },
                last_env,
            )
        }
    }

    #[async_trait]
    impl OsSandboxDriverTrait for EnvRecordingDriver {
        fn platform(&self) -> &'static str {
            "macos/seatbelt"
        }
        fn is_supported(&self) -> bool {
            true
        }
        fn profile_for(
            &self,
            _caps: &SandboxCapabilities,
            _cwd: &Path,
        ) -> Result<OsSandboxProfile, SandboxError> {
            Ok(OsSandboxProfile {
                contents: String::new(),
                max_memory_mb: None,
                linux_init_policy: None,
                windows_init_policy: None,
            })
        }
        async fn run(
            &self,
            _program: &str,
            _args: &[String],
            env: &HashMap<String, String>,
            _stdin: Option<&[u8]>,
            _cwd: &Path,
            _profile: &OsSandboxProfile,
            _timeout: Duration,
            _max_output_bytes: usize,
        ) -> Result<SandboxOutput, SandboxError> {
            *self.last_env.write().await = env.clone();
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

    #[tokio::test]
    async fn injects_aleph_sandbox_env_var_with_mechanism_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let (driver, last_env) = EnvRecordingDriver::new();
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = Arc::new(driver);
        let sandbox = build_sandbox(
            &tmp,
            driver_trait,
            build_gate_auto_deny(),
            SandboxHooks::new(),
        );
        sandbox
            .execute(SandboxCommand {
                session_id: sid(),
                program: "echo".into(),
                args: vec!["hi".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect("execute");
        let env = last_env.read().await;
        assert_eq!(
            env.get("ALEPH_SANDBOX").map(String::as_str),
            Some("seatbelt")
        );
        assert_eq!(
            env.get("ALEPH_SANDBOX_NETWORK_DISABLED")
                .map(String::as_str),
            Some("1"),
            "strict caps deny network → env var must be set"
        );
    }

    #[tokio::test]
    async fn omits_network_disabled_env_var_when_network_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let (driver, last_env) = EnvRecordingDriver::new();
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = Arc::new(driver);
        // approve any elevation so AllowAll passes
        let sandbox = build_sandbox(
            &tmp,
            driver_trait,
            build_gate_with(ApprovalOutcome::Approved),
            SandboxHooks::new(),
        );
        sandbox
            .execute(SandboxCommand {
                session_id: sid(),
                program: "curl".into(),
                args: vec!["https://example.com".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities {
                    network: crate::sandbox::capabilities::NetworkPolicy::AllowAll,
                    ..Default::default()
                },
                timeout: None,
            })
            .await
            .expect("execute");
        let env = last_env.read().await;
        assert_eq!(
            env.get("ALEPH_SANDBOX").map(String::as_str),
            Some("seatbelt")
        );
        assert!(
            !env.contains_key("ALEPH_SANDBOX_NETWORK_DISABLED"),
            "AllowAll caps must not set the network-disabled annotation"
        );
    }

    #[tokio::test]
    async fn caller_env_wins_over_sandbox_annotation() {
        let tmp = tempfile::tempdir().unwrap();
        let (driver, last_env) = EnvRecordingDriver::new();
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = Arc::new(driver);
        let sandbox = build_sandbox(
            &tmp,
            driver_trait,
            build_gate_auto_deny(),
            SandboxHooks::new(),
        );
        let mut env = HashMap::new();
        env.insert("ALEPH_SANDBOX".to_string(), "explicit-override".to_string());
        sandbox
            .execute(SandboxCommand {
                session_id: sid(),
                program: "echo".into(),
                args: vec!["hi".into()],
                env,
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect("execute");
        let env = last_env.read().await;
        assert_eq!(
            env.get("ALEPH_SANDBOX").map(String::as_str),
            Some("explicit-override"),
            "explicit caller env must not be overwritten"
        );
    }
}

#[cfg(test)]
mod scrub_integration_tests {
    use super::*;
    use crate::sandbox::driver::OsSandboxProfile;
    use crate::sandbox::exec_approval::gate::ApprovalGate;
    use crate::sandbox::exec_approval::types::ApprovalConfig;
    use crate::sandbox::hooks::SandboxHooks;
    use std::path::Path;

    /// Driver that injects a caller-supplied stdout payload, allowing tests to
    /// plant a known secret in the output and verify it is scrubbed.
    struct LeakDriver {
        stdout_payload: Vec<u8>,
        stderr_payload: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl OsSandboxDriverTrait for LeakDriver {
        fn platform(&self) -> &'static str {
            "leak-test"
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
            Ok(SandboxOutput {
                stdout: self.stdout_payload.clone(),
                stderr: self.stderr_payload.clone(),
                exit_code: Some(0),
                signal: None,
                truncated: false,
                duration_ms: 1,
            })
        }
    }

    fn build_leak_sandbox(
        tmp: &tempfile::TempDir,
        stdout_payload: Vec<u8>,
        stderr_payload: Vec<u8>,
    ) -> WorkspaceSandbox {
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(LeakDriver {
            stdout_payload,
            stderr_payload,
        });
        let gate = Arc::new(ApprovalGate::new(ApprovalConfig::default(), None));
        WorkspaceSandbox::new(tmp.path().to_path_buf(), driver, gate, SandboxHooks::new())
    }

    fn mk_cmd() -> SandboxCommand {
        SandboxCommand {
            session_id: crate::routing::session_key::SessionKey::ephemeral("scrub-test"),
            program: "echo".into(),
            args: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::strict(),
            timeout: None,
        }
    }

    #[tokio::test]
    async fn workspace_scrubs_leaked_secret_in_stdout() {
        // Plant a recognisable OpenAI-style key in stdout.
        let mut leak = b"out:".to_vec();
        leak.extend_from_slice(b"sk-proj-");
        leak.extend(std::iter::repeat(b'Z').take(40));

        let tmp = tempfile::tempdir().unwrap();
        let sandbox = build_leak_sandbox(&tmp, leak.clone(), Vec::new());
        let output = sandbox.execute(mk_cmd()).await.expect("execute");

        // The raw key must not appear in the returned bytes.
        let raw_key: Vec<u8> = {
            let mut k = b"sk-proj-".to_vec();
            k.extend(std::iter::repeat(b'Z').take(40));
            k
        };
        assert!(
            !output.stdout.windows(raw_key.len()).any(|w| w == raw_key),
            "raw secret must be redacted from stdout"
        );
        // The redaction marker must be present.
        assert!(
            output
                .stdout
                .windows(b"[REDACTED".len())
                .any(|w| w == b"[REDACTED"),
            "stdout must contain [REDACTED marker"
        );
        // stderr is untouched (nothing was planted there).
        assert!(output.stderr.is_empty());
    }

    #[tokio::test]
    async fn workspace_scrubs_leaked_secret_in_stderr() {
        // Plant a recognisable key in stderr.
        let mut leak = b"err:".to_vec();
        leak.extend_from_slice(b"sk-proj-");
        leak.extend(std::iter::repeat(b'A').take(40));

        let tmp = tempfile::tempdir().unwrap();
        let sandbox = build_leak_sandbox(&tmp, Vec::new(), leak.clone());
        let output = sandbox.execute(mk_cmd()).await.expect("execute");

        let raw_key: Vec<u8> = {
            let mut k = b"sk-proj-".to_vec();
            k.extend(std::iter::repeat(b'A').take(40));
            k
        };
        assert!(
            !output.stderr.windows(raw_key.len()).any(|w| w == raw_key),
            "raw secret must be redacted from stderr"
        );
        assert!(
            output
                .stderr
                .windows(b"[REDACTED".len())
                .any(|w| w == b"[REDACTED"),
            "stderr must contain [REDACTED marker"
        );
    }

    #[tokio::test]
    async fn clean_output_passes_through_unchanged() {
        let stdout = b"hello world\n".to_vec();
        let stderr = b"no secrets here\n".to_vec();

        let tmp = tempfile::tempdir().unwrap();
        let sandbox = build_leak_sandbox(&tmp, stdout.clone(), stderr.clone());
        let output = sandbox.execute(mk_cmd()).await.expect("execute");

        assert_eq!(output.stdout, stdout, "clean stdout must be unchanged");
        assert_eq!(output.stderr, stderr, "clean stderr must be unchanged");
    }
}
