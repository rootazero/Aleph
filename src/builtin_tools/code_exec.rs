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
use crate::sandbox::command::{SandboxCommand, SandboxDenialHint, SandboxError, SandboxOutput};
use crate::sandbox::{current_session, Sandbox};
use crate::tool_metadata::DEFAULT_CODE_EXEC_TIMEOUT;
use crate::tool_output::sanitize::sanitize_command_output;
use crate::tools::AlephTool;

use crate::utils::shell::STDIN_PIPE_THRESHOLD;

use super::command_canonicalize::canonicalize_shell_cmd;
use super::command_ledger::command_ledger;

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
///
/// `pub(super)` so `code_check` (and any future sibling) can apply the same
/// clamp at its own entry point instead of duplicating the constant.
pub(super) fn clamp_foreground_timeout(timeout: Option<u64>) -> Option<u64> {
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

/// The interpreter every shell / `bash` tool call is spawned under.
///
/// Exposed so the prompt's environment envelope
/// ([`RuntimeContext`](crate::thinker::runtime_context::RuntimeContext)) can
/// state the shell the model will actually get, instead of guessing from the
/// operator's `$SHELL` (a login shell the agent never uses, and unset on
/// Windows). This is a **view of** [`crate::utils::shell::resolve`], never a
/// second answer: `build_shell_invocation` spawns `resolve().program` and this
/// returns `resolve().label`, which `utils::shell` pins to that file's stem.
/// So the advertised shell cannot drift from the spawned one — the pair is
/// held by `advertised_shell_is_the_spawned_shell` here and by
/// `label_is_the_stem_of_the_resolved_program` there.
///
/// No longer `const`: on Windows the answer is a probe result (`pwsh` →
/// `powershell` → `cmd`), and the old constant `"bash"` was simply false there.
#[must_use]
pub fn shell_interpreter() -> &'static str {
    crate::utils::shell::resolve().label.as_str()
}

