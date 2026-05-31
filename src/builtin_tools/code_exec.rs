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
//! Dangerous command handling is the responsibility of the sandbox layer, in
//! two complementary stages: a content-level hard-filter
//! (`sandbox::command_policy`, a `SandboxBeforeHook` that refuses catastrophic
//! command patterns up front) and OS-level enforcement (`WorkspaceSandbox` +
//! seatbelt/bwrap/job-object). This tool is a thin adapter from `CodeExecArgs`
//! to `SandboxCommand`.

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::{current_session, Sandbox};
use crate::tool_metadata::DEFAULT_CODE_EXEC_TIMEOUT;
use crate::tools::AlephTool;

use super::command_canonicalize::canonicalize_shell_cmd;

/// Threshold above which a shell script switches from `bash -c <script>`
/// to `bash -s` reading the script from stdin. Linux's `ARG_MAX` for a
/// single argv element (`MAX_ARG_STRLEN`) is typically 128 KiB; we keep a
/// 4× margin to leave room for the rest of the argv vector plus env.
const SHELL_STDIN_PIPE_THRESHOLD: usize = 32 * 1024;

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
            max_memory_mb: None,
            timeout_secs: None,
        }
    }
}

/// Output from code execution tool.
///
/// `truncated` stays for back-compat; new callers should inspect
/// `stdout_truncated_bytes` / `stderr_truncated_bytes` for the exact
/// number of bytes that were dropped per stream — codex-style
/// "what did I lose" visibility.
///
/// On timeout: `success` is `false`, `exit_code` is `124` (POSIX
/// `timeout(1)` convention so the model can pattern-match), `stdout`
/// carries whatever the process printed before the kill, and `stderr`
/// is a human-readable message that also embeds the partial stderr
/// captured during the IO-drain window.
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
    /// Bytes dropped from stdout to satisfy the sandbox cap.
    /// Omitted from JSON when zero so existing consumers stay quiet.
    #[serde(skip_serializing_if = "is_zero", default)]
    pub stdout_truncated_bytes: u64,
    /// Bytes dropped from stderr to satisfy the sandbox cap.
    #[serde(skip_serializing_if = "is_zero", default)]
    pub stderr_truncated_bytes: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
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

    /// Tool description for AI prompt — also used at the trait-impl
    /// site below so the `bash` wrapper inherits the same teaching.
    pub const DESCRIPTION: &'static str = r#"Execute code in a per-session sandboxed workspace. Supported languages:
- python: runs via `python3 -c <code>`
- javascript: runs via `node -e <code>`
- shell: runs via `bash -c <code>` (or `bash -s` over stdin for scripts >32 KB)

Multi-line code is first-class for all three languages. For shell, prefer
ONE multi-line script (newlines, heredocs, pipelines, `set -e`) over many
small calls — each call is a fresh process, so `cd`, env vars, virtualenv
activation, and similar state do NOT persist between calls. If you need
cross-call state, write it to a file under `working_dir`.

`working_dir` (optional) is resolved inside the session workspace; paths
outside the workspace are denied. Defaults to the workspace root.

`timeout` defaults to 60s, capped by the tool budget (180s ceiling). On
timeout the runtime is killed, stdout/stderr are drained for up to 2s,
and we return `exit_code = 124` (POSIX `timeout(1)` convention) with the
partial output preserved so you can see what the script accomplished.

Output is capped per stream; the response carries
`stdout_truncated_bytes` / `stderr_truncated_bytes` when bytes were
dropped, so you know exactly how much you lost.

Capability escalations (`allow_network`, `allow_subprocess`,
`extra_writable_paths`) require approval the first time per session.

Examples:
- Python: {"language": "python", "code": "print('Hello, World!')"}
- JavaScript: {"language": "javascript", "code": "console.log('Hello, World!')"}
- Shell (single-line): {"language": "shell", "code": "ls -la"}
- Shell (multi-line): {"language": "shell", "code": "set -e\ncd src\ncargo check\necho ok"}
- Shell (heredoc): {"language": "shell", "code": "cat <<'EOF' > /tmp/x\nhello\nEOF\nwc -l /tmp/x"}
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
                    stdout_truncated_bytes: 0,
                    stderr_truncated_bytes: 0,
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
                    stdout_truncated_bytes: 0,
                    stderr_truncated_bytes: 0,
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
        let invocation = build_exec_invocation(&args.language, &args.code);
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
            program: invocation.program,
            args: invocation.args,
            env,
            stdin: invocation.stdin,
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

