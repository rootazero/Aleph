//! Code execution tool for AI agent integration
//!
//! Implements `AlephTool` trait to provide code/script execution capabilities.
//! Supports: Python, JavaScript/Node.js, Shell (bash).
//!
//! # Safety
//!
//! This tool routes subprocess execution through `Arc<dyn Sandbox>` (Phase 3
//! Task 8). The sandbox enforces:
//! - Capability-level approval for escalations (network / extra `fs_write` /
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
use crate::tool_output::sanitize::sanitize_command_output;
use crate::tools::AlephTool;

use super::command_canonicalize::canonicalize_shell_cmd;
use super::command_ledger::command_ledger;

/// Threshold above which a shell script switches from `bash -c <script>`
/// to `bash -s` reading the script from stdin. Linux's `ARG_MAX` for a
/// single argv element (`MAX_ARG_STRLEN`) is typically 128 KiB; we keep a
/// 4× margin to leave room for the rest of the argv vector plus env.
const SHELL_STDIN_PIPE_THRESHOLD: usize = 32 * 1024;

/// Wall-clock ceiling (seconds) applied to a **foreground** exec so the
/// sandbox's own timeout fires *before* the 180s per-tool budget wrapper
/// (`tools::budget::builtin_tool_budget_ms` = 180_000 for `bash`/`code_exec`).
///
/// Without this clamp a foreground call with an over-long `timeout` (e.g.
/// `bash(cmd, timeout=600)`) sets the sandbox timeout to 600s, so the outer
/// budget wrapper trips first and aborts the whole call with a misleading
/// "slow or unresponsive source — no result" — discarding the partial output
/// the sandbox's exit-124 timeout path would otherwise preserve. Clamping to
/// 170s (10s under the budget, room for the kill + 2s drain) means an
/// over-long foreground `timeout` instead yields a clean `exit_code = 124`
/// with partial output. Background jobs escape the budget wrapper entirely and
/// are NOT clamped (see `BashExecTool::spawn_background` → `call_unclamped`);
/// for anything longer than this, background mode is the right tool. Mirrors
/// the `wait`-window clamp (`WAIT_MAX_TIMEOUT_SECS`) in `bash_exec`.
const FOREGROUND_MAX_TIMEOUT_SECS: u64 = 170;

/// Clamp a caller-supplied foreground `timeout` under the tool-budget ceiling.
/// `None` is preserved (the sandbox applies `DEFAULT_CODE_EXEC_TIMEOUT`); a
/// value at or below the ceiling passes through unchanged.
fn clamp_foreground_timeout(timeout: Option<u64>) -> Option<u64> {
    timeout.map(|t| t.min(FOREGROUND_MAX_TIMEOUT_SECS))
}

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
    const fn runtime(&self) -> &'static str {
        match self {
            Self::Python => "python3",
            Self::JavaScript => "node",
            Self::Shell => "bash",
        }
    }

    const fn code_flag(&self) -> &'static str {
        match self {
            Self::Python => "-c",
            Self::JavaScript => "-e",
            Self::Shell => "-c",
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
    /// Timeout in seconds (optional, defaults to 60). Accepts the legacy
    /// `timeout` spelling.
    #[serde(default, alias = "timeout")]
    pub timeout_seconds: Option<u64>,
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
    /// Optional natural-language reason for *why* an escalation
    /// (`allow_network` / `allow_subprocess` / `extra_writable_paths`) is
    /// needed. Surfaced to the human approver alongside the requested
    /// capabilities so they can make an informed decision. Ignored when the
    /// call requests no escalation. codex `justification` parity.
    #[serde(default)]
    pub justification: Option<String>,
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
    /// Advisory note attached by the tool layer, NOT produced by the command
    /// itself — e.g. a heads-up that this exact shell command was already run
    /// moments ago in this session (see `command_ledger`). Omitted from JSON
    /// when absent so it only appears when there is something to say.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub advisory: Option<String>,
}

