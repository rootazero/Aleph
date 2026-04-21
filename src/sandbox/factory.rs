//! `build_sandbox` — compose the `Arc<dyn Sandbox>` held by `AppContext`.
//!
//! Produces a `WorkspaceSandbox` when `SandboxConfig::enabled` is `true`
//! and falls back to `NoopSandbox` otherwise. Task 8 will thread the
//! returned `Arc<dyn Sandbox>` into exec-class tool constructors.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::sandbox::exec_approval::gate::ApprovalGate;
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::driver::OsSandboxDriverTrait;
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
) -> Arc<dyn Sandbox> {
    if !cfg.enabled {
        return Arc::new(NoopSandbox);
    }
    let ws = WorkspaceSandbox::new(cfg.workspace_root.clone(), driver, approval)
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
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use super::*;
    use crate::sandbox::exec_approval::types::ApprovalConfig;
    use crate::sandbox::capabilities::SandboxCapabilities;
    use crate::sandbox::driver::OsSandboxProfile;

    /// Minimal driver used for factory tests — never invoked because the
    /// factory tests do not call `execute`.
    struct UnusedDriver;

    #[async_trait]
    impl OsSandboxDriverTrait for UnusedDriver {
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
            unreachable!("factory tests should not invoke driver.run");
        }
    }

    fn make_gate() -> Arc<ApprovalGate> {
        Arc::new(ApprovalGate::new(ApprovalConfig::default(), None))
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
        };
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(UnusedDriver);
        let sandbox = build_sandbox(&cfg, driver, make_gate());

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
        };
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeRunDriver::default());
        let sandbox = build_sandbox(&cfg, driver, make_gate());

        let before = std::fs::read_dir(tmp.path())
            .map(|d| d.count())
            .unwrap_or(0);
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

        let after = std::fs::read_dir(tmp.path())
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(
            after, 1,
            "WorkspaceSandbox must create exactly one session dir on first execute"
        );
    }

    /// Driver that returns a canned success response — used in the
    /// enabled-path test so `WorkspaceSandbox::execute` can finish.
    #[derive(Default)]
    struct FakeRunDriver;

    #[async_trait]
    impl OsSandboxDriverTrait for FakeRunDriver {
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
}