/// Shape of the runtime invocation derived from `(language, code)`.
/// Threaded into `SandboxCommand` so the sandbox sees the final argv +
/// stdin without the tool layer knowing about platform mechanics.
#[derive(Debug, Clone)]
struct ExecInvocation {
    program: String,
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
}

/// Build the (`program`, `args`, `stdin`) triple for the runtime that
/// matches `language`. For `Shell`, applies command canonicalization
/// (peel any outer `bash -lc '...'` wrapper) and switches to a stdin
/// pipe (`bash -s`) when the script is large enough to risk hitting
/// `ARG_MAX`. Python/JavaScript paths are unchanged.
fn build_exec_invocation(language: &Language, code: &str) -> ExecInvocation {
    match language {
        Language::Shell => build_shell_invocation(code),
        Language::Python => ExecInvocation {
            program: language.runtime().to_string(),
            args: vec![language.code_flag().to_string(), code.to_string()],
            stdin: None,
        },
        Language::JavaScript => ExecInvocation {
            program: language.runtime().to_string(),
            args: vec![language.code_flag().to_string(), code.to_string()],
            stdin: None,
        },
    }
}

fn build_shell_invocation(code: &str) -> ExecInvocation {
    let canonical = canonicalize_shell_cmd(code);
    if let Some(wrapper) = canonical.unwrapped_from {
        debug!(
            wrapper = wrapper,
            "code_exec: peeled outer shell wrapper before exec"
        );
    }
    let script = canonical.script.into_owned();

    if script.len() > SHELL_STDIN_PIPE_THRESHOLD {
        debug!(
            script_bytes = script.len(),
            threshold = SHELL_STDIN_PIPE_THRESHOLD,
            "code_exec: shell script exceeds threshold — piping via bash -s + stdin"
        );
        ExecInvocation {
            program: "bash".to_string(),
            args: vec!["-s".to_string()],
            stdin: Some(script.into_bytes()),
        }
    } else {
        ExecInvocation {
            program: "bash".to_string(),
            args: vec!["-c".to_string(), script],
            stdin: None,
        }
    }
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
                stdout_truncated_bytes = out.stdout_truncated_bytes,
                stderr_truncated_bytes = out.stderr_truncated_bytes,
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
                stdout_truncated_bytes: out.stdout_truncated_bytes,
                stderr_truncated_bytes: out.stderr_truncated_bytes,
            }
        }
        Err(SandboxError::Timeout {
            elapsed_ms,
            partial_stdout,
            partial_stderr,
        }) => {
            warn!(
                timeout_secs = timeout_secs,
                elapsed_ms = elapsed_ms,
                partial_stdout_bytes = partial_stdout.len(),
                partial_stderr_bytes = partial_stderr.len(),
                "Sandbox code execution timed out — surfacing partial output"
            );
            let stdout = String::from_utf8_lossy(&partial_stdout).to_string();
            let partial_stderr_text = String::from_utf8_lossy(&partial_stderr).to_string();
            // Frame the human-readable banner so the model can see _that_
            // it timed out, and inline the captured partial stderr (if
            // any) so it knows _what the script printed_ on its way out.
            let stderr = if partial_stderr_text.is_empty() {
                format!(
                    "Execution timed out after {timeout_secs}s (exit code 124, POSIX timeout convention). Partial stdout above was captured before the kill; partial stderr was empty."
                )
            } else {
                format!(
                    "Execution timed out after {timeout_secs}s (exit code 124, POSIX timeout convention). Partial stderr captured before kill:\n{partial_stderr_text}"
                )
            };
            CodeExecOutput {
                success: false,
                // POSIX `timeout(1)` exit convention — codex returns the
                // same so the model can pattern-match across both stacks.
                exit_code: 124,
                stdout,
                stderr,
                duration_ms: elapsed_ms,
                language,
                truncated: None,
                stdout_truncated_bytes: 0,
                stderr_truncated_bytes: 0,
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
            stdout_truncated_bytes: 0,
            stderr_truncated_bytes: 0,
        },
        Err(err) => CodeExecOutput {
            success: false,
            exit_code: -1,
            stdout: String::new(),
            stderr: format!("Sandbox error: {err}"),
            duration_ms: 0,
            language,
            truncated: None,
            stdout_truncated_bytes: 0,
            stderr_truncated_bytes: 0,
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
    // Share the rich teaching with the inherent `DESCRIPTION` const so
    // there's a single source of truth — keeps the prompt and the
    // type-API description in lock-step.
    const DESCRIPTION: &'static str = Self::DESCRIPTION;

    type Args = CodeExecArgs;
    type Output = CodeExecOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.execute(args).await
    }
}

