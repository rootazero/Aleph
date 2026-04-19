//! Code execution tool for AI agent integration
//!
//! Implements AlephTool trait to provide code/script execution capabilities.
//! Supports: Python, JavaScript/Node.js, Shell (bash).
//!
//! # Safety
//!
//! This tool routes subprocess execution through `Arc<dyn Sandbox>` (Phase 3
//! Task 8). The sandbox enforces:
//! - Capability-level approval for escalations (network / extra fs_write /
//!   subprocess spawn) via the shared `ApprovalGate`
//! - Per-session workspace cwd — `cwd=None` lands in the session workspace
//!   root materialised lazily by `WorkspaceSandbox`
//! - macOS seatbelt profile + per-command timeout + output truncation
//!
//! Dangerous command blocking remains the responsibility of `ExecSecurityGate`
//! at the executor layer; this tool is now a thin adapter from `CodeExecArgs`
//! to `SandboxCommand`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::dispatcher::DEFAULT_CODE_EXEC_TIMEOUT;
use crate::error::Result;
use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::{current_session, Sandbox};
use crate::tools::AlephTool;

/// Supported programming languages
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    /// Python (uses python3)
    Python,
    /// JavaScript (uses node)
    JavaScript,
    /// Shell script (uses bash)
    Shell,
}

impl Language {
    fn runtime(&self) -> &'static str {
        match self {
            Language::Python => "python3",
            Language::JavaScript => "node",
            Language::Shell => "bash",
        }
    }

    fn code_flag(&self) -> &'static str {
        match self {
            Language::Python => "-c",
            Language::JavaScript => "-e",
            Language::Shell => "-c",
        }
    }
}

/// Arguments for code execution tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CodeExecArgs {
    /// The programming language to use
    pub language: Language,
    /// The code to execute
    pub code: String,
    /// Working directory (optional, defaults to session workspace root).
    /// Must live under the session workspace — paths outside are denied.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Timeout in seconds (optional, defaults to 60)
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Request elevated network access for this call. Triggers a single
    /// capability-approval dialog via `ApprovalGate` on first use in a session;
    /// subsequent same-or-narrower requests reuse the cached grant.
    #[serde(default)]
    pub allow_network: bool,
    /// Request permission for the spawned runtime to fork subprocesses.
    /// Like `allow_network`, requires user approval the first time.
    #[serde(default)]
    pub allow_subprocess: bool,
    /// Extra writable paths (beyond the session workspace). Each entry is
    /// presented in the approval prompt so the user can decide.
    #[serde(default)]
    pub extra_writable_paths: Vec<std::path::PathBuf>,
}

impl CodeExecArgs {
    /// Derive the `SandboxCapabilities` this invocation is asking for.
    /// Baseline (`strict()`) stays in-session; escalations route through the
    /// approval gate inside `WorkspaceSandbox::execute`.
    fn as_capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            fs_read: Vec::new(),
            fs_write: self.extra_writable_paths.clone(),
            network: if self.allow_network {
                NetworkPolicy::AllowAll
            } else {
                NetworkPolicy::None
            },
            spawn_subprocess: self.allow_subprocess,
        }
    }
}