const fn is_zero(v: &u64) -> bool {
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
cross-call state, write it to a file under `working_dir`. Re-running a shell
command you already ran this session comes back with an `advisory` field
flagging it — don't repeat unless you expect the output to have changed.

`working_dir` (optional) is resolved inside the session workspace; paths
outside the workspace are denied. Defaults to the workspace root.

`timeout` defaults to 60s; foreground calls are clamped to ~170s (just under
the 180s tool budget) so an over-long `timeout` returns a clean
`exit_code = 124` with partial output, not a "no result" abort. On
timeout the runtime is killed, stdout/stderr are drained for up to 2s,
and we return `exit_code = 124` (POSIX `timeout(1)` convention) with the
partial output preserved so you can see what the script accomplished.

Output is capped per stream; when a stream overflows we keep BOTH its
head and its tail (with a `…[N bytes elided]…` marker between them), so a
build that prints megabytes of progress and then fails still shows you the
final error — not just the start. The response also carries
`stdout_truncated_bytes` / `stderr_truncated_bytes` so you know exactly how
much was elided. ANSI colour codes and stray binary control bytes are
stripped automatically — no need for `--color=never` or piping through `cat`.

If the process is killed by a signal it surfaces as `exit_code = 128 + N`
(POSIX convention) with a `stderr` note naming the signal — e.g. `137`
(SIGKILL, usually an out-of-memory or resource-limit kill), `139` (SIGSEGV,
a crash), `134` (SIGABRT, an assertion/panic abort).

Capability escalations (`allow_network`, `allow_subprocess`,
`extra_writable_paths`) require approval the first time per session. When you
escalate, pass `justification` with a one-line reason WHY (e.g. "fetch crates
from crates.io for cargo build") — it is shown to the human approver so they
can decide; calls without an escalation can omit it.

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
    #[must_use]
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

    /// Execute WITHOUT the foreground timeout clamp. Used only by
    /// [`BashExecTool::spawn_background`](super::bash_exec), whose detached task
    /// runs *outside* the 180s per-tool budget wrapper and so may legitimately
    /// run up to the background ceiling (1h default, or an explicit `timeout`).
    /// The clamp applied in [`AlephTool::call`] would otherwise cut every
    /// backgrounded build/install off at 170s.
    pub(crate) async fn call_unclamped(&self, args: CodeExecArgs) -> Result<CodeExecOutput> {
        self.execute(args).await
    }

    /// Execute code and return result
    async fn execute(&self, args: CodeExecArgs) -> Result<CodeExecOutput> {
        // A worktree-isolated subagent scopes its `WorktreeSandbox` here via the
        // task-local override so this command runs inside the checkout; outside
        // that scope the tool falls back to its construction-time sandbox.
        let sandbox = match crate::sandbox::context::current_sandbox_override()
            .or_else(|| self.sandbox.clone())
        {
            Some(s) => s,
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
                    advisory: None,
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
                    advisory: None,
                });
            }
        };

        let timeout_secs = args.timeout_seconds.unwrap_or(DEFAULT_CODE_EXEC_TIMEOUT);
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

        // Carry the model's escalation justification to the approval prompt via
        // the EXEC_JUSTIFICATION task-local (read by `WorkspaceSandbox` when it
        // formats the human approval request). Only scope it when the model
        // actually supplied a non-blank reason — otherwise the prompt stays
        // byte-identical to its pre-justification form. Setting it innermost
        // (right around `execute`) means the background path inherits it too,
        // since `spawn_background` re-enters this method inside its task.
        let result = match args
            .justification
            .as_deref()
            .map(str::trim)
            .filter(|j| !j.is_empty())
        {
            Some(why) => {
                crate::sandbox::context::EXEC_JUSTIFICATION
                    .scope(why.to_string(), sandbox.execute(cmd))
                    .await
            }
            None => sandbox.execute(cmd).await,
        };
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
/// Resolve the exit code to surface to the model, plus an optional note when
/// the process was killed by a signal.
///
/// On Unix `ExitStatus::code()` returns `None` for a signal death, so the
/// sandbox records the signal separately in `SandboxOutput.signal`. Without
/// reading it, every SIGKILL / SIGSEGV / OOM kill collapsed to `exit_code: -1`
/// with no hint. We mirror the POSIX `128 + signal` convention (the same value
/// codex synthesises and that bash itself reports via `$?`), and annotate the
/// likely cause so the model doesn't have to guess whether `-1` meant a crash,
/// an OOM kill, or a clean failure.
///
/// Only universal POSIX signal semantics are annotated — never command-specific
/// exit-code guesses (e.g. "grep exited 1 so no match"), which would mean
/// pattern-matching the command string and violate R7 (LLM sovereignty).
fn resolve_exit_code(exit_code: Option<i32>, signal: Option<i32>) -> (i32, Option<String>) {
    match (exit_code, signal) {
        // Normal exit (including a clean non-zero) — the code speaks for itself.
        (Some(code), _) => (code, None),
        // Killed by a signal: synthesise 128+N and explain it.
        (None, Some(sig)) => {
            let surfaced = 128 + sig;
            let name = signal_name(sig);
            let hint = signal_hint(sig);
            let note = format!(
                "Process was killed by signal {sig} ({name}){hint} — surfaced as exit code {surfaced} (POSIX 128+signal convention)."
            );
            (surfaced, Some(note))
        }
        // No code and no signal — nothing better than the legacy sentinel.
        (None, None) => (-1, None),
    }
}

