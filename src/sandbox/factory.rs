//! `build_sandbox` — compose the `Arc<dyn Sandbox>` held by `AppContext`.
//!
//! Produces a `WorkspaceSandbox` when `SandboxConfig::enabled` is `true`
//! and falls back to `NoopSandbox` otherwise. Task 8 will thread the
//! returned `Arc<dyn Sandbox>` into exec-class tool constructors.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::command_policy::CommandPolicyHook;
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::exec_approval::gate::ApprovalGate;
use crate::sandbox::hooks::SandboxHooks;
use crate::sandbox::rate_limit::{RateLimitHook, SandboxRateLimitConfig, SandboxRateLimiter};
use crate::sandbox::workspace::WorkspaceSandbox;
use crate::sandbox::Sandbox;

/// Assemble the shared `Arc<dyn Sandbox>` during application boot.
///
/// When `cfg.enabled` is `false`, returns a no-op sandbox that
/// refuses execution. Production boot enables the real
/// `WorkspaceSandbox` which provisions per-session workspaces under
/// `cfg.workspace_root` and routes exec-class tool calls through the
/// provided OS sandbox driver with capability escalation arbitrated
/// by `approval_gate`.
pub fn build_sandbox(
    cfg: &SandboxConfig,
    driver: Arc<dyn OsSandboxDriverTrait>,
    approval: Arc<ApprovalGate>,
    rate_limit_config: SandboxRateLimitConfig,
    shell_security: &crate::config::types::ShellSecurityConfig,
) -> Arc<dyn Sandbox> {
    if !cfg.enabled {
        return Arc::new(NoopSandbox);
    }
    let rate_limiter = Arc::new(SandboxRateLimiter::new(rate_limit_config));
    let mut hooks = SandboxHooks::new();

    // Advisory shell-security layer: Panel-managed `[security]` custom patterns.
    // Installed BEFORE the command policy so an explicitly user-blocked command
    // is vetoed first. A malformed custom-rule regex fails *safe* (logged and
    // skipped) rather than aborting sandbox construction.
    let kernel = crate::exec::SecurityKernel::from_config(shell_security);
    if shell_security.enable_custom_patterns {
        hooks = hooks.with_before(Arc::new(
            crate::sandbox::security_kernel_hook::SecurityKernelHook::new(kernel),
        ));
    }

    // Command-policy hard-filter runs FIRST so a catastrophic command is
    // refused before it consumes rate-limit budget or reaches the OS driver.
    // A malformed custom-rule regex fails *safe*: we log and fall back to the
    // curated defaults rather than booting with no content filter. Every arm
    // installs a hook carrying at least the catastrophic hardline floor — no
    // config path leaves disk-wipe / fork-bomb protection off.
    match cfg.command_policy.clone().into_policy() {
        Ok(Some(policy)) => {
            hooks = hooks.with_before(Arc::new(CommandPolicyHook::new(policy)));
        }
        Ok(None) => {
            // User disabled the *tunable* policy, but the catastrophic floor is
            // not negotiable: install a hardline-only hook so fork bombs / disk
            // wipes are still refused before reaching the OS driver.
            tracing::info!(
                target: "command_policy",
                "command policy disabled by config — hardline catastrophic floor remains active"
            );
            hooks = hooks.with_before(Arc::new(CommandPolicyHook::new(
                crate::sandbox::command_policy::CommandPolicy::hardline_only(),
            )));
        }
        Err(e) => {
            tracing::error!(
                target: "command_policy",
                error = %e,
                "command policy config invalid — falling back to default ruleset"
            );
            let policy = crate::sandbox::command_policy::CommandPolicy::defaults(
                cfg.command_policy.enforcement,
            );
            hooks = hooks.with_before(Arc::new(CommandPolicyHook::new(policy)));
        }
    }

    let mut hooks = hooks.with_before(Arc::new(RateLimitHook::new(rate_limiter)));

    // Load-aware admission gate for heavy (Dangerous-class) execution.
    // Sourced from `cfg` (no extra `build_sandbox` parameter → no caller
    // churn). Dormant unless `[sandbox.resource_governor] enabled = true`, so
    // boot behaviour is unchanged by default.
    let governor =
        crate::sandbox::resource_governor::ResourceGovernor::from_schema(&cfg.resource_governor);
    if governor.is_enabled() {
        hooks = hooks.with_before(Arc::new(governor));
    }

    let ws = WorkspaceSandbox::new(cfg.workspace_root.clone(), driver, approval, hooks)
        .with_timeout(Duration::from_secs(cfg.default_timeout_seconds))
        .with_max_output_bytes(cfg.max_output_bytes);
    Arc::new(ws)
}