/// Output from code execution tool
#[derive(Debug, Clone, Serialize)]
pub struct CodeExecOutput {
    pub success: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Code execution tool. Holds `Arc<dyn Sandbox>` (Phase 3 Task 8) so every
/// invocation routes through the Sandbox seam — capability enforcement,
/// per-session workspace, OS seatbelt profile, and `capability_ledger`
/// tracing audit all live there rather than in the tool.
#[derive(Clone)]
pub struct CodeExecTool {
    /// Allowed environment variables to forward into the sandboxed process.
    /// Kept small and explicit — the sandbox clears the child environment.
    pass_env: Vec<String>,
    /// Shared sandbox. `None` preserves the zero-argument `CodeExecTool::new()`
    /// constructor used by `AlephToolServer::with_code_exec` and test harnesses
    /// that don't wire a sandbox; calls made while unconfigured surface a
    /// structured error rather than spawning directly.
    sandbox: Option<Arc<dyn Sandbox>>,
}

impl CodeExecTool {
    /// Tool identifier
    pub const NAME: &'static str = "code_exec";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = r#"Execute code in various programming languages. Supported languages:
- python: Execute Python 3 code
- javascript: Execute JavaScript code using Node.js
- shell: Execute shell commands using bash

Timeout: Default 60 seconds, configurable.

Examples:
- Python: {"language": "python", "code": "print('Hello, World!')"}
- JavaScript: {"language": "javascript", "code": "console.log('Hello, World!')"}
- Shell: {"language": "shell", "code": "echo 'Hello, World!' && ls -la"}
"#;

    /// Create a new code execution tool without a sandbox wired in yet.
    /// Boot wiring injects the real `Arc<dyn Sandbox>` via
    /// [`CodeExecTool::with_sandbox`]; unconfigured instances will refuse
    /// execution with a clear error rather than bypass sandboxing.
    pub fn new() -> Self {
        Self {
            pass_env: default_pass_env(),
            sandbox: None,
        }
    }