/// Map a Unix signal number to its conventional name. Covers the signals a
/// sandboxed build/test/script realistically dies from; anything else is
/// reported by number with an `unknown` label.
const fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        _ => "unknown",
    }
}

/// A short, model-facing cause hint for the signals with a common,
/// actionable interpretation. Empty for signals where naming it is enough.
const fn signal_hint(sig: i32) -> &'static str {
    match sig {
        6 => " — an abort (assertion failure, Rust panic with abort, or abort())",
        9 => " — typically an out-of-memory kill or an enforced memory/CPU resource limit",
        11 => " — a segmentation fault; the process crashed",
        13 => " — wrote to a closed pipe (a downstream reader exited early)",
        15 => " — terminated by an external request",
        24 => " — CPU time limit exceeded",
        25 => " — file size limit exceeded",
        _ => "",
    }
}

fn sandbox_result_to_output(
    result: std::result::Result<SandboxOutput, SandboxError>,
    language: String,
    timeout_secs: u64,
) -> CodeExecOutput {
    match result {
        Ok(out) => {
            // Strip ANSI/VT100 escapes + stray binary control bytes before the
            // text enters the agent envelope (pi/openclaw parity; clean output
            // stays byte-identical via the borrow fast-path).
            let stdout =
                sanitize_command_output(&String::from_utf8_lossy(&out.stdout)).into_owned();
            let mut stderr =
                sanitize_command_output(&String::from_utf8_lossy(&out.stderr)).into_owned();
            // Wire the signal the sandbox captured but no consumer read.
            // `ExitStatus::code()` is `None` on Unix when a signal killed the
            // child, so `out.exit_code.unwrap_or(-1)` previously flattened a
            // SIGKILL/SIGSEGV/OOM into a meaningless `-1` with no explanation.
            // Surface `128 + signal` (POSIX convention, codex parity) and append
            // a human-readable note so the model can tell an OOM kill from a
            // clean failure.
            let (exit_code, signal_note) = resolve_exit_code(out.exit_code, out.signal);
            if let Some(note) = signal_note {
                if stderr.is_empty() {
                    stderr = note;
                } else {
                    stderr = format!("{stderr}\n{note}");
                }
            }

            debug!(
                exit_code = exit_code,
                signal = out.signal,
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
                advisory: None,
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
            // Same sanitization as the natural-exit path — partial output is
            // just as agent-facing.
            let stdout =
                sanitize_command_output(&String::from_utf8_lossy(&partial_stdout)).into_owned();
            let partial_stderr_text =
                sanitize_command_output(&String::from_utf8_lossy(&partial_stderr)).into_owned();
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
                advisory: None,
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
            advisory: None,
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
            advisory: None,
        },
    }
}

impl Default for CodeExecTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of `AlephTool` trait for `CodeExecTool`
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
        // Foreground path: clamp `timeout` under the per-tool budget so an
        // over-long value yields a clean exit-124 (with partial output) from
        // the sandbox's own timeout, not a "no result" budget-overrun abort.
        // The background path escapes the budget wrapper and calls
        // `call_unclamped` directly, keeping its generous ceiling.
        let mut args = args;
        args.timeout_seconds = clamp_foreground_timeout(args.timeout_seconds);

        // Advisory repeat-detection for shell commands (foreground only — the
        // background path uses `call_unclamped`, and a deliberate long-running
        // background job is not a wasteful re-run). Recorded BEFORE execute so
        // the submission timestamp reflects when the model asked; it never
        // gates execution — the command always runs regardless (R7).
        let advisory = recent_shell_advisory(&args.language, &args.code);

        let mut out = self.execute(args).await?;
        if advisory.is_some() {
            out.advisory = advisory;
        }
        Ok(out)
    }
}

