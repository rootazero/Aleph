//! `WorkspaceSandbox` — lazy per-session workspace directory + capability enforcement.
//!
//! Implements the `Sandbox` trait by materializing `~/.aleph/workspaces/{hash(session_id)}/`
//! on first exec-class call and routing commands through the 6-step pipeline:
//! 1. resolve session workspace (lazy create dir)
//! 2. validate cwd (None → workspace root; Some(p) must live under root)
//! 3. capability check: within baseline → pass; else consult granted cache then
//!    `ApprovalGate::request_approval_for_action`
//! 4. `OsSandboxDriverTrait::profile_for`
//! 5. `OsSandboxDriverTrait::run`
//! 6. emit `capability_ledger` tracing audit record

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::dns;
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::exec_approval::denial_ledger;
use crate::sandbox::exec_approval::gate::{ApprovalGate, ApprovalOutcome};
use crate::sandbox::hooks::{SandboxHookContext, SandboxHookResult, SandboxHooks};
use crate::sandbox::Sandbox;
use crate::session::service::SessionId;

mod approval;
mod env;
mod path;
mod proxy;

pub use path::session_workspace_dir;

pub(crate) use approval::{format_capability_request, format_capability_summary};
pub(crate) use env::sandbox_env_tag;
pub(crate) use path::{normalize_path, session_key_to_filename};
use proxy::maybe_spawn_proxy;
pub(crate) use proxy::ActiveProxy;

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
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Override the default 1 MB combined stdout+stderr truncation budget.
    #[must_use]
    pub const fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    /// Override the default hooks (empty by default).
    #[must_use]
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
            cwd,
            baseline: SandboxCapabilities::strict(),
            granted_elevations: RwLock::new(HashSet::new()),
        });
        sessions.insert(
            // rust-doctor-disable-next-line excessive-clone
            sid.clone(),
            // rust-doctor-disable-next-line excessive-clone
            ws.clone(),
        );
        Ok(ws)
    }
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
            policy_tier: crate::sandbox::summary::PolicyTier::WorkspaceWrite.as_str(),
            writable_roots: vec![self.workspace_root.clone()],
            // Honest per-call default: the baseline is `strict()` (network
            // DENIED). Network is reachable only via an approval-gated
            // capability escalation, so telling the model "allowed (all hosts)"
            // here would invite it to assume egress it does not actually have.
            network: crate::sandbox::summary::NetworkState::Denied,
            max_memory_mb: None,
        })
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
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
            // rust-doctor-disable-next-line excessive-clone
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
                let real_root = match tokio::fs::canonicalize(&ws.cwd).await {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(SandboxError::CapabilityDenied {
                            reason: "workspace root cannot be resolved".into(),
                        });
                    }
                };
                let resolved = tokio::fs::canonicalize(&normalized)
                    .await
                    .ok()
                    .filter(|real| real.starts_with(&real_root));
                match resolved {
                    Some(real_cwd) => real_cwd,
                    None => {
                        return Err(SandboxError::CapabilityDenied {
                            reason: "cwd outside workspace root".into(),
                        });
                    }
                }
            }
        };

        let normalized_caps = cmd.capabilities.normalized();
        if !cmd.capabilities.is_within(&ws.baseline) {
            let already_granted = {
                let granted = ws.granted_elevations.read().await;
                granted.iter().any(|g| normalized_caps.is_within(g))
            };
            if !already_granted {
                let reason = format_capability_request(&cmd.program, &cmd.capabilities);
                // Denial ledger: a prior refusal of this exact elevation — or a
                // session past the denial threshold — auto-denies without
                // re-prompting (blind-retry guard + circuit breaker). The
                // fingerprint keys on the deterministic capability SUMMARY
                // (`format_capability_summary` over the NORMALIZED caps) — never
                // on `reason`, which appends the model-controlled `justification`:
                // folding that in would let the model mint a fresh fingerprint for
                // the same escalation just by rewording its reason, defeating this
                // guard. Path order is neutralized by `normalized_caps`.
                // `denial_ledger::ledger_key`, NOT `session_key_to_filename`:
                // the latter names this session's workspace *directory*, and
                // keying the ledger with it put this gate in a different bucket
                // from the tool confirm gate, so neither saw the other's
                // refusals. A filename encoder is not an identity.
                let led_key = denial_ledger::ledger_key(&cmd.session_id);
                let fingerprint = denial_ledger::action_fingerprint(
                    &cmd.program,
                    &format_capability_summary(&cmd.program, &normalized_caps),
                );
                if let Some(reason_kind) =
                    denial_ledger::global().is_blocked(&led_key, &fingerprint)
                {
                    tracing::info!(
                        program = %cmd.program,
                        denial = ?reason_kind,
                        "capability elevation auto-denied by denial ledger: {}",
                        reason_kind.agent_hint()
                    );
                    return Err(SandboxError::CapabilityDenied {
                        reason: format!(
                            "elevated capability previously denied this session. {}",
                            reason_kind.agent_hint()
                        ),
                    });
                }
                // The card shows the real command line (redacted), not just the
                // program: `python` says nothing about what it is about to do
                // with the network capability it is asking for.
                let action = crate::sandbox::exec_approval::ApprovalAction::for_command(
                    &cmd.program,
                    &cmd.args,
                    cmd.cwd.as_deref(),
                    &reason,
                );
                let response = self
                    .approval_gate
                    .request_approval_for_action(&action)
                    .await;
                let outcome = response.outcome;
                match outcome {
                    // Either grant flavour elevates; this path already caches the
                    // grant per-session in `granted_elevations`, so a one-shot
                    // `Approved` and a session-scoped `ApprovedForSession` are
                    // equivalent here.
                    ApprovalOutcome::Approved | ApprovalOutcome::ApprovedForSession => {
                        ws.granted_elevations.write().await.insert(normalized_caps);
                    }
                    ApprovalOutcome::Denied | ApprovalOutcome::Timeout => {
                        // Remember the refusal so the next blind retry of this
                        // exact elevation is short-circuited above. A `Timeout`
                        // is passed but dropped by the ledger (an expired card
                        // is not a decision) — see `DenialLedger::record_denial`.
                        let reason_kind = if matches!(outcome, ApprovalOutcome::Timeout) {
                            denial_ledger::DenialReason::Timeout
                        } else {
                            denial_ledger::DenialReason::UserRejected
                        };
                        // Capture the circuit-breaker trip edge and mirror the
                        // tool-confirm gate's anti-reference-bypass purge: when
                        // this refusal is the one that crosses the brute-force
                        // denial threshold, wipe THIS session's offloaded
                        // tool-result cache so a now-paused adversarial session
                        // cannot mine results cached under an earlier, more
                        // permissive moment via `ctx_search` / `read_file`. Both
                        // gates share one session bucket (same `led_key`), so the
                        // breaker can trip here too; the sandbox layer holds no
                        // injected `result_store`, so it purges via the
                        // process-global one — `led_key` is the read path's scope
                        // string byte-for-byte.
                        let just_paused = denial_ledger::global().record_denial(
                            &led_key,
                            &fingerprint,
                            reason_kind,
                        );
                        if just_paused {
                            if let Some(store) =
                                crate::tools::result_store::global_tool_result_store()
                            {
                                crate::tools::result_store::ToolResultStore::for_session(
                                    &store,
                                    led_key.as_str(),
                                )
                                .purge_all();
                                tracing::warn!(
                                    session = %led_key,
                                    "denial circuit-breaker tripped at elevation gate — \
                                     purged offloaded tool-result cache (anti-reference-bypass)"
                                );
                            }
                        }
                        // Relay the human's stated reason (if any) so the model
                        // re-plans on it instead of a bare "denied".
                        let user_reason = response
                            .deny_reason
                            .as_deref()
                            .map(|r| format!(" The user said: \"{r}\"."))
                            .unwrap_or_default();
                        return Err(SandboxError::CapabilityDenied {
                            reason: format!(
                                "user denied elevated capability request.{user_reason} {}",
                                reason_kind.agent_hint()
                            ),
                        });
                    }
                }
            }
        }

        // Cycle 6 Phase A — managed in-process proxy.
        //
        // When `NetworkPolicy::AllowHosts` is in effect and the platform
        // can reach loopback (macOS Seatbelt, Windows AppContainer), we
        // spawn a per-call HTTP CONNECT + SOCKS5 proxy bound to
        // `127.0.0.1:0`, enforce the hostname allowlist there, and
        // collapse the OS-level network policy to "loopback only".
        // Standard `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` env vars
        // are injected so HTTP clients route through us.
        //
        // Linux bwrap reaches the proxy via the Phase B netns→UDS→loopback
        // bridge: `maybe_spawn_proxy` additionally spawns a host bridge and
        // injects a route spec env var that the driver + `sandbox-init`
        // consume. The returned guard keeps the proxy (and bridge) alive.
        let active_proxy: Option<ActiveProxy> =
            maybe_spawn_proxy(&self.os_driver, &mut cmd).await?;

        // SP-4: pre-resolve any hostnames in AllowHosts to IPs before the
        // driver builds its profile. After the proxy rewrite above this
        // is a no-op for the proxy path (capability becomes IP-only), but
        // it's still required for `AllowAll` (no-op) and for the Linux
        // fallback path where AllowHosts goes straight to the driver.
        dns::resolve_hosts_in_capabilities(&mut cmd.capabilities).await?;

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
        if matches!(cmd.capabilities.network, NetworkPolicy::None) {
            cmd.env
                .entry("ALEPH_SANDBOX_NETWORK_DISABLED".to_string())
                .or_insert_with(|| "1".to_string());
        }

        // Effective wall-clock timeout, most-specific first: an explicit
        // per-command `timeout` wins; else the capability-level `timeout_secs`
        // override (the documented per-call ceiling); else the sandbox-wide
        // default. This is the single authoritative timeout for execution:
        // `SandboxCapabilities.timeout_secs` is the only timeout carrier — no
        // OS-profile timeout is derived or enforced.
        let timeout = cmd
            .timeout
            .or_else(|| cmd.capabilities.timeout_secs.map(Duration::from_secs))
            .unwrap_or(self.default_timeout);
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

        // Keep the managed proxy (and Linux host bridge) alive across the OS
        // driver `run` and only shut it down here (drop = shutdown + UDS dir
        // cleanup). Explicit binding is required because `active_proxy` is
        // declared early in the function for scope but not consumed by any
        // sub-call.
        drop(active_proxy);

        // Command-output content floor (secret redaction + block-class gate +
        // invisible/bidi neutralization) before any downstream consumer touches
        // stdout/stderr. Shared with `WorktreeSandbox` via the single source of
        // truth in `scrub::scrub_and_gate_output`, so the two execution paths
        // cannot silently diverge on what they redact or refuse.
        let output_blocked: Vec<&'static str> = if let Ok(ref mut out) = output {
            crate::sandbox::scrub::scrub_and_gate_output(out)
        } else {
            Vec::new()
        };

        // Block-class secret floor: a catastrophic secret (e.g. a PEM private
        // key) in command output is never legitimate — fail closed rather than
        // return the already-redacted output to the model. Shell-output analogue
        // of clawshell's `DlpAction::Block`, mirroring the always-on hard-filter
        // posture of the default command policy.
        if !output_blocked.is_empty() {
            tracing::warn!(
                target: "shell_security",
                session_id = ?cmd.session_id,
                program = %cmd.program,
                blocked = ?output_blocked,
                "command output blocked: catastrophic secret material detected"
            );
            output = Err(SandboxError::Other(format!(
                "command output blocked: catastrophic secret material detected ({})",
                output_blocked.join(", ")
            )));
        }

        match &output {
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
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::sandbox::capabilities::NetworkPolicy;
    use crate::sandbox::driver::OsSandboxProfile;
    use crate::sandbox::exec_approval::gate::ApprovalRequester;
    use crate::sync_primitives::Mutex;

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
                exit_code: Some(0),
                duration_ms: 5,
                ..Default::default()
            })
        }
    }

    /// Cycle 6: a fake driver that claims `macos/seatbelt` as its platform
    /// (so `WorkspaceSandbox::maybe_spawn_proxy` engages) and captures the
    /// `env` + `capabilities` it would have run with. Lets us assert the
    /// proxy injection without running real macOS sandbox-exec.
    struct CapturingMacosDriver {
        captured_env: Mutex<HashMap<String, String>>,
        captured_caps: Mutex<Option<SandboxCapabilities>>,
    }

    impl CapturingMacosDriver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                captured_env: Mutex::new(HashMap::new()),
                captured_caps: Mutex::new(None),
            })
        }

        fn env(&self) -> HashMap<String, String> {
            self.captured_env
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        fn caps(&self) -> Option<SandboxCapabilities> {
            self.captured_caps
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl OsSandboxDriverTrait for CapturingMacosDriver {
        fn platform(&self) -> &'static str {
            "macos/seatbelt"
        }

        fn is_supported(&self) -> bool {
            true
        }

        fn profile_for(
            &self,
            capabilities: &SandboxCapabilities,
            _cwd: &Path,
        ) -> Result<OsSandboxProfile, SandboxError> {
            // Capture the (post-proxy-rewrite, post-DNS) capabilities at the
            // exact moment the OS driver would build its profile.
            *self.captured_caps.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(capabilities.clone());
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
            *self.captured_env.lock().unwrap_or_else(|e| e.into_inner()) = env.clone();
            Ok(SandboxOutput {
                exit_code: Some(0),
                duration_ms: 1,
                ..Default::default()
            })
        }
    }

    /// Like [`CapturingMacosDriver`] but reports `linux/bwrap`, so the
    /// workspace engages the Phase B Linux branch of `maybe_spawn_proxy`
    /// (managed proxy + host bridge + route spec). The bridge itself is
    /// cross-platform (UDS ↔ TCP), so this path runs on a macOS dev box.
    #[cfg(target_os = "linux")] // sole consumer is the linux-gated test below
    struct CapturingLinuxDriver {
        captured_env: Mutex<HashMap<String, String>>,
        captured_caps: Mutex<Option<SandboxCapabilities>>,
    }

    #[cfg(target_os = "linux")]
    impl CapturingLinuxDriver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                captured_env: Mutex::new(HashMap::new()),
                captured_caps: Mutex::new(None),
            })
        }
        fn env(&self) -> HashMap<String, String> {
            self.captured_env
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
        fn caps(&self) -> Option<SandboxCapabilities> {
            self.captured_caps
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    #[cfg(target_os = "linux")]
    impl OsSandboxDriverTrait for CapturingLinuxDriver {
        fn platform(&self) -> &'static str {
            "linux/bwrap"
        }
        fn is_supported(&self) -> bool {
            true
        }
        fn profile_for(
            &self,
            capabilities: &SandboxCapabilities,
            _cwd: &Path,
        ) -> Result<OsSandboxProfile, SandboxError> {
            *self.captured_caps.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(capabilities.clone());
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
            *self.captured_env.lock().unwrap_or_else(|e| e.into_inner()) = env.clone();
            Ok(SandboxOutput {
                exit_code: Some(0),
                duration_ms: 1,
                ..Default::default()
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
        async fn request_approval(
            &self,
            _action: &crate::sandbox::exec_approval::ApprovalAction,
        ) -> crate::sandbox::exec_approval::ApprovalResponse {
            *self.calls.write().await += 1;
            self.outcome.into()
        }
    }

    /// A session of this test's own.
    ///
    /// `ephemeral()` mints a fresh uuid, so every call yields a distinct key
    /// (`agent:ws-test:ephemeral:<uuid>`) — which is the only reason these tests
    /// can run in parallel at all: the denial ledger is process-global and keyed
    /// by session, and its session-pause flag is sticky, so two tests sharing a
    /// key would share a bucket and become order-dependent.
    ///
    /// That isolation is incidental — nothing here would fail loudly if
    /// `ephemeral()` ever became deterministic. [`sid_is_unique_per_call`] pins
    /// it, so the day it changes, one obvious test breaks instead of a dozen
    /// unrelated ones flaking.
    fn sid() -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral("ws-test")
    }

    #[test]
    fn sid_is_unique_per_call() {
        assert_ne!(
            sid().to_string(),
            sid().to_string(),
            "every test must own its session: the denial ledger is process-global \
             and keyed by session, and its pause flag is sticky"
        );
    }

    fn build_gate_auto_deny() -> Arc<ApprovalGate> {
        // No requester → ApprovalGate::request_approval_for_action returns Denied.
        Arc::new(ApprovalGate::new(None))
    }

    fn build_gate_with(outcome: ApprovalOutcome) -> Arc<ApprovalGate> {
        Arc::new(ApprovalGate::new(Some(Arc::new(FixedRequester::new(
            outcome,
        )))))
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
        let SandboxError::CapabilityDenied { reason } = err else {
            panic!("expected CapabilityDenied, got {err:?}");
        };
        // The denial ledger's agent hint must reach the model-facing reason —
        // a generic "denied" lets the agent blind-retry; the hint tells it to
        // change approach instead.
        assert!(
            reason.contains("do not re-request"),
            "live denial reason must carry the agent hint; got: {reason}"
        );
    }

    /// Denial ledger circuit breaker: after the first denial is recorded, an
    /// identical elevated request is auto-blocked by the denial ledger
    /// *without* re-prompting, and the model-facing reason must carry the
    /// `RepeatedSameIntent` hint so the agent stops looping.
    #[tokio::test]
    async fn blind_retry_is_auto_blocked_with_ledger_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
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
        // Pin one session so both attempts hash to the same ledger key.
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
        // First attempt: live denial records the refusal in the ledger.
        let _ = sandbox.execute(mk()).await.expect_err("first denied");
        // Second attempt: same intent → auto-blocked by the ledger.
        let err = sandbox
            .execute(mk())
            .await
            .expect_err("blind retry blocked");
        let SandboxError::CapabilityDenied { reason } = err else {
            panic!("expected CapabilityDenied, got {err:?}");
        };
        assert!(
            reason.contains("previously denied this session") && reason.contains("auto-refused"),
            "auto-block reason must carry the RepeatedSameIntent hint; got: {reason}"
        );
    }

    /// REGRESSION — the session-key encoding one. `blind_retry_is_auto_blocked_with_ledger_hint`
    /// above records *and* reads through this same path, so it passes under any
    /// key encoding and cannot see the bug. This test reconstructs the elevation
    /// gate's ledger key from scratch — `ledger_key(session)` + the exact
    /// capability fingerprint — and asserts the refusal is retrievable there.
    ///
    /// The elevation gate used to key the ledger with `session_key_to_filename`
    /// — the SHA-256 encoder that names this session's workspace *directory* —
    /// while the confirm gate used `ledger_key`. Same session, two strings that
    /// never collide, so one global map held two disjoint ledgers, and the
    /// session-wide "3 denials pause the session" breaker really allowed 3 per
    /// *path*. Both gates now share `ledger_key(session)` for the SESSION
    /// dimension (that shared breaker is the real cross-gate invariant). The
    /// per-intent fingerprint stays gate-specific by design — the confirm gate
    /// keys on `grant_fingerprint(tool, json)`, the elevation gate on the
    /// capability summary — so this test verifies the elevation gate's own
    /// blind-retry readback under the shared session key, not per-intent
    /// visibility across gates.
    #[tokio::test]
    async fn an_elevation_refusal_is_visible_to_the_tool_confirm_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
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
        // The denial ledger is process-global, so this test must not share a
        // bucket with any other. It doesn't: `sid()` mints a fresh
        // `agent:ws-test:ephemeral:<uuid>` per call, so every test here already
        // owns its own session.
        let session = sid();

        let denied = sandbox
            .execute(SandboxCommand {
                session_id: session.clone(),
                program: "curl".into(),
                args: vec!["https://example.com".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: elevated.clone(),
                timeout: None,
            })
            .await;
        assert!(denied.is_err(), "the elevation must be refused");

        // Look it up exactly as the production elevation gate keys it: on the
        // justification-free capability summary over the normalized caps (never
        // `format_capability_request`, which would fold in a model justification).
        let fingerprint = denial_ledger::action_fingerprint(
            "curl",
            &format_capability_summary("curl", &elevated.normalized()),
        );
        assert_eq!(
            denial_ledger::global().is_blocked(&denial_ledger::ledger_key(&session), &fingerprint),
            Some(denial_ledger::DenialReason::RepeatedSameIntent),
            "the confirm gate must see the refusal the elevation gate recorded — \
             if this is None, the two gates are keying the ledger differently again"
        );
    }

    /// The denial circuit-breaker must be reachable at the ELEVATION gate, not
    /// only the confirm gate — both increment one shared per-session counter.
    /// Three distinct elevation refusals must pause the session, which is the
    /// precondition for the anti-reference-bypass tool-result purge this gate now
    /// fires on the trip edge. (The `purge_all` mechanics themselves are covered
    /// in `tools::result_store`; the process-global store makes a deterministic
    /// purge assertion order-dependent here, so we pin the reachable trip
    /// instead.)
    #[tokio::test]
    async fn three_distinct_elevation_denials_trip_the_session_breaker() {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
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
        let session = sid();
        // Three DISTINCT intents (different program → different fingerprint), so
        // each is a fresh denial that advances the session-wide denial counter.
        for program in ["curl", "wget", "nc"] {
            let _ = sandbox
                .execute(SandboxCommand {
                    session_id: session.clone(),
                    program: program.into(),
                    args: vec!["https://example.com".into()],
                    env: HashMap::new(),
                    stdin: None,
                    cwd: None,
                    capabilities: elevated.clone(),
                    timeout: None,
                })
                .await
                .expect_err("each elevation is denied");
        }
        // A brand-new, never-denied intent is now auto-refused: the breaker
        // tripped at the elevation gate (pause outranks per-intent state).
        let fresh = denial_ledger::action_fingerprint(
            "totally-new-program",
            &format_capability_summary("totally-new-program", &elevated.normalized()),
        );
        assert_eq!(
            denial_ledger::global().is_blocked(&denial_ledger::ledger_key(&session), &fresh),
            Some(denial_ledger::DenialReason::ThresholdExceeded),
            "3 distinct elevation denials must pause the session — the breaker is \
             reachable at the elevation gate, so the trip-edge purge can fire here"
        );
    }

    #[tokio::test]
    async fn approval_approved_caches_grant_for_session() {
        let tmp = tempfile::tempdir().unwrap();
        let driver = Arc::new(FakeDriver::new());
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver.clone();
        let requester = FixedRequester::new(ApprovalOutcome::Approved);
        let calls = requester.calls.clone();
        let gate = Arc::new(ApprovalGate::new(Some(Arc::new(requester))));
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
                exit_code: Some(0),
                duration_ms: 1,
                ..Default::default()
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

    // ── Cycle 6 Phase A — managed proxy injection ─────────────────────

    /// Build a sandbox using a `CapturingMacosDriver` so the workspace's
    /// `maybe_spawn_proxy` path engages (platform == `macos/seatbelt`),
    /// with the approval gate fixed to `Approved` so AllowHosts requests
    /// pass the capability check.
    fn build_macos_capturing(
        tmp: &tempfile::TempDir,
    ) -> (WorkspaceSandbox, Arc<CapturingMacosDriver>) {
        let driver = CapturingMacosDriver::new();
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver.clone();
        let sandbox = WorkspaceSandbox::new(
            tmp.path().to_path_buf(),
            driver_trait,
            build_gate_with(ApprovalOutcome::Approved),
            SandboxHooks::new(),
        );
        (sandbox, driver)
    }

    fn allow_hosts_caps(hosts: &[&str]) -> SandboxCapabilities {
        SandboxCapabilities {
            network: NetworkPolicy::AllowHosts {
                hosts: hosts.iter().map(|h| h.to_string()).collect(),
            },
            ..SandboxCapabilities::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proxy_injects_env_vars_for_allow_hosts_on_macos() {
        let tmp = tempfile::tempdir().unwrap();
        let (sandbox, driver) = build_macos_capturing(&tmp);
        let cmd = SandboxCommand {
            session_id: sid(),
            program: "curl".into(),
            args: vec!["https://api.example.com/".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: allow_hosts_caps(&["api.example.com", "*.github.com"]),
            timeout: None,
        };
        let out = sandbox.execute(cmd).await.expect("execute");
        assert_eq!(out.exit_code, Some(0));

        let env = driver.env();
        let http_proxy = env.get("HTTP_PROXY").expect("HTTP_PROXY missing");
        assert!(
            http_proxy.starts_with("http://127.0.0.1:"),
            "HTTP_PROXY should point at loopback proxy, got {http_proxy}"
        );
        assert_eq!(env.get("HTTPS_PROXY"), Some(http_proxy));
        assert_eq!(env.get("ALL_PROXY"), Some(http_proxy));
        assert_eq!(env.get("http_proxy"), Some(http_proxy));
        assert!(env.get("NO_PROXY").unwrap().contains("127.0.0.1"));

        // Capabilities reaching the OS driver have been collapsed to loopback.
        let driver_caps = driver.caps().expect("profile_for not called");
        match driver_caps.network {
            NetworkPolicy::AllowHosts { hosts } => {
                assert_eq!(hosts, vec!["127.0.0.1".to_string()]);
            }
            other => panic!("expected AllowHosts(['127.0.0.1']), got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proxy_skipped_when_network_policy_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let (sandbox, driver) = build_macos_capturing(&tmp);
        let cmd = SandboxCommand {
            session_id: sid(),
            program: "echo".into(),
            args: vec!["hi".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::strict(),
            timeout: None,
        };
        let _ = sandbox.execute(cmd).await.expect("execute");
        let env = driver.env();
        assert!(!env.contains_key("HTTP_PROXY"));
        assert!(!env.contains_key("HTTPS_PROXY"));
    }

    // ── Phase B — Linux netns bridge orchestration (cross-platform path) ──

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[cfg(target_os = "linux")] // Linux netns host-bridge orchestration only
    async fn proxy_injects_route_spec_and_host_bridge_on_linux() {
        // The host bridge derives its UDS path from $HOME (via
        // `create_proxy_socket_dir()` → `dirs::home_dir()`). Other tests mutate
        // HOME to a long temp path; serialize against them so we bind under the
        // real (short) HOME and never overflow the platform `SUN_LEN` limit.
        let _home_guard = crate::runtimes::post_install::HomeEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        let driver = CapturingLinuxDriver::new();
        let driver_trait: Arc<dyn OsSandboxDriverTrait> = driver.clone();
        let sandbox = WorkspaceSandbox::new(
            tmp.path().to_path_buf(),
            driver_trait,
            build_gate_with(ApprovalOutcome::Approved),
            SandboxHooks::new(),
        );
        let cmd = SandboxCommand {
            session_id: sid(),
            program: "curl".into(),
            args: vec!["https://api.example.com/".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: allow_hosts_caps(&["api.example.com"]),
            timeout: None,
        };
        sandbox.execute(cmd).await.expect("execute");

        let env = driver.env();
        // Standard proxy env vars still injected (sandbox-init rewrites the
        // port in-netns; here we just confirm they reached the driver).
        assert!(env
            .get("HTTP_PROXY")
            .is_some_and(|v| v.starts_with("http://127.0.0.1:")));

        // The route spec env var is present and parses into a well-formed spec
        // listing the proxy env keys and a UDS path under the socket dir.
        let spec_json = env
            .get(crate::sandbox::proxy::PROXY_ROUTE_SPEC_ENV)
            .expect("route spec env var must be injected on linux");
        let spec = crate::sandbox::proxy::ProxyRouteSpec::from_env_json(spec_json)
            .expect("route spec must be valid JSON");
        assert!(spec.env_keys.contains(&"HTTP_PROXY".to_string()));
        assert!(spec.env_keys.contains(&"all_proxy".to_string()));
        assert!(
            spec.uds_path.to_string_lossy().ends_with(".sock"),
            "uds_path should be a socket file, got {:?}",
            spec.uds_path
        );

        // Capabilities reaching the driver are collapsed to loopback.
        let driver_caps = driver.caps().expect("profile_for not called");
        match driver_caps.network {
            NetworkPolicy::AllowHosts { hosts } => {
                assert_eq!(hosts, vec!["127.0.0.1".to_string()])
            }
            other => panic!("expected AllowHosts(['127.0.0.1']), got {other:?}"),
        }

        // The host bridge's socket dir is cleaned up once `execute` drops the
        // ActiveProxy guard (drop = abort + remove_dir_all).
        assert!(
            !spec.uds_path.exists(),
            "UDS must be removed after the run completes"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn proxy_respects_caller_supplied_http_proxy_env() {
        let tmp = tempfile::tempdir().unwrap();
        let (sandbox, driver) = build_macos_capturing(&tmp);
        let mut env_pre = HashMap::new();
        env_pre.insert("HTTP_PROXY".into(), "http://corporate-proxy:8080".into());
        let cmd = SandboxCommand {
            session_id: sid(),
            program: "curl".into(),
            args: vec!["https://api.example.com/".into()],
            env: env_pre,
            stdin: None,
            cwd: None,
            capabilities: allow_hosts_caps(&["api.example.com"]),
            timeout: None,
        };
        let _ = sandbox.execute(cmd).await.expect("execute");
        let env = driver.env();
        // Caller wins: HTTP_PROXY stays as supplied; HTTPS_PROXY/ALL_PROXY
        // (not supplied) point at the loopback proxy.
        assert_eq!(
            env.get("HTTP_PROXY"),
            Some(&"http://corporate-proxy:8080".to_string())
        );
        assert!(env
            .get("HTTPS_PROXY")
            .map(|v: &String| v.starts_with("http://127.0.0.1:"))
            .unwrap_or(false));
    }
}

#[cfg(test)]
mod scrub_integration_tests {
    use super::approval::{sanitize_justification, JUSTIFICATION_MAX_CHARS};
    use super::*;
    use crate::sandbox::driver::OsSandboxProfile;
    use crate::sandbox::exec_approval::gate::ApprovalGate;
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
                duration_ms: 1,
                ..Default::default()
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
        let gate = Arc::new(ApprovalGate::new(None));
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
        leak.extend(std::iter::repeat_n(b'Z', 40));

        let tmp = tempfile::tempdir().unwrap();
        let sandbox = build_leak_sandbox(&tmp, leak.clone(), Vec::new());
        let output = sandbox.execute(mk_cmd()).await.expect("execute");

        // The raw key must not appear in the returned bytes.
        let raw_key: Vec<u8> = {
            let mut k = b"sk-proj-".to_vec();
            k.extend(std::iter::repeat_n(b'Z', 40));
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
        leak.extend(std::iter::repeat_n(b'A', 40));

        let tmp = tempfile::tempdir().unwrap();
        let sandbox = build_leak_sandbox(&tmp, Vec::new(), leak.clone());
        let output = sandbox.execute(mk_cmd()).await.expect("execute");

        let raw_key: Vec<u8> = {
            let mut k = b"sk-proj-".to_vec();
            k.extend(std::iter::repeat_n(b'A', 40));
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

    // ── Escalation justification (codex `justification` / hermes reason) ──

    fn escalating_caps() -> SandboxCapabilities {
        SandboxCapabilities {
            network: crate::sandbox::capabilities::NetworkPolicy::AllowAll,
            spawn_subprocess: true,
            ..SandboxCapabilities::strict()
        }
    }

    /// Without a scoped `EXEC_JUSTIFICATION` the approval reason is exactly the
    /// pre-feature capabilities-only string — no behavioural change for the
    /// (common) no-justification path.
    #[test]
    fn capability_request_is_byte_identical_without_justification() {
        let reason = format_capability_request("bash", &escalating_caps());
        assert_eq!(
            reason,
            "bash requests elevated capabilities: network=AllowAll, spawn=true"
        );
        assert!(!reason.contains("justification"));
    }

    /// When the model supplied a justification the approver sees WHY appended
    /// to the (unchanged) WHAT.
    #[tokio::test]
    async fn capability_request_appends_scoped_justification() {
        let reason = crate::sandbox::context::EXEC_JUSTIFICATION
            .scope(
                "fetch crates from crates.io for cargo build".to_string(),
                async { format_capability_request("bash", &escalating_caps()) },
            )
            .await;
        assert!(
            reason.starts_with("bash requests elevated capabilities: network=AllowAll, spawn=true"),
            "the WHAT portion must stay byte-identical: {reason}"
        );
        assert!(
            reason.contains("— justification: fetch crates from crates.io for cargo build"),
            "the WHY must be appended: {reason}"
        );
    }

    #[test]
    fn sanitize_justification_collapses_control_and_whitespace() {
        // Newlines / tabs / NUL collapse to single spaces; runs of blanks fold.
        let cleaned = sanitize_justification("line one\n\tline\u{0}  two\r\n".to_string());
        assert_eq!(cleaned.as_deref(), Some("line one line two"));
    }

    #[test]
    fn sanitize_justification_rejects_blank() {
        assert_eq!(sanitize_justification(String::new()), None);
        assert_eq!(sanitize_justification("   \n\t  ".to_string()), None);
    }

    #[test]
    fn sanitize_justification_clamps_runaway_length() {
        let huge = "x".repeat(JUSTIFICATION_MAX_CHARS * 4);
        let cleaned = sanitize_justification(huge).expect("non-empty");
        assert_eq!(cleaned.chars().count(), JUSTIFICATION_MAX_CHARS);
    }
}