    /// Attach a shared `Arc<dyn Sandbox>` — consumed by boot wiring to thread
    /// the single `WorkspaceSandbox` built in `build_sandbox(...)` through to
    /// this tool and its `BashExecTool` wrapper.
    pub fn with_sandbox(mut self, sandbox: Arc<dyn Sandbox>) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// Execute code and return result
    async fn execute(&self, args: CodeExecArgs) -> Result<CodeExecOutput> {
        let sandbox = match self.sandbox.as_ref() {
            Some(s) => s.clone(),
            None => {
                return Ok(CodeExecOutput {
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: "code_exec: sandbox not configured — boot wiring must inject Arc<dyn Sandbox>".into(),
                    duration_ms: 0,
                    language: language_label(&args.language),
                    truncated: None,
                });
            }
        };

        // Phase 3 Task 8: session id is carried by SandboxCommand, populated
        // from the task-local set by `invoke_with_session_trace`. Outside a
        // session scope the tool cannot target a per-session workspace and
        // must refuse rather than silently escape the sandbox.
        let session_id = match current_session() {
            Some(sid) => sid,
            None => {
                return Ok(CodeExecOutput {
                    success: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: "code_exec: no active session context — this tool must be invoked via invoke_with_session_trace".into(),
                    duration_ms: 0,
                    language: language_label(&args.language),
                    truncated: None,
                });
            }
        };

        let timeout_secs = args.timeout.unwrap_or(DEFAULT_CODE_EXEC_TIMEOUT);
        let language_label = language_label(&args.language);

        info!(
            language = %language_label,
            code_length = args.code.len(),
            allow_network = args.allow_network,
            allow_subprocess = args.allow_subprocess,
            "Executing code via Sandbox"
        );

        // Build SandboxCommand. `env` is the additive overrides; the sandbox
        // itself decides what base environment the child sees.
        let program = args.language.runtime().to_string();
        let code_flag = args.language.code_flag().to_string();
        let code = args.code.clone();
        let cwd = validate_working_dir(args.working_dir.as_deref());

        let mut env = HashMap::new();
        for name in &self.pass_env {
            if name == "PATH" {
                if let Ok(enhanced) = crate::runtimes::ledger::build_enhanced_path() {
                    env.insert("PATH".to_string(), enhanced);
                    continue;
                }
            }
            if let Ok(value) = std::env::var(name) {
                env.insert(name.clone(), value);
            }
        }

        let cmd = SandboxCommand {
            session_id,
            program,
            args: vec![code_flag, code],
            env,
            stdin: None,
            cwd,
            capabilities: args.as_capabilities(),
            timeout: Some(Duration::from_secs(timeout_secs)),
        };

        let result = sandbox.execute(cmd).await;
        Ok(sandbox_result_to_output(
            result,
            language_label,
            timeout_secs,
        ))
    }
}

/// Resolve a user-supplied `working_dir` string into the `cwd` field of a
/// `SandboxCommand`. `None` lets the sandbox route the call into the session
/// workspace root — cwd validity (within workspace) is enforced by
/// `WorkspaceSandbox`. Empty / unparseable strings fall back to `None` so
/// the tool never bypasses sandbox policy with a raw host path.
fn validate_working_dir(raw: Option<&str>) -> Option<std::path::PathBuf> {
    raw.filter(|s| !s.is_empty()).map(std::path::PathBuf::from)
}

fn default_pass_env() -> Vec<String> {
    vec![
        "PATH".to_string(),
        "HOME".to_string(),
        "USER".to_string(),
        "LANG".to_string(),
        "LC_ALL".to_string(),
        "TERM".to_string(),
    ]
}

fn language_label(lang: &Language) -> String {
    match lang {
        Language::Python => "python",
        Language::JavaScript => "javascript",
        Language::Shell => "shell",
    }
    .to_string()
}

/// Map `Result<SandboxOutput, SandboxError>` to the tool's `CodeExecOutput`
/// envelope. All recoverable failures surface as `success = false` with
/// `exit_code = -1` and a human-readable `stderr` — matching the pre-Sandbox
/// behaviour so existing callers see no shape changes.
fn sandbox_result_to_output(
    result: std::result::Result<SandboxOutput, SandboxError>,
    language: String,
    timeout_secs: u64,
) -> CodeExecOutput {
    match result {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let exit_code = out.exit_code.unwrap_or(-1);

            debug!(
                exit_code = exit_code,
                duration_ms = out.duration_ms,
                truncated = out.truncated,
                "Code execution completed"
            );

            CodeExecOutput {
                success: exit_code == 0,
                exit_code,
                stdout,
                stderr,
                duration_ms: out.duration_ms,
                language,
                truncated: if out.truncated { Some(true) } else { None },
            }
        }
        Err(SandboxError::Timeout { elapsed_ms }) => {
            warn!(
                timeout_secs = timeout_secs,
                elapsed_ms = elapsed_ms,
                "Sandbox code execution timed out"
            );
            CodeExecOutput {
                success: false,
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Execution timed out after {} seconds", timeout_secs),
                duration_ms: elapsed_ms,
                language,
                truncated: None,
            }
        }
        Err(SandboxError::CapabilityDenied { reason }) => CodeExecOutput {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("Capability denied: {reason}"),
            duration_ms: 0,
            language,
            truncated: None,
        },
        Err(err) => CodeExecOutput {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("Sandbox error: {err}"),
            duration_ms: 0,
            language,
            truncated: None,
        },
    }
}

impl Default for CodeExecTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of AlephTool trait for CodeExecTool
#[async_trait]
impl AlephTool for CodeExecTool {
    const NAME: &'static str = "code_exec";
    const DESCRIPTION: &'static str = r#"Execute code in various programming languages. Supported languages:
- python: Execute Python 3 code
- javascript: Execute JavaScript code using Node.js
- shell: Execute shell commands using bash

Timeout: Default 60 seconds, configurable."#;

    type Args = CodeExecArgs;
    type Output = CodeExecOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.execute(args).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::sandbox::capabilities::NetworkPolicy;
    use crate::sandbox::context::SESSION_ID;
    use crate::sandbox::test_util::MockSandbox;
    use crate::sandbox::{Sandbox, SandboxOutput};

    use super::*;

    fn sid() -> crate::session::service::SessionId {
        crate::routing::session_key::SessionKey::ephemeral("code-exec-test")
    }