/// Stub sandbox used when the subsystem is disabled. Returns a
/// structured error for every `execute` call so that misconfigured
/// setups fail fast rather than silently escaping sandboxing.
#[derive(Debug, Default)]
pub struct NoopSandbox;

#[async_trait]
impl Sandbox for NoopSandbox {
    async fn execute(&self, _command: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        Err(SandboxError::Other(
            "sandbox disabled: set [sandbox] enabled = true in config to execute commands".into(),
        ))
    }

    fn summary(&self) -> Option<crate::sandbox::summary::SandboxSummary> {
        // Explicit "off" signal so the LLM does not believe enforcement is
        // active when it is not. Mirrors codex's `permission_profile_sandbox_tag = "none"`.
        Some(crate::sandbox::summary::SandboxSummary {
            backend: "none/disabled",
            policy_tier: crate::sandbox::summary::PolicyTier::DangerFullAccess.as_str(),
            writable_roots: Vec::new(),
            network: crate::sandbox::summary::NetworkState::AllowAll,
            max_memory_mb: None,
            permission_profile_id: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;
    use crate::sandbox::capabilities::SandboxCapabilities;
    use crate::sandbox::driver::OsSandboxProfile;
    use crate::sandbox::rate_limit::SandboxRateLimitConfig;

    /// Minimal driver used for factory tests — never invoked because the
    /// factory tests do not call `execute`.
    struct UnusedDriver;

    #[async_trait]
    impl OsSandboxDriverTrait for UnusedDriver {
        fn platform(&self) -> &'static str {
            "unused"
        }

        fn is_supported(&self) -> bool {
            false
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
            unreachable!("factory tests should not invoke driver.run");
        }
    }

    fn make_gate() -> Arc<ApprovalGate> {
        Arc::new(ApprovalGate::new(None))
    }

    fn make_sid() -> crate::session::service::SessionId {
        crate::routing::session_key::SessionKey::ephemeral("factory-test")
    }

    #[tokio::test]
    async fn build_sandbox_with_enabled_false_returns_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            workspace_root: tmp.path().to_path_buf(),
            enabled: false,
            default_timeout_seconds: 60,
            max_output_bytes: 1024,
            deny_read_globs: Vec::new(),
            allow_unix_sockets: Vec::new(),
            dangerously_allow_all_unix_sockets: false,
            allow_local_binding: false,
            linux: Default::default(),
            windows: Default::default(),
            rate_limit: Default::default(),
            resource_governor: Default::default(),
            command_policy: Default::default(),
        };
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(UnusedDriver);
        let sandbox = build_sandbox(
            &cfg,
            driver,
            make_gate(),
            SandboxRateLimitConfig::default(),
            &crate::config::types::ShellSecurityConfig::default(),
        );

        // NoopSandbox::execute must surface a structured error — it never
        // reaches the OS driver, so the workspace dir must remain absent.
        let err = sandbox
            .execute(SandboxCommand {
                session_id: make_sid(),
                program: "echo".into(),
                args: vec!["hi".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect_err("disabled sandbox must refuse execution");
        assert!(matches!(err, SandboxError::Other(ref msg) if msg.contains("sandbox disabled")));
        // WorkspaceSandbox would have created a session dir — Noop must not.
        assert!(
            std::fs::read_dir(tmp.path())
                .map(|d| d.count())
                .unwrap_or(0)
                == 0,
            "NoopSandbox must not create any session directories"
        );
    }

    #[tokio::test]
    async fn build_sandbox_with_enabled_true_returns_workspace_sandbox() {
        // Build with enabled = true; verify the factory composes the real
        // WorkspaceSandbox by confirming it materialises a session workspace
        // lazily — behaviour that NoopSandbox does not exhibit.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            workspace_root: tmp.path().to_path_buf(),
            enabled: true,
            default_timeout_seconds: 60,
            max_output_bytes: 1024,
            deny_read_globs: Vec::new(),
            allow_unix_sockets: Vec::new(),
            dangerously_allow_all_unix_sockets: false,
            allow_local_binding: false,
            linux: Default::default(),
            windows: Default::default(),
            rate_limit: Default::default(),
            resource_governor: Default::default(),
            command_policy: Default::default(),
        };
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeRunDriver);
        let sandbox = build_sandbox(
            &cfg,
            driver,
            make_gate(),
            SandboxRateLimitConfig::default(),
            &crate::config::types::ShellSecurityConfig::default(),
        );

        let mut before = 0;
        let mut before_dir = tokio::fs::read_dir(tmp.path()).await.unwrap();
        while let Ok(Some(_)) = before_dir.next_entry().await {
            before += 1;
        }
        assert_eq!(before, 0, "no session dirs before first execute");

        sandbox
            .execute(SandboxCommand {
                session_id: make_sid(),
                program: "echo".into(),
                args: vec!["hi".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect("enabled sandbox should execute via driver");

        let mut after = 0;
        let mut after_dir = tokio::fs::read_dir(tmp.path()).await.unwrap();
        while let Ok(Some(_)) = after_dir.next_entry().await {
            after += 1;
        }
        assert_eq!(
            after, 1,
            "WorkspaceSandbox must create exactly one session dir on first execute"
        );
    }

    /// Wiring check: with the default config the command-policy hook is
    /// installed via `build_sandbox`, so a catastrophic command is refused
    /// before it reaches the OS driver.
    #[tokio::test]
    async fn build_sandbox_installs_command_policy_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            workspace_root: tmp.path().to_path_buf(),
            enabled: true,
            default_timeout_seconds: 60,
            max_output_bytes: 1024,
            deny_read_globs: Vec::new(),
            allow_unix_sockets: Vec::new(),
            dangerously_allow_all_unix_sockets: false,
            allow_local_binding: false,
            linux: Default::default(),
            windows: Default::default(),
            rate_limit: Default::default(),
            resource_governor: Default::default(),
            command_policy: Default::default(),
        };
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeRunDriver);
        let sandbox = build_sandbox(
            &cfg,
            driver,
            make_gate(),
            SandboxRateLimitConfig::default(),
            &crate::config::types::ShellSecurityConfig::default(),
        );

        let err = sandbox
            .execute(SandboxCommand {
                session_id: make_sid(),
                program: "bash".into(),
                args: vec!["-c".into(), ":(){ :|:& };:".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect_err("fork bomb must be refused by the command policy hook");
        // WorkspaceSandbox maps a hook denial to SandboxError::Other.
        assert!(
            matches!(err, SandboxError::Other(ref m) if m.contains("command policy")),
            "unexpected error: {err}"
        );
    }

    /// Wiring check: even when the operator disables `[sandbox.command_policy]`,
    /// `build_sandbox` installs the undisableable hardline floor, so a
    /// catastrophic command is still refused before it reaches the OS driver.
    #[tokio::test]
    async fn build_sandbox_disabled_command_policy_still_blocks_hardline() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SandboxConfig {
            workspace_root: tmp.path().to_path_buf(),
            enabled: true,
            default_timeout_seconds: 60,
            max_output_bytes: 1024,
            deny_read_globs: Vec::new(),
            allow_unix_sockets: Vec::new(),
            dangerously_allow_all_unix_sockets: false,
            allow_local_binding: false,
            linux: Default::default(),
            windows: Default::default(),
            rate_limit: Default::default(),
            resource_governor: Default::default(),
            command_policy: crate::sandbox::command_policy::CommandPolicyConfigSchema {
                enabled: false,
                ..Default::default()
            },
        };
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeRunDriver);
        let sandbox = build_sandbox(
            &cfg,
            driver,
            make_gate(),
            SandboxRateLimitConfig::default(),
            &crate::config::types::ShellSecurityConfig::default(),
        );

        let err = sandbox
            .execute(SandboxCommand {
                session_id: make_sid(),
                program: "bash".into(),
                args: vec!["-c".into(), "dd if=/dev/zero of=/dev/sda".into()],
                env: HashMap::new(),
                stdin: None,
                cwd: None,
                capabilities: SandboxCapabilities::strict(),
                timeout: None,
            })
            .await
            .expect_err("hardline floor must refuse dd even with the policy disabled");
        assert!(
            matches!(err, SandboxError::Other(ref m) if m.contains("dd_to_block_device")),
            "unexpected error: {err}"
        );
    }

    /// Driver that returns a canned success response — used in the
    /// enabled-path test so `WorkspaceSandbox::execute` can finish.
    #[derive(Default)]
    struct FakeRunDriver;

    #[async_trait]
    impl OsSandboxDriverTrait for FakeRunDriver {
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
            Ok(SandboxOutput {
                stdout: b"ok".to_vec(),
                exit_code: Some(0),
                duration_ms: 1,
                ..Default::default()
            })
        }
    }
}