/// Stable per-session key for the recent-command ledger. `None` outside a
/// session scope (some CLI / test paths) — those simply get no advisory.
fn session_ledger_key() -> Option<String> {
    current_session().map(|sid| serde_json::to_string(&sid).unwrap_or_else(|_| format!("{sid:?}")))
}

/// Return a repeat advisory when this exact shell command was already run in
/// the current session within the ledger's recency window. Shell only — other
/// languages' re-runs usually follow an edit and are never flagged. Pure
/// mechanical string equality on the canonicalized script (advisory only,
/// never a gate: R7).
fn recent_shell_advisory(language: &Language, code: &str) -> Option<String> {
    if !matches!(language, Language::Shell) {
        return None;
    }
    let key = canonicalize_shell_cmd(code).script.trim().to_string();
    if key.is_empty() {
        return None;
    }
    let session = session_ledger_key()?;
    command_ledger()
        .record(&session, &key)
        .map(|advisory| advisory.message())
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

    /// TDD RED: `timeout_seconds` is the canonical spelling; the legacy bare
    /// `timeout` must still parse via `#[serde(alias = "timeout")]` so saved
    /// calls / prompts don't break (deserialize-only, no schema change).
    #[test]
    fn code_exec_timeout_accepts_canonical_and_legacy_alias() {
        let a: CodeExecArgs = serde_json::from_value(serde_json::json!({
            "language": "shell",
            "code": "true",
            "timeout_seconds": 30
        }))
        .unwrap();
        assert_eq!(a.timeout_seconds, Some(30));
        let b: CodeExecArgs = serde_json::from_value(serde_json::json!({
            "language": "shell",
            "code": "true",
            "timeout": 30
        }))
        .unwrap();
        assert_eq!(b.timeout_seconds, Some(30));
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

    /// The repeat advisory is gated to `Language::Shell`: a re-run shell
    /// command is flagged, but a re-run python/js command (whose repeats
    /// usually follow an edit) never is. No sandbox needed — the advisory is
    /// attached in `call` around `execute`, whose no-sandbox path still
    /// returns `Ok`. Unique ephemeral session isolates the global ledger.
    #[tokio::test]
    async fn advisory_is_shell_only() {
        let session = crate::routing::session_key::SessionKey::ephemeral("code-exec-advisory-gate");
        SESSION_ID
            .scope(session, async {
                let tool = CodeExecTool::new();
                let args = |language, code: &str| CodeExecArgs {
                    language,
                    code: code.to_string(),
                    working_dir: None,
                    timeout_seconds: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                };
                // Shell: first run clean, identical re-run flagged.
                assert!(tool
                    .call(args(Language::Shell, "ls -la"))
                    .await
                    .unwrap()
                    .advisory
                    .is_none());
                assert!(
                    tool.call(args(Language::Shell, "ls -la"))
                        .await
                        .unwrap()
                        .advisory
                        .is_some(),
                    "identical shell re-run should be flagged"
                );
                // Python: never flagged, even on an identical re-run.
                assert!(tool
                    .call(args(Language::Python, "print(1)"))
                    .await
                    .unwrap()
                    .advisory
                    .is_none());
                assert!(
                    tool.call(args(Language::Python, "print(1)"))
                        .await
                        .unwrap()
                        .advisory
                        .is_none(),
                    "python re-runs are never flagged (Shell-only gate)"
                );
            })
            .await;
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
        assert!(
            d.contains("justification"),
            "should teach passing a justification when escalating"
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
                timeout_seconds: Some(5),
                allow_network: false,
                allow_subprocess: false,
                extra_writable_paths: Vec::new(),
                justification: None,
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
                timeout_seconds: Some(5),
                allow_network: false,
                allow_subprocess: false,
                extra_writable_paths: Vec::new(),
                justification: None,
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
                    timeout_seconds: Some(3),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
                    timeout_seconds: None,
                    allow_network: true,
                    allow_subprocess: true,
                    extra_writable_paths: vec!["/tmp/out".into()],
                    justification: None,
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
                    timeout_seconds: Some(3),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
                    timeout_seconds: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
                    timeout_seconds: Some(10),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
                    timeout_seconds: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
                    timeout_seconds: Some(5),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
                    timeout_seconds: Some(1),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
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
    async fn ansi_and_binary_are_stripped_from_command_output() {
        // A colourised, control-byte-laden build line must reach the model
        // clean — pi/openclaw parity. Stage runs through the real sandbox
        // result mapping, not just the pure helper.
        struct NoisySandbox;
        #[async_trait::async_trait]
        impl Sandbox for NoisySandbox {
            async fn execute(
                &self,
                _cmd: SandboxCommand,
            ) -> std::result::Result<SandboxOutput, SandboxError> {
                Ok(SandboxOutput {
                    stdout: "\u{1b}[32mPASS\u{1b}[0m\u{0} 12 tests\n"
                        .as_bytes()
                        .to_vec(),
                    stderr: "\u{1b}[31mwarn\u{1b}[0m\u{7}\n".as_bytes().to_vec(),
                    exit_code: Some(0),
                    duration_ms: 3,
                    ..Default::default()
                })
            }
        }

        let sandbox: Arc<dyn Sandbox> = Arc::new(NoisySandbox);
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        let out = SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "cargo test".to_string(),
                    working_dir: None,
                    timeout_seconds: Some(5),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                })
                .await
                .unwrap()
            })
            .await;
        assert_eq!(out.stdout, "PASS 12 tests\n", "ANSI + NUL stripped");
        assert_eq!(out.stderr, "warn\n", "ANSI + bell stripped");
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
                    timeout_seconds: Some(1),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                })
                .await
                .unwrap()
            })
            .await;
        assert_eq!(out.stdout_truncated_bytes, 4242);
        assert_eq!(out.stderr_truncated_bytes, 7);
        assert_eq!(out.truncated, Some(true));
    }

    /// A sandbox that records whatever `current_justification()` reports during
    /// `execute` — proves the tool scopes the EXEC_JUSTIFICATION task-local
    /// around the sandbox call (the approval-prompt enrichment depends on this).
    struct JustificationProbe(Arc<std::sync::Mutex<Option<String>>>);
    #[async_trait::async_trait]
    impl Sandbox for JustificationProbe {
        async fn execute(
            &self,
            _cmd: SandboxCommand,
        ) -> std::result::Result<SandboxOutput, SandboxError> {
            *self.0.lock().unwrap() = crate::sandbox::current_justification();
            Ok(SandboxOutput {
                exit_code: Some(0),
                ..Default::default()
            })
        }
    }

    async fn justification_seen_by_sandbox(justification: Option<String>) -> Option<String> {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let sandbox: Arc<dyn Sandbox> = Arc::new(JustificationProbe(seen.clone()));
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "curl https://crates.io".to_string(),
                    working_dir: None,
                    timeout_seconds: Some(5),
                    allow_network: true,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification,
                })
                .await
                .unwrap()
            })
            .await;
        let v = seen.lock().unwrap().clone();
        v
    }

    #[tokio::test]
    async fn justification_is_scoped_around_execute() {
        let seen = justification_seen_by_sandbox(Some("fetch deps".to_string())).await;
        assert_eq!(seen.as_deref(), Some("fetch deps"));
    }

    #[tokio::test]
    async fn absent_justification_leaves_task_local_unset() {
        assert_eq!(justification_seen_by_sandbox(None).await, None);
    }

    #[tokio::test]
    async fn blank_justification_is_not_scoped() {
        // Whitespace-only ⇒ filtered out, never scoped — approver falls back to
        // the capabilities-only prompt.
        assert_eq!(
            justification_seen_by_sandbox(Some("   \n  ".to_string())).await,
            None
        );
    }

    #[test]
    fn resolve_exit_code_passes_through_normal_exit() {
        // A real exit code (zero or non-zero) is reported verbatim with no note.
        assert_eq!(resolve_exit_code(Some(0), None), (0, None));
        assert_eq!(resolve_exit_code(Some(1), None), (1, None));
        // A present code wins even if a signal was also (spuriously) recorded.
        assert_eq!(resolve_exit_code(Some(2), Some(9)), (2, None));
    }

    #[test]
    fn resolve_exit_code_synthesises_128_plus_signal() {
        // The core BUG fix: a signal death had no exit code, so it used to
        // flatten to -1. Now it surfaces 128+signal with an explanatory note.
        let (code, note) = resolve_exit_code(None, Some(9));
        assert_eq!(code, 137, "SIGKILL → 128 + 9");
        let note = note.expect("signal death must carry a note");
        assert!(note.contains("SIGKILL"), "names the signal: {note}");
        assert!(
            note.contains("out-of-memory") || note.contains("resource limit"),
            "explains the likely cause: {note}"
        );

        let (code, note) = resolve_exit_code(None, Some(11));
        assert_eq!(code, 139, "SIGSEGV → 128 + 11");
        assert!(note.unwrap().contains("segmentation fault"));
    }

    #[test]
    fn resolve_exit_code_falls_back_to_sentinel() {
        // No code and no signal: nothing better than the legacy -1 sentinel,
        // and crucially no spurious note.
        assert_eq!(resolve_exit_code(None, None), (-1, None));
    }

    #[test]
    fn sandbox_output_surfaces_signal_death_end_to_end() {
        // Drive the whole mapping: a sandbox result with no exit code but a
        // captured signal must reach the model as exit 137 + a stderr note,
        // and must be marked unsuccessful.
        let out = sandbox_result_to_output(
            Ok(SandboxOutput {
                stdout: b"partial progress\n".to_vec(),
                stderr: Vec::new(),
                exit_code: None,
                signal: Some(9),
                ..Default::default()
            }),
            "shell".to_string(),
            60,
        );
        assert!(!out.success, "a signal death is never a success");
        assert_eq!(out.exit_code, 137);
        assert_eq!(out.stdout, "partial progress\n", "stdout is preserved");
        assert!(
            out.stderr.contains("SIGKILL") && out.stderr.contains("137"),
            "stderr explains the signal death: {}",
            out.stderr
        );
    }

    #[test]
    fn sandbox_output_appends_signal_note_to_existing_stderr() {
        // When the process already printed to stderr before dying, the note is
        // appended rather than clobbering the real diagnostic.
        let out = sandbox_result_to_output(
            Ok(SandboxOutput {
                stdout: Vec::new(),
                stderr: b"thread panicked at 'boom'".to_vec(),
                exit_code: None,
                signal: Some(6),
                ..Default::default()
            }),
            "shell".to_string(),
            60,
        );
        assert_eq!(out.exit_code, 134, "SIGABRT → 128 + 6");
        assert!(out.stderr.contains("thread panicked at 'boom'"));
        assert!(out.stderr.contains("SIGABRT"));
    }

    #[test]
    fn clamp_foreground_timeout_caps_over_long_but_preserves_smaller() {
        // None → None: the sandbox applies DEFAULT_CODE_EXEC_TIMEOUT downstream.
        assert_eq!(clamp_foreground_timeout(None), None);
        // At/under the ceiling passes through unchanged.
        assert_eq!(clamp_foreground_timeout(Some(30)), Some(30));
        assert_eq!(
            clamp_foreground_timeout(Some(FOREGROUND_MAX_TIMEOUT_SECS)),
            Some(FOREGROUND_MAX_TIMEOUT_SECS)
        );
        // Over the ceiling is clamped so the sandbox timeout fires before the
        // 180s per-tool budget wrapper — a clean exit-124, not a hard abort.
        assert_eq!(
            clamp_foreground_timeout(Some(600)),
            Some(FOREGROUND_MAX_TIMEOUT_SECS)
        );
    }

    #[tokio::test]
    async fn foreground_over_long_timeout_is_clamped_under_budget() {
        // A foreground code_exec/bash call asking for 600s must hand the
        // sandbox only ~170s so the sandbox's own timeout produces a clean
        // exit-124 (with partial output) before the 180s budget wrapper aborts
        // the whole call with a "no result". Contrast background, which escapes
        // the wrapper and keeps its generous ceiling.
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "sleep 999".to_string(),
                    working_dir: None,
                    timeout_seconds: Some(600),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        assert_eq!(
            calls[0].timeout,
            Some(Duration::from_secs(FOREGROUND_MAX_TIMEOUT_SECS)),
            "foreground timeout must be clamped under the tool budget"
        );
    }

    #[tokio::test]
    async fn call_unclamped_preserves_long_timeout_for_background() {
        // The background entry point must NOT clamp — a backgrounded build may
        // run for the full ceiling. `call_unclamped` hands the sandbox the raw
        // 600s value.
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call_unclamped(CodeExecArgs {
                    language: Language::Shell,
                    code: "sleep 999".to_string(),
                    working_dir: None,
                    timeout_seconds: Some(600),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        assert_eq!(
            calls[0].timeout,
            Some(Duration::from_secs(600)),
            "background path must keep the caller's long timeout"
        );
    }
}