impl Language {
    /// `(program, code flag)` for the languages spawned as
    /// `<program> <flag> <code>`.
    ///
    /// `Shell` is absent **by construction**: its program *and* its argv are
    /// resolved per platform at run time by [`build_shell_invocation`], so a
    /// constant arm here could only be a Windows-side lie that nothing reads.
    const fn interpreter(&self) -> Option<(&'static str, &'static str)> {
        match self {
            // `node` stays a bare name: it is the same program under the same
            // spelling everywhere, and Windows has no Store alias for it.
            Self::JavaScript => Some(("node", "-e")),
            // Absent by construction, both of them: the program is a probe
            // result, not a literal. `Shell` resolves through
            // `utils::shell::resolve`, `Python` through `utils::shell::python3`
            // — a constant arm here could only be a Windows-side lie.
            Self::Shell | Self::Python => None,
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
            // A shell needs fork for ANY composition (`&&`, `|`, `$()`, even
            // a redirection); without it the tool fails selectively, which is
            // how a model mislearns "bash is blocked" from one compound
            // command. The ban never contained anything: bash exec's a single
            // simple command in place, so `rm -rf x` ran under it regardless.
            // `code_check` already ships this same reasoning for its checkers.
            // Interpreters are NOT exempt — node/python are already running
            // when they ask to spawn, so there the gate really does gate.
            //
            // NOT on Linux: there `allow_fork` does not gate forking at all —
            // bwrap spends it on `--unshare-pid` (PID-namespace isolation), and
            // shells fork fine under that. Taking the exemption there would
            // hand a sandboxed shell shared process visibility with every other
            // process of the same uid, trading a control that works for a
            // problem Linux does not have. Measured per platform: macOS
            // `(deny process-fork)` is the incoherent ban this fixes; Windows
            // clamps the job object to 1 active process, the same breakage;
            // Linux is untouched.
            spawn_subprocess: self.allow_subprocess
                || (cfg!(not(target_os = "linux")) && matches!(self.language, Language::Shell)),
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

    /// Tool description for AI prompt. Re-exported at the `AlephTool` impl
    /// below so the type API and the prompt cannot drift apart.
    ///
    /// Deliberately short. `bash` documents the execution contract the two
    /// tools SHARE — statelessness, `working_dir`, `timeout`/exit 124, output
    /// caps and head-tail elision, signal exit codes, escalation approvals —
    /// and both tools are in `default_core_tools()`, so every request that
    /// lists one lists the other and a restatement here would send those bytes
    /// a second time. Only what is true of `code_exec` and NOT of `bash` — the
    /// language table — belongs here; everything else is a pointer.
    ///
    /// Hoisting the shared prose into one const shared by both tools is NOT
    /// the fix and must not be attempted: the budget counts how many times a
    /// sentence is SENT, not how many times it is written, and text behind a
    /// shared const still ships once per tool that references it.
    pub const DESCRIPTION: &'static str = r#"Execute code in a per-session sandboxed workspace. Supported languages:
- python: the host's Python 3
- javascript: `node -e <code>`
- shell: the interpreter named in the Environment block's `- **Shell**:` line
  — `bash` on Unix, PowerShell on Windows, where POSIX syntax does not apply

Multi-line code is first-class in all three. Everything else — stateless
processes, `working_dir`, `timeout` and exit 124, output caps, abnormal-exit
annotation, escalation approvals — is exactly as `bash` documents it, and the
`bash` description is also where the shell dialect for this host is spelled
out.
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

        // Inject Aleph session context into the child environment so any
        // shell / python / node script the model spawns can self-identify.
        // `ALEPH_SESSION_ID` mirrors the per-session workspace key the
        // sandbox already targets — same value, surfaced for the script.
        // `ALEPH_TOOL_NAME` lets a script detect that it is running under
        // Aleph (vs. a plain interactive shell) and opt into defensive
        // paths (e.g. `[[ -n "$ALEPH_SESSION_ID" ]] && rm -rf build/`).
        // Both are sourced from task-locals already on this stack, so there
        // is no extra IPC cost; values are stable for the call.
        env.insert(
            "ALEPH_SESSION_ID".to_string(),
            serde_json::to_string(&session_id).unwrap_or_else(|_| format!("{session_id:?}")),
        );
        // One derivation, two readers. The child script reads it out of its
        // environment to self-identify; the sandbox admission hooks read it off
        // `SandboxCommand::tool_name` to bucket and audit. Computing it twice
        // would be two answers to "which tool is this", and the pair is already
        // pinned by `bash_child_env_carries_aleph_session_and_tool_name`.
        let tool_name = match args.language {
            Language::Shell => "bash",
            Language::Python => "code_exec:python",
            Language::JavaScript => "code_exec:javascript",
        }
        .to_string();
        env.insert("ALEPH_TOOL_NAME".to_string(), tool_name.clone());

        let cmd = SandboxCommand {
            session_id,
            tool_name,
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
        Language::Python => {
            let py = crate::utils::shell::python3();
            let mut args = py.leading.clone();
            args.push("-c".to_string());
            args.push(code.to_string());
            ExecInvocation {
                program: py.program.to_string_lossy().into_owned(),
                args,
                stdin: None,
            }
        }
        Language::JavaScript => {
            let (program, flag) = language
                .interpreter()
                .expect("JavaScript names its interpreter as a literal");
            ExecInvocation {
                program: program.to_string(),
                args: vec![flag.to_string(), code.to_string()],
                stdin: None,
            }
        }
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

    // Resolved once per process. `program` is the ABSOLUTE path: the sandbox
    // `env_clear()`s the child and hands it a rebuilt PATH, so a bare name
    // would be resolved against an environment we constructed rather than the
    // one we probed. `label` is the same object's prompt-facing half — see
    // `shell_interpreter`.
    let shell = crate::utils::shell::resolve();
    let (args, stdin) = shell.kind.invocation(&script);

    if stdin.is_some() {
        debug!(
            script_bytes = script.len(),
            threshold = STDIN_PIPE_THRESHOLD,
            shell = %shell.label,
            "code_exec: shell script exceeds threshold — piping via stdin"
        );
    }

    ExecInvocation {
        program: shell.program_string(),
        args,
        stdin,
    }
}

#[cfg(test)]
mod shell_interpreter_tests {
    use super::{build_shell_invocation, shell_interpreter, STDIN_PIPE_THRESHOLD};
    use crate::utils::shell::{resolve, ShellKind};

    /// The prompt tells the model `- **Shell**: <shell_interpreter()>`. That
    /// claim is only true if the spawn path uses the same resolution — the
    /// program spawned must be the resolved shell's own path, and the label the
    /// prompt shows must be that file (pinned to its stem in `utils::shell`).
    #[test]
    fn advertised_shell_is_the_spawned_shell() {
        let expected = resolve().program_string();
        assert_eq!(build_shell_invocation("echo hi").program, expected);
        assert_eq!(shell_interpreter(), resolve().label);
    }

    /// A script over BOTH thresholds must still reach the shell whole — the
    /// route differs, the delivery does not.
    ///
    /// Asserted as "argv or stdin carries it", not as one shape: bash pipes via
    /// `-s`, PowerShell via `-Command -`, and `cmd` (the floor, which has no
    /// stdin form) keeps it in argv. Pinning one shape here would go red on the
    /// hosts that take the other route, which is how a guard ends up narrowed
    /// to the platform its author happened to be on.
    #[test]
    fn over_threshold_script_still_reaches_the_shell() {
        let big = "x".repeat(STDIN_PIPE_THRESHOLD + 1);
        let piped = build_shell_invocation(&big);
        assert_eq!(piped.program, resolve().program_string());

        let in_argv = piped.args.iter().any(|a| a.contains(&big));
        let in_stdin = piped
            .stdin
            .as_deref()
            .is_some_and(|b| String::from_utf8_lossy(b).contains(&big));
        assert!(
            in_argv || in_stdin,
            "the script must survive the route, whichever it is"
        );

        // The one shape that IS platform-specific and worth pinning: neither
        // PowerShell arm may put a script this size on the command line, where
        // `CreateProcess` would refuse to spawn it at all.
        if matches!(
            resolve().kind,
            ShellKind::Pwsh | ShellKind::WindowsPowerShell
        ) {
            assert!(
                in_stdin,
                "an over-threshold PowerShell script must ride stdin"
            );
        }
    }
}

/// Environment variable names copied from this process into the sandboxed
/// child. The sandbox drivers `env_clear()` first, so this list is not a
/// filter over an inherited environment — it IS the child's environment, and
/// anything absent here is absent in the child.
///
/// The Windows arm is not "nice to have". MEASURED with the POSIX-only list:
/// a Windows child saw `PATHEXT=".CPL"` (PowerShell's internal fallback, with
/// `.EXE` absent) and an empty `TEMP` — so `.cmd` / `.ps1` / even `.exe`
/// resolution and every temp-file API misbehaved, in the shape where the shell
/// starts fine and individual commands fail for no visible reason.
pub(crate) fn default_pass_env() -> Vec<String> {
    const POSIX_PASS_ENV: &[&str] = &["PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM"];

    #[cfg(windows)]
    let names = POSIX_PASS_ENV.iter().chain(WINDOWS_PASS_ENV.iter());
    #[cfg(not(windows))]
    let names = POSIX_PASS_ENV.iter();

    names.map(|name| (*name).to_string()).collect()
}

/// Windows' own baseline environment. Spelled in the canonical mixed case the
/// OS uses; lookup and delivery are both case-insensitive on Windows
/// (`std::env::var` goes through `GetEnvironmentVariableW`, and
/// `std::process::Command`'s env map compares keys case-insensitively there),
/// so a host that stores `SYSTEMROOT` is still found and still delivered once.
#[cfg(windows)]
const WINDOWS_PASS_ENV: &[&str] = &[
    // Load-bearing: without `SystemRoot` the CRT cannot locate the system
    // directory, so temp-file creation and most DLL loads fail.
    "SystemRoot",
    "windir",
    // Load-bearing: `PATHEXT` is the list of extensions a bare command name may
    // resolve to. Absent, the shell falls back to a stub that omits `.EXE`.
    "PATHEXT",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramData",
    "ProgramW6432",
    "COMSPEC",
    "SystemDrive",
    "HOMEDRIVE",
    "HOMEPATH",
    "USERNAME",
    "COMPUTERNAME",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    // PowerShell resolves modules (including `Microsoft.PowerShell.*`) through
    // this; without it a script that imports anything fails to find it.
    "PSModulePath",
];

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
/// Annotate the handful of Windows abnormal-termination codes, the way
/// `signal_name` annotates a POSIX signal death.
///
/// Windows has no signals, so a crash arrives through `ExitStatus::code()` as
/// an ordinary (very negative) integer and reaches the model indistinguishable
/// from a program's own exit code. `-1073741819` names nothing; "access
/// violation" does.
///
/// The number is deliberately left ALONE — no Windows analogue of `128 + N`.
/// That synthesis exists on POSIX only because `code()` is `None` there and
/// something has to be invented; here the OS already gave us a code, and
/// rewriting it would put a second spelling of the same fact in front of the
/// model. This adds the sentence, not a new number.
///
/// Only codes that actually occur under an agent's commands are listed; an
/// exhaustive NTSTATUS table would be a list nobody reads and nobody prunes.
fn ntstatus_note(code: i32) -> Option<String> {
    // Compared as u32: these are NTSTATUS values, and `ExitStatus::code()`
    // hands them back reinterpreted as a negative i32.
    #[allow(clippy::cast_sign_loss)]
    let status = code as u32;
    let meaning = match status {
        0xC000_0005 => "access violation — the program dereferenced bad memory",
        0xC000_0409 => {
            "stack buffer overrun / __fastfail — usually a Rust panic=abort or a CRT assertion"
        }
        0xC000_013A => "terminated by Ctrl+C / Ctrl+Break",
        0xC000_0094 => "integer divide by zero",
        0xC000_008C => "array bounds exceeded",
        0x8000_0003 => "breakpoint — a debugger trap reached with no debugger attached",
        _ => return None,
    };
    Some(format!(
        "Process terminated abnormally: {meaning} (Windows status 0x{status:08X}, surfaced as exit code {code})."
    ))
}

fn resolve_exit_code(exit_code: Option<i32>, signal: Option<i32>) -> (i32, Option<String>) {
    match (exit_code, signal) {
        // Normal exit (including a clean non-zero) — the code speaks for
        // itself, unless it is one of the Windows crash codes below, which
        // speak for the OS instead.
        (Some(code), _) => (code, ntstatus_note(code)),
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

/// One conservative line linking a failed run to the sandbox that may have
/// caused it, and to the escalation parameters this tool's schema already
/// offers.
///
/// Deliberately a *possibility*: [`SandboxDenialHint`] is a substring match on
/// the running backend's dialect, and an application's own permission error
/// (an `ssh` publickey rejection, a `sudo` refusal) is byte-identical to a
/// Landlock one. A zero exit says the process handled whatever it saw, so
/// there is nothing to report.
///
/// Same class as the POSIX signal annotation above — a universal machine fact
/// the model would otherwise have to guess at. It stops at the fact: which
/// escalation to request, or whether to request one at all, stays the model's
/// call (R7), and nothing here retries or selects a recovery (A2 / R10).
fn denial_advisory(hint: Option<&SandboxDenialHint>, exit_code: i32) -> Option<String> {
    if exit_code == 0 {
        return None;
    }
    let hint = hint?;
    Some(format!(
        "Sandbox note: this command exited {exit_code} and its stderr matches the {} sandbox's own denial dialect (\"{}\"), so the sandbox may have blocked a file, network, or subprocess effect — an application's own permission error looks the same. If a wider boundary is what it needs, the escalation parameters are extra_writable_paths / allow_network / allow_subprocess plus a justification; each is approval-gated.",
        hint.platform, hint.signature
    ))
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
                advisory: denial_advisory(out.denial_hint.as_ref(), exit_code),
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
    // Point at the inherent `DESCRIPTION` const so there's a single source
    // of truth — keeps the prompt and the type-API description in lock-step.
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
        // `execute` may already have attached a sandbox-denial label. The two
        // notes answer different questions — what happened to THIS result vs.
        // what this session already ran — so neither may displace the other.
        out.advisory = merge_advisory(out.advisory.take(), advisory);
        Ok(out)
    }
}

/// Join the tool layer's independent advisory notes into the single
/// `advisory` field. Both survive: they are annotations about different
/// things, and the field carrying only one of them is a silent loss.
/// Production order — the note about this result first, the session-history
/// note second.
fn merge_advisory(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}\n{second}")),
        (first, second) => first.or(second),
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

    /// Expected `(program, args, stdin)` for a shell call whose *canonicalized*
    /// script is `script`.
    ///
    /// Derived from the same `utils::shell` contract the production path uses,
    /// so the tests below assert the peeling / routing they are about instead
    /// of hard-coding `bash -c`, which is only one host's answer.
    fn expected_shell_spawn(script: &str) -> (String, Vec<String>, Option<Vec<u8>>) {
        let shell = crate::utils::shell::resolve();
        let (args, stdin) = shell.kind.invocation(script);
        (shell.program_string(), args, stdin)
    }

    #[test]
    fn test_language_runtime() {
        assert_eq!(Language::JavaScript.interpreter(), Some(("node", "-e")));
        // Shell and Python deliberately have no constant interpreter — both are
        // probe results. A literal arm here would be a Windows-side lie, which
        // is what `Some(("python3", "-c"))` was: on Windows that name usually
        // resolves to the Store alias, which exits 49 without running.
        assert_eq!(Language::Shell.interpreter(), None);
        assert_eq!(Language::Python.interpreter(), None);
    }

    /// The sandbox drivers `env_clear()` before `envs(..)`, so this list is the
    /// child's whole environment. Two entries are load-bearing in a way that
    /// fails silently rather than loudly, and both were MEASURED missing:
    ///
    /// * `PATHEXT` — the extensions a bare command name may resolve to.
    ///   Without it the child inherited PowerShell's internal fallback
    ///   `".CPL"`, i.e. `.EXE` was not resolvable and every bare command name
    ///   "did not exist".
    /// * `SystemRoot` — the CRT needs it to find the system directory; without
    ///   it `TEMP` handling and DLL loading break inside the child.
    #[cfg(windows)]
    #[test]
    fn windows_pass_env_carries_the_load_bearing_names() {
        let names = default_pass_env();
        for required in ["PATHEXT", "SystemRoot", "PATH"] {
            assert!(
                names.iter().any(|n| n.eq_ignore_ascii_case(required)),
                "{required} must be passed to the sandboxed child"
            );
        }
        // The real host may spell them in any case; `std::env::var` is
        // case-insensitive on Windows, so our canonical spelling must still
        // find a value here — otherwise we would pass the name and no value.
        assert!(
            std::env::var("SystemRoot").is_ok_and(|v| !v.is_empty()),
            "canonical spelling must resolve against the host's own casing"
        );
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

    /// `code_exec` states its own content and points at `bash` for the rest.
    ///
    /// Until 2026-08-10 this description restated six things `bash` already
    /// says in its own words — statelessness, `working_dir`, `timeout`/exit
    /// 124, the output cap and head-tail elision, signal exit codes, and the
    /// escalation/justification contract — plus a five-line `Examples:` block.
    /// Both tools sit in `default_core_tools()` and `CHAT_CORE_SUBTRACT` keeps
    /// both, so there is no session in which one ships without the other:
    /// every one of those bytes went out twice, every request. This test
    /// asserts the absence, not the presence — it is the only shape that goes
    /// red if the paragraphs creep back.
    #[test]
    fn description_defers_to_bash_instead_of_restating_it() {
        let d = CodeExecTool::DESCRIPTION;

        // Own content: the language table is code_exec's alone. `bash` never
        // mentions python or node, so cutting this would delete it outright.
        // `python` is no longer spelled as a command line here — the program is
        // a probe result (`utils::shell::python3`), and printing `python3 -c`
        // would be this description asserting a name the host may not have.
        // What must survive is the TABLE: three languages, each named.
        assert!(
            d.contains("- python:") && d.contains("- javascript:") && d.contains("- shell:"),
            "the language table is code_exec's own content and must stay"
        );
        assert!(
            d.contains("node -e"),
            "node is the one interpreter still named as a literal, because it is one"
        );

        // The pointer has to name the tool that actually carries the
        // contract. Without it the deleted paragraphs are simply gone, and
        // the model has no way to learn where they went.
        assert!(
            d.contains("`bash`"),
            "the description must point at `bash` for the shared execution \
             contract, or the cut paragraphs are lost rather than deduplicated"
        );

        // Facts `bash` documents in full. A near-repeat here is not free:
        // it is a second copy on the wire in every request.
        for restated in [
            "stdout_truncated_bytes",
            "exit_code = 124",
            "128 + N",
            "SIGKILL",
            "allow_subprocess",
            "justification",
            "Examples:",
        ] {
            assert!(
                !d.contains(restated),
                "`{restated}` is documented by `bash`; restating it in \
                 `code_exec` ships it twice in every request that lists both \
                 tools (both are core, in every session mode)"
            );
        }

        // Non-vacuity: the pointer replaced ~2.4 KB of prose, so a description
        // that has quietly grown back past a kilobyte means the restatement
        // returned under different wording than the literals above.
        assert!(
            d.len() < 1_000,
            "code_exec's description is {} B — it is a language table plus one \
             pointer sentence; anything this large is bash's contract creeping \
             back in different words",
            d.len()
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
        assert_eq!(
            cmd.program,
            crate::utils::shell::python3().program.to_string_lossy()
        );
        assert_eq!(
            cmd.args,
            vec!["-c".to_string(), "print('hello')".to_string()]
        );
        assert_eq!(cmd.capabilities.network, NetworkPolicy::None);
        assert!(!cmd.capabilities.spawn_subprocess);
        assert_eq!(cmd.timeout, Some(Duration::from_secs(3)));
    }

    /// A shell invocation asks for fork WITHOUT the model setting
    /// `allow_subprocess`.
    ///
    /// Measured on macOS/seatbelt: `(deny process-fork)` does not stop a shell
    /// running a program — bash exec's a single simple command in place, so
    /// `bash -c 'rm -rf x'` runs fine. What it stops is COMPOSITION: `a && b`,
    /// `a | b`, `$(a)`, and even `a > /dev/null` all die with
    /// `bash: fork: Operation not permitted` (exit 128). So the ban bought no
    /// containment and cost the shell nearly everything, while failing
    /// SELECTIVELY — which is how a model learns the false lesson "bash is
    /// blocked" from one compound command. `code_check` already shipped this
    /// same reasoning for its checkers.
    #[tokio::test]
    async fn shell_asks_for_fork_by_default() {
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);

        SESSION_ID
            .scope(sid(), async {
                tool.call(CodeExecArgs {
                    language: Language::Shell,
                    code: "echo a && echo b".to_string(),
                    working_dir: None,
                    timeout_seconds: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: vec![],
                    justification: None,
                })
                .await
                .unwrap()
            })
            .await;

        let calls = mock.calls.lock().await;
        assert_eq!(
            calls[0].capabilities.spawn_subprocess,
            cfg!(not(target_os = "linux")),
            "off Linux a default shell call must be able to fork or every \
             compound command fails with exit 128; on Linux the same flag buys \
             PID isolation instead and must stay off"
        );
    }

    /// The other half, and the one that keeps this change narrow: an
    /// INTERPRETER is already running when it asks to spawn, so denying fork
    /// there is a control that actually controls. Only the shell is exempt.
    #[tokio::test]
    async fn interpreters_do_not_ask_for_fork_by_default() {
        for language in [Language::Python, Language::JavaScript] {
            let label = format!("{language:?}");
            let mock = MockSandbox::new(ok_output(""));
            let sandbox: Arc<dyn Sandbox> = mock.clone();
            let tool = CodeExecTool::new().with_sandbox(sandbox);

            SESSION_ID
                .scope(sid(), async {
                    tool.call(CodeExecArgs {
                        language,
                        code: "1".to_string(),
                        working_dir: None,
                        timeout_seconds: None,
                        allow_network: false,
                        allow_subprocess: false,
                        extra_writable_paths: vec![],
                        justification: None,
                    })
                    .await
                    .unwrap()
                })
                .await;

            let calls = mock.calls.lock().await;
            assert!(
                !calls[0].capabilities.spawn_subprocess,
                "{label} must still route subprocess spawning through approval"
            );
        }
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
        let (program, args, stdin) = expected_shell_spawn("cargo test");
        assert_eq!(cmd.program, program);
        assert_eq!(cmd.args, args, "the wrapper must be peeled off the script");
        assert_eq!(cmd.stdin, stdin);
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
        let (_, args, _) = expected_shell_spawn("bash -c \"echo $(date)\"");
        assert_eq!(cmd.args, args, "unpeelable wrapper must survive verbatim");
    }

    #[tokio::test]
    async fn large_shell_script_pipes_via_stdin() {
        // ~40 KiB script. Under `bash` we switch from `-c <script>` to `-s` +
        // stdin above the threshold, to dodge ARG_MAX on Linux
        // (`MAX_ARG_STRLEN = 128 KiB`). PowerShell has no such branch, so the
        // assertion below is on the shared contract, not on `-s`.
        let big_script = format!("# header\n{}\n", "echo hi\n".repeat(5_000));
        assert!(big_script.len() > STDIN_PIPE_THRESHOLD);

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
        // `canonicalize_shell_cmd` leaves a plain script untouched, so the
        // expected spawn is the contract applied to the script itself.
        let (program, args, stdin) = expected_shell_spawn(&big_script);
        assert_eq!(cmd.program, program);
        assert_eq!(cmd.args, args);
        assert_eq!(
            cmd.stdin, stdin,
            "the whole script must reach the shell, by whichever route"
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
        assert_eq!(
            cmd.program,
            crate::utils::shell::python3().program.to_string_lossy()
        );
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

    /// Aleph context vars (ALEPH_SESSION_ID / ALEPH_TOOL_NAME) are injected
    /// into the child env so a script can self-identify — `bash` actually
    /// gets the literal string `"bash"` (it is the BashExecTool wrapper),
    /// not `"code_exec:shell"`, so scripts that test `[[ $ALEPH_TOOL_NAME ==
    /// bash ]]` work without the model remembering the wrapper detail.
    /// Session id is the JSON form the sandbox already uses for its
    /// workspace key — same value, surfaced for the script.
    async fn env_seen_by_sandbox(
        session: &crate::session::service::SessionId,
        language: Language,
    ) -> (String, String) {
        let mock = MockSandbox::new(ok_output(""));
        let sandbox: Arc<dyn Sandbox> = mock.clone();
        let tool = CodeExecTool::new().with_sandbox(sandbox);
        SESSION_ID
            .scope(session.clone(), async {
                tool.call(CodeExecArgs {
                    language,
                    code: "echo hi".to_string(),
                    working_dir: None,
                    timeout_seconds: Some(3),
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                })
                .await
                .unwrap();
            })
            .await;
        let calls = mock.calls.lock().await;
        let cmd = &calls[0];
        (
            cmd.env.get("ALEPH_SESSION_ID").cloned().unwrap_or_default(),
            cmd.env.get("ALEPH_TOOL_NAME").cloned().unwrap_or_default(),
        )
    }

    #[tokio::test]
    async fn bash_child_env_carries_aleph_session_and_tool_name() {
        let session = sid();
        let expected_id = serde_json::to_string(&session).unwrap();
        SESSION_ID
            .scope(session.clone(), async {
                let (id, name) = env_seen_by_sandbox(&session, Language::Shell).await;
                assert_eq!(id, expected_id, "session id is the JSON sandbox form");
                assert_eq!(name, "bash", "shell wraps as bash, not code_exec:shell");
            })
            .await;
    }

    #[tokio::test]
    async fn non_shell_languages_carry_code_exec_tool_name() {
        // Python / JS callers go straight through `code_exec` — the
        // ALEPH_TOOL_NAME matches the language so a python script can tell
        // itself apart from a bash one.
        let session = sid();
        SESSION_ID
            .scope(session.clone(), async {
                let (_, name_py) = env_seen_by_sandbox(&session, Language::Python).await;
                assert_eq!(name_py, "code_exec:python");
                let (_, name_js) = env_seen_by_sandbox(&session, Language::JavaScript).await;
                assert_eq!(name_js, "code_exec:javascript");
            })
            .await;
    }

    #[test]
    fn resolve_exit_code_passes_through_normal_exit() {
        // A real exit code (zero or non-zero) is reported verbatim with no note.
        assert_eq!(resolve_exit_code(Some(0), None), (0, None));
        assert_eq!(resolve_exit_code(Some(1), None), (1, None));
        // A present code wins even if a signal was also (spuriously) recorded.
        assert_eq!(resolve_exit_code(Some(2), Some(9)), (2, None));
    }

    /// Windows crash codes reach the model as bare negative integers, which
    /// name nothing. Goes red if the table is emptied, if the annotation starts
    /// rewriting the number instead of explaining it, or if an ordinary
    /// non-zero exit starts collecting a note it should not have.
    #[test]
    fn windows_crash_codes_are_named_but_not_renumbered() {
        // 0xC0000005 as i32 — an access violation.
        let (code, note) = resolve_exit_code(Some(-1_073_741_819), None);
        assert_eq!(code, -1_073_741_819, "the OS's number is left alone");
        let note = note.expect("a crash code must carry an explanation");
        assert!(note.contains("access violation"), "{note}");
        assert!(note.contains("0xC0000005"), "names the status: {note}");

        // 0xC0000409 — what a Rust `panic = abort` looks like here, and what
        // rustc itself produced while this round was being verified.
        let (_, note) = resolve_exit_code(Some(-1_073_740_791), None);
        assert!(note.unwrap().contains("__fastfail"));

        // An ordinary failure stays unannotated: a note on every non-zero exit
        // would be noise, and would make the real ones invisible.
        assert_eq!(resolve_exit_code(Some(1), None), (1, None));
        assert_eq!(resolve_exit_code(Some(127), None), (127, None));
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

    fn seatbelt_hint() -> SandboxDenialHint {
        SandboxDenialHint {
            platform: "macos/seatbelt".to_string(),
            signature: "operation not permitted".to_string(),
        }
    }

    #[test]
    fn denial_advisory_names_the_backend_and_every_escalation_param() {
        let note = denial_advisory(Some(&seatbelt_hint()), 1).expect("labelled failure advises");
        assert!(note.contains("macos/seatbelt"), "names the backend: {note}");
        assert!(
            note.contains("operation not permitted"),
            "quotes the matched signature: {note}"
        );
        // All three, because seatbelt reports file / network / fork denials
        // through the same errno — naming one would be a guess.
        for param in [
            "extra_writable_paths",
            "allow_network",
            "allow_subprocess",
            "justification",
        ] {
            assert!(note.contains(param), "advises {param}: {note}");
        }
        // Conservative by construction: an application's own EACCES is
        // byte-identical, so the line may never assert a cause.
        assert!(
            note.contains("may have blocked") && note.contains("looks the same"),
            "advisory must stay a possibility: {note}"
        );
    }

    #[test]
    fn denial_advisory_is_silent_on_success_and_without_a_hint() {
        // Exit 0 with a matching stderr line: the process handled whatever it
        // saw, so there is nothing to report.
        assert!(denial_advisory(Some(&seatbelt_hint()), 0).is_none());
        // Failure with no hint: an ordinary non-zero exit stays unannotated.
        assert!(denial_advisory(None, 1).is_none());
    }

    #[test]
    fn sandbox_output_carries_the_denial_advisory_end_to_end() {
        let out = sandbox_result_to_output(
            Ok(SandboxOutput {
                stderr: b"open: /etc/hosts: Operation not permitted\n".to_vec(),
                exit_code: Some(1),
                denial_hint: Some(seatbelt_hint()),
                ..Default::default()
            }),
            "shell".to_string(),
            60,
        );
        assert!(!out.success);
        let note = out.advisory.expect("a labelled failure reaches the model");
        assert!(note.contains("macos/seatbelt"));
        // The raw stderr is untouched — the advisory is an annotation beside
        // it, never a rewrite of what the command actually printed.
        assert_eq!(out.stderr, "open: /etc/hosts: Operation not permitted\n");
    }

    #[test]
    fn unlabelled_failure_reaches_the_model_with_no_advisory() {
        // Every backend that declares no dialect, and every ordinary failure,
        // must look exactly as it did before this annotation existed.
        let out = sandbox_result_to_output(
            Ok(SandboxOutput {
                stderr: b"error: no such file\n".to_vec(),
                exit_code: Some(2),
                ..Default::default()
            }),
            "shell".to_string(),
            60,
        );
        assert!(out.advisory.is_none());
    }

    #[test]
    fn merge_advisory_keeps_both_notes() {
        assert_eq!(
            merge_advisory(Some("denial".into()), Some("repeat".into())),
            Some("denial\nrepeat".to_string()),
            "neither note may displace the other"
        );
        assert_eq!(
            merge_advisory(Some("denial".into()), None),
            Some("denial".to_string())
        );
        assert_eq!(
            merge_advisory(None, Some("repeat".into())),
            Some("repeat".to_string())
        );
        assert_eq!(merge_advisory(None, None), None);
    }

    /// The collision the merge exists for: a repeated shell command that is
    /// ALSO sandbox-denied must come back carrying both notes. Overwriting —
    /// the pre-fix behaviour — silently dropped the denial label for exactly
    /// the command most likely to be retried.
    #[tokio::test]
    async fn repeat_advisory_does_not_clobber_the_denial_label() {
        let session = crate::routing::session_key::SessionKey::ephemeral("code-exec-denial-merge");
        let denied = SandboxOutput {
            stderr: b"open: /etc/hosts: Operation not permitted\n".to_vec(),
            exit_code: Some(1),
            duration_ms: 3,
            denial_hint: Some(seatbelt_hint()),
            ..Default::default()
        };
        let sandbox: Arc<dyn Sandbox> = MockSandbox::new(denied);
        SESSION_ID
            .scope(session, async {
                let tool = CodeExecTool::new().with_sandbox(sandbox);
                let args = || CodeExecArgs {
                    language: Language::Shell,
                    code: "cat /etc/hosts".to_string(),
                    working_dir: None,
                    timeout_seconds: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    justification: None,
                };

                let first = tool.call(args()).await.unwrap();
                let first_note = first.advisory.expect("denial label on the first run");
                assert!(first_note.contains("macos/seatbelt"));
                assert!(
                    !first_note.contains("already ran this exact command"),
                    "first run is not a repeat: {first_note}"
                );

                let second = tool.call(args()).await.unwrap();
                let second_note = second.advisory.expect("both notes on the repeat");
                assert!(
                    second_note.contains("macos/seatbelt"),
                    "denial label survives the repeat advisory: {second_note}"
                );
                assert!(
                    second_note.contains("already ran this exact command"),
                    "repeat advisory survives the denial label: {second_note}"
                );
            })
            .await;
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