    fn ok_output(stdout: &str) -> SandboxOutput {
        SandboxOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            signal: None,
            truncated: false,
            duration_ms: 7,
        }
    }

    #[test]
    fn test_language_runtime() {
        assert_eq!(Language::Python.runtime(), "python3");
        assert_eq!(Language::JavaScript.runtime(), "node");
        assert_eq!(Language::Shell.runtime(), "bash");
    }

    #[tokio::test]
    async fn call_without_sandbox_returns_structured_error() {
        let tool = CodeExecTool::new();
        let out = tool
            .call(CodeExecArgs {
                language: Language::Shell,
                code: "echo hi".to_string(),
                working_dir: None,
                timeout: Some(5),
                allow_network: false,
                allow_subprocess: false,
                extra_writable_paths: Vec::new(),
            })
            .await
            .expect("tool returns structured failure, not Err");
        assert!(!out.success);
        assert_eq!(out.exit_code, -1);
        assert!(
            out.stderr.contains("sandbox not configured"),
            "unexpected stderr: {}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn call_without_session_scope_returns_structured_error() {
        let sandbox: Arc<dyn Sandbox> = MockSandbox::new(ok_output("ok"));
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        let out = tool
            .call(CodeExecArgs {
                language: Language::Shell,
                code: "echo hi".to_string(),
                working_dir: None,
                timeout: Some(5),
                allow_network: false,
                allow_subprocess: false,
                extra_writable_paths: Vec::new(),
            })
            .await
            .unwrap();
        assert!(!out.success);
        assert!(
            out.stderr.contains("no active session context"),
            "unexpected stderr: {}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn call_with_sandbox_routes_through_execute() {
        let mock = MockSandbox::new(ok_output("hello\n"));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        let session = sid();
        let out = SESSION_ID
            .scope(session.clone(), async {
                tool.call(CodeExecArgs {
                    language: Language::Python,
                    code: "print('hello')".to_string(),
                    working_dir: None,
                    timeout: Some(3),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;

        assert!(out.success);
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.stdout, "hello\n");

        let calls = mock.calls.lock().await;
        assert_eq!(calls.len(), 1, "sandbox should be invoked once");
        let cmd = &calls[0];
        assert_eq!(cmd.session_id, session);
        assert_eq!(cmd.program, "python3");
        assert_eq!(
            cmd.args,
            vec!["-c".to_string(), "print('hello')".to_string()]
        );
        assert_eq!(cmd.capabilities.network, NetworkPolicy::None);
        assert!(!cmd.capabilities.spawn_subprocess);
        assert_eq!(cmd.timeout, Some(Duration::from_secs(3)));
    }

    #[tokio::test]
    async fn allow_network_escalates_capabilities() {
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "curl https://example.com".to_string(),
                    working_dir: None,
                    timeout: None,
                    allow_network: true,
                    allow_subprocess: true,
                    extra_writable_paths: vec!["/tmp/out".into()],
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        let cmd = &calls[0];
        assert_eq!(cmd.capabilities.network, NetworkPolicy::AllowAll);
        assert!(cmd.capabilities.spawn_subprocess);
        assert_eq!(
            cmd.capabilities.fs_write,
            vec![std::path::PathBuf::from("/tmp/out")]
        );
    }

    #[tokio::test]
    async fn sandbox_timeout_surfaces_as_structured_failure() {
        struct TimeoutSandbox;
        #[async_trait::async_trait]
        impl Sandbox for TimeoutSandbox {
            async fn execute(
                &self,
                _cmd: SandboxCommand,
            ) -> std::result::Result<SandboxOutput, SandboxError> {
                Err(SandboxError::Timeout { elapsed_ms: 1234 })
            }
        }

        let sandbox: Arc<dyn Sandbox> = Arc::new(TimeoutSandbox);
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        let out = SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "sleep 9999".to_string(),
                    working_dir: None,
                    timeout: Some(5),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;

        assert!(!out.success);
        assert_eq!(out.exit_code, -1);
        assert!(
            out.stderr.contains("timed out"),
            "unexpected stderr: {}",
            out.stderr
        );
        assert_eq!(out.duration_ms, 1234);
    }
}