#[cfg(test)]
mod tests {
    use crate::sync_primitives::Arc;

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
            exit_code: Some(0),
            duration_ms: 7,
            ..Default::default()
        }
    }

    #[test]
    fn test_language_runtime() {
        assert_eq!(Language::Python.runtime(), "python3");
        assert_eq!(Language::JavaScript.runtime(), "node");
        assert_eq!(Language::Shell.runtime(), "bash");
    }

    #[test]
    fn description_is_single_sourced() {
        // The AlephTool::DESCRIPTION should be the same string as the
        // inherent const so there's no drift between the type API and
        // what the model actually sees in the prompt.
        assert_eq!(
            CodeExecTool::DESCRIPTION,
            <CodeExecTool as AlephTool>::DESCRIPTION
        );
    }

    #[test]
    fn description_teaches_partial_output_and_stateless_sessions() {
        let d = CodeExecTool::DESCRIPTION;
        assert!(d.contains("multi-line"), "should encourage multi-line code");
        assert!(d.contains("heredoc"), "should mention heredoc for shell");
        assert!(
            d.contains("32 KB"),
            "should mention the stdin-pipe threshold"
        );
        assert!(
            d.contains("124"),
            "should document the POSIX timeout exit code"
        );
        assert!(
            d.contains("preserved"),
            "should promise partial output on kill"
        );
        assert!(
            d.contains("stdout_truncated_bytes"),
            "should mention the explicit truncation byte fields"
        );
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
    async fn shell_wrapper_is_canonicalized_before_exec() {
        // LLM emits `bash -lc 'cargo test'` as a single cmd string. With
        // canonicalization the sandbox sees `["-c", "cargo test"]`, not
        // the doubly-wrapped `["-c", "bash -lc 'cargo test'"]`.
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "bash -lc 'cargo test'".to_string(),
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

        let calls = mock.calls.lock().await;
        let cmd = &calls[0];
        assert_eq!(cmd.program, "bash");
        assert_eq!(cmd.args, vec!["-c".to_string(), "cargo test".to_string()]);
        assert!(cmd.stdin.is_none());
    }

    #[tokio::test]
    async fn unrecognized_wrapper_passes_through_unchanged() {
        // `bash -c "echo $(date)"` has a double-quoted script with command
        // substitution — too risky to peel. We pass through, so the
        // sandbox sees the literal wrapper string.
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "bash -c \"echo $(date)\"".to_string(),
                    working_dir: None,
                    timeout: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        let cmd = &calls[0];
        assert_eq!(
            cmd.args,
            vec!["-c".to_string(), "bash -c \"echo $(date)\"".to_string()]
        );
    }

    #[tokio::test]
    async fn large_shell_script_pipes_via_stdin() {
        // ~40 KiB script. Below this we use `bash -c <script>`; above, we
        // switch to `bash -s` and feed the script via stdin to dodge
        // ARG_MAX limits on Linux (`MAX_ARG_STRLEN = 128 KiB`).
        let big_script = format!("# header\n{}\n", "echo hi\n".repeat(5_000));
        assert!(big_script.len() > super::SHELL_STDIN_PIPE_THRESHOLD);

        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: big_script.clone(),
                    working_dir: None,
                    timeout: Some(10),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        let cmd = &calls[0];
        assert_eq!(cmd.program, "bash");
        assert_eq!(cmd.args, vec!["-s".to_string()]);
        assert_eq!(
            cmd.stdin.as_deref(),
            Some(big_script.as_bytes()),
            "large script should arrive on stdin, not argv"
        );
    }

    #[tokio::test]
    async fn python_path_is_unaffected_by_shell_canonicalize() {
        // Regression: only Language::Shell takes the new path.
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Python,
                    code: "bash -lc 'print(1)'".to_string(),
                    working_dir: None,
                    timeout: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        let cmd = &calls[0];
        assert_eq!(cmd.program, "python3");
        // Python literally receives the string — no canonicalization.
        assert_eq!(
            cmd.args,
            vec!["-c".to_string(), "bash -lc 'print(1)'".to_string()]
        );
    }

    #[tokio::test]
    async fn sandbox_timeout_surfaces_partial_output_and_posix_exit() {
        // Simulate the helper having drained partial output before the
        // kill: stdout has "started" (so the model can see what the
        // script accomplished) and stderr has a warning line.
        struct TimeoutSandbox;
        #[async_trait::async_trait]
        impl Sandbox for TimeoutSandbox {
            async fn execute(
                &self,
                _cmd: SandboxCommand,
            ) -> std::result::Result<SandboxOutput, SandboxError> {
                Err(SandboxError::Timeout {
                    elapsed_ms: 1234,
                    partial_stdout: b"started step 1\n".to_vec(),
                    partial_stderr: b"warning: foo\n".to_vec(),
                })
            }
        }

        let sandbox: Arc<dyn Sandbox> = Arc::new(TimeoutSandbox);
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        let out = SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "echo started step 1; sleep 9999".to_string(),
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
        // POSIX timeout convention (codex parity) — the model can
        // pattern-match `exit_code == 124` instead of parsing strings.
        assert_eq!(out.exit_code, 124);
        assert_eq!(
            out.stdout, "started step 1\n",
            "partial stdout must surface verbatim"
        );
        assert!(
            out.stderr.contains("timed out after 5s"),
            "stderr banner missing timeout signal: {}",
            out.stderr
        );
        assert!(
            out.stderr.contains("warning: foo"),
            "stderr must embed partial stderr captured during drain: {}",
            out.stderr
        );
        assert_eq!(out.duration_ms, 1234);
    }

    #[tokio::test]
    async fn timeout_with_empty_partial_streams_still_explains() {
        struct TimeoutSandbox;
        #[async_trait::async_trait]
        impl Sandbox for TimeoutSandbox {
            async fn execute(
                &self,
                _cmd: SandboxCommand,
            ) -> std::result::Result<SandboxOutput, SandboxError> {
                Err(SandboxError::Timeout {
                    elapsed_ms: 500,
                    partial_stdout: Vec::new(),
                    partial_stderr: Vec::new(),
                })
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
                    timeout: Some(1),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;
        assert_eq!(out.exit_code, 124);
        assert!(out.stdout.is_empty());
        assert!(
            out.stderr.contains("partial stderr was empty"),
            "drain-empty banner missing: {}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn truncation_bytes_propagate_to_output() {
        // SandboxOutput carries explicit dropped byte counts now; the
        // tool envelope should pass them through unchanged so the model
        // can see exactly how much it lost.
        struct TruncatingSandbox;
        #[async_trait::async_trait]
        impl Sandbox for TruncatingSandbox {
            async fn execute(
                &self,
                _cmd: SandboxCommand,
            ) -> std::result::Result<SandboxOutput, SandboxError> {
                Ok(SandboxOutput {
                    stdout: b"hello".to_vec(),
                    stderr: Vec::new(),
                    exit_code: Some(0),
                    truncated: true,
                    stdout_truncated_bytes: 4242,
                    stderr_truncated_bytes: 7,
                    duration_ms: 9,
                    ..Default::default()
                })
            }
        }

        let sandbox: Arc<dyn Sandbox> = Arc::new(TruncatingSandbox);
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        let out = SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "yes".to_string(),
                    working_dir: None,
                    timeout: Some(1),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                })
                .await
                .unwrap()
            })
            .await;
        assert_eq!(out.stdout_truncated_bytes, 4242);
        assert_eq!(out.stderr_truncated_bytes, 7);
        assert_eq!(out.truncated, Some(true));
    }
}
