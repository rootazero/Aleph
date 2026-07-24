//! Bash execution tool - a convenience wrapper around `CodeExecTool`
//!
//! This tool provides a simplified interface for executing bash commands,
//! automatically routing to `CodeExecTool` with language=shell.
//!
//! This exists to maintain compatibility with AI prompts and skills that
//! reference "bash" as a tool name instead of "`code_exec`".
//!
//! Phase 3 Task 8: like `CodeExecTool`, this wrapper now carries the shared
//! `Arc<dyn Sandbox>` transitively — all subprocess execution routes through
//! `WorkspaceSandbox::execute`.

use crate::sync_primitives::Arc;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::code_exec::{CodeExecArgs, CodeExecOutput, CodeExecTool, Language};
use super::process_registry::{
    process_registry, KillOutcome, PollOutcome, RegisterOutcome, WaitOutcome,
};
use crate::error::Result;
use crate::sandbox::context::SESSION_ID;
use crate::sandbox::{current_session, Sandbox};
use crate::tools::AlephTool;

/// Default wall-clock timeout (seconds) applied to a **background** job when
/// the caller does not pass an explicit `timeout`.
///
/// Foreground calls default to 60s (`DEFAULT_CODE_EXEC_TIMEOUT`) and are capped
/// at the 180s tool budget. Background calls escape that budget wrapper (they
/// return a `process_id` immediately), so inheriting the 60s foreground default
/// would SIGKILL a backgrounded `cargo build` / `pip install` long before it
/// finishes — defeating the entire point of background mode. Give unspecified
/// background jobs a generous one-hour ceiling instead; an explicit `timeout`
/// still wins, and the job stays killable via `process_action: "kill"`.
const BACKGROUND_DEFAULT_TIMEOUT_SECS: u64 = 3600;

/// Default wait window (seconds) for `process_action: "wait"` when the caller
/// passes no `timeout`. A `wait` blocks the *foreground* tool call, so it lives
/// under the same 180s tool budget — we keep the default modest and cap it
/// below the ceiling so the wait returns a clean `running`/`done` verdict
/// rather than being SIGKILLed by the budget wrapper mid-wait.
const WAIT_DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Hard ceiling for a `wait` window. Stays under the 180s foreground tool
/// budget so an over-eager `timeout` can't push the blocking wait past the
/// point where the budget wrapper kills the whole call.
const WAIT_MAX_TIMEOUT_SECS: u64 = 170;

/// Arguments for bash execution tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BashExecArgs {
    /// The bash command to execute. Optional only when `process_action` is set
    /// (poll/kill/list don't run a command).
    #[serde(default)]
    pub cmd: String,
    /// Working directory (optional, defaults to session workspace root)
    #[serde(default)]
    pub working_dir: Option<String>,
    /// Timeout in seconds (optional, defaults to 60). Accepts the legacy
    /// `timeout` spelling.
    #[serde(default, alias = "timeout")]
    pub timeout_seconds: Option<u64>,
    /// Request elevated network access for this call (sandbox approval-gated).
    #[serde(default)]
    pub allow_network: bool,
    /// Request permission to fork subprocesses (sandbox approval-gated).
    #[serde(default)]
    pub allow_subprocess: bool,
    /// Extra writable paths beyond the session workspace (sandbox approval-gated).
    #[serde(default)]
    pub extra_writable_paths: Vec<PathBuf>,
    /// Run `cmd` in the background and return a `process_id` immediately instead
    /// of blocking until it finishes. Poll/kill it later with `process_action`.
    #[serde(default)]
    pub background: bool,
    /// Manage a background process instead of running a command:
    /// `"poll"` (fetch status/output once), `"wait"` (block until it finishes,
    /// up to `timeout`), `"kill"` (terminate), or `"list"` (enumerate this
    /// session's background processes). When set, `cmd` is ignored;
    /// `poll`/`wait`/`kill` require `process_id`.
    #[serde(default)]
    pub process_action: Option<String>,
    /// Target background process id for `process_action` = `poll` | `wait` | `kill`.
    #[serde(default)]
    pub process_id: Option<u64>,
    /// Optional natural-language reason for *why* an escalation
    /// (`allow_network` / `allow_subprocess` / `extra_writable_paths`) is
    /// needed. Forwarded to the human approver alongside the requested
    /// capabilities. Ignored for non-escalating calls. codex `justification`
    /// parity.
    #[serde(default)]
    pub justification: Option<String>,
}

/// Bash execution tool - wraps `CodeExecTool` for bash/shell commands
#[derive(Clone)]
pub struct BashExecTool {
    inner: CodeExecTool,
}

impl BashExecTool {
    /// Create a new bash execution tool without a sandbox wired in yet.
    /// Boot wiring attaches the shared `Arc<dyn Sandbox>` via
    /// [`BashExecTool::with_sandbox`]; unconfigured instances refuse execution
    /// with a structured error (delegated to `CodeExecTool`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: CodeExecTool::new(),
        }
    }

    /// Attach a shared `Arc<dyn Sandbox>`. Delegates to the inner
    /// `CodeExecTool` — both wrappers share the same sandbox instance.
    pub fn with_sandbox(mut self, sandbox: Arc<dyn Sandbox>) -> Self {
        self.inner = self.inner.with_sandbox(sandbox);
        self
    }
}

impl Default for BashExecTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of `AlephTool` trait for `BashExecTool`
#[async_trait]
impl AlephTool for BashExecTool {
    const NAME: &'static str = "bash";
    const DESCRIPTION: &'static str = r#"Execute bash/shell commands in a per-session sandboxed workspace.

Multi-line scripts are first-class — newlines, heredocs (`cat <<'EOF' ... EOF`),
loops, and pipelines all work. If you need 10 commands in a row, write them as
ONE multi-line `cmd` instead of 10 calls; that runs in a single bash process
with shared variables and exit-on-error semantics. Scripts larger than 32 KB
are automatically piped via `bash -s` + stdin so you never hit ARG_MAX.

Sessions are stateless: each call spawns a fresh bash process. `cd`, exported
variables, `set -e`, `source`, etc. do NOT carry over to the next call. If you
need cross-call state, write the state into a file under `working_dir` and
read it back in the next script, or just put everything into one cmd.

Don't re-run a command you already ran this session unless you expect its
output to have changed — the previous result is still above in the conversation.
If you're polling for something to change (a build finishing, a file appearing),
use BACKGROUND MODE's `wait`/`poll` below instead of re-issuing the same
command; an identical re-run comes back with an `advisory` field flagging it.
And prefer the purpose-built tools over shelling out: `file_read` to read a
file, `file_edit` to change one, `search` to find files or text — they beat
`cat`/`sed`/`grep`/`find` and don't spend a shell turn.

`working_dir` (optional) is resolved inside the session workspace; paths
outside the workspace are denied by the sandbox. If omitted the call lands at
the workspace root.

`timeout` defaults to 60s. Foreground calls are clamped to ~170s (just under
the 180s tool budget) so an over-long `timeout` still returns a clean
`exit_code = 124` with partial output instead of a hard "no result" abort —
for longer runs use BACKGROUND MODE below.
On timeout we kill the process, drain stdout/stderr for up to 2s, and return
`exit_code = 124` (POSIX `timeout(1)` convention) with whatever the script
printed before the kill preserved in `stdout` and `stderr` — so even a
runaway script tells you what it accomplished.

ANSI colour codes and stray binary control bytes are stripped from the
returned `stdout`/`stderr` automatically — no need for `--color=never`
or piping through `cat`.

When a stream overflows its cap we keep BOTH the head and the tail (with a
`…[N bytes elided]…` marker between them), so a long build that fails at the
end still shows you the final error, not just the opening. If the command is
killed by a signal it surfaces as `exit_code = 128 + N` with a `stderr` note
naming the signal — e.g. `137` (SIGKILL, usually an out-of-memory kill),
`139` (SIGSEGV, a crash), `134` (SIGABRT, an assertion/panic abort).

Capability escalations (`allow_network`, `allow_subprocess`, `extra_writable_paths`)
trigger an approval prompt the first time per session; subsequent same-or-
narrower requests reuse the grant. When you escalate, pass `justification` with
a one-line reason WHY (e.g. "clone the repo over https") — it is shown to the
human approver so they can decide.

BACKGROUND MODE — for commands that outlive the 180s ceiling (builds, installs,
long test runs). Set `background: true` and the call returns a `process_id`
immediately instead of blocking. Background jobs escape the 180s foreground
ceiling: with no explicit `timeout` they get a generous 1-hour default (pass
`timeout` to raise or lower it), and you can stop one anytime with
`process_action: "kill"`. Manage it with `process_action`:
- `{"process_action": "poll", "process_id": N}` → status while running, or the
  full {exit_code, stdout, stderr} once finished (output is captured, not
  streamed mid-run — poll again until done).
- `{"process_action": "wait", "process_id": N}` → block until it finishes and
  return its full output, or a `running` status if it is still going after the
  wait window (default 60s, set `timeout` to extend up to 170s). Prefer `wait`
  over a tight `poll` loop — it costs no round-trips while the job runs.
- `{"process_action": "kill", "process_id": N}` → terminate it (SIGKILL).
- `{"process_action": "list"}` → enumerate this session's background processes.
Background processes are scoped to your session; you cannot see or kill another
session's processes, and each session may have at most 8 running at once — if
you hit that cap, poll/kill an existing one before starting another. Prefer
foreground (blocking) execution for anything that finishes quickly —
backgrounding is only worth it past ~the timeout ceiling.

Examples:
- One-liner: {"cmd": "ls -la /tmp"}
- Multi-line with set -e: {"cmd": "set -e\ncd src\ncargo check\necho ok"}
- Heredoc: {"cmd": "cat <<'EOF' > /tmp/note\nhello world\nEOF\nwc -l /tmp/note"}
- Large script: {"cmd": "<paste a 50 KB build script — auto-piped via stdin>"}
- Custom timeout: {"cmd": "find . -name '*.rs' | wc -l", "timeout": 30}
- Background a build: {"cmd": "cargo build --release", "background": true}
- Check on it later: {"process_action": "poll", "process_id": 1}"#;

    type Args = BashExecArgs;
    type Output = super::code_exec::CodeExecOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "bash(cmd='ls -la /tmp')".to_string(),
            "bash(cmd='echo \"Hello World\" > /tmp/test.txt')".to_string(),
            "bash(cmd='pwd && ls -l', working_dir='/home/user')".to_string(),
            "bash(cmd='find . -name \"*.rs\" | wc -l', timeout=30)".to_string(),
        ])
    }

    /// Build/test/log output can run long; cap at 8k tokens (was the legacy
    /// `resolve_result_budget` name-table value for `bash`).
    fn max_result_tokens(&self) -> Option<usize> {
        Some(8_000)
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Background-process management never runs a command — handle first.
        if let Some(action) = args.process_action.as_deref() {
            return Ok(handle_process_action(action, args.process_id, args.timeout_seconds).await);
        }

        if args.cmd.is_empty() {
            return Ok(error_output(
                "bash: `cmd` is required (or set `process_action` to manage a background process)",
            ));
        }

        let mut code_exec_args = CodeExecArgs {
            language: Language::Shell,
            code: args.cmd,
            working_dir: args.working_dir,
            timeout_seconds: args.timeout_seconds,
            allow_network: args.allow_network,
            allow_subprocess: args.allow_subprocess,
            extra_writable_paths: args.extra_writable_paths,
            justification: args.justification,
        };

        if args.background {
            // Background escapes the 180s foreground budget wrapper, so the
            // foreground 60s default would prematurely SIGKILL long builds.
            // Substitute a generous default only when the caller left `timeout`
            // unset; an explicit value (longer or shorter) is always honoured.
            if code_exec_args.timeout_seconds.is_none() {
                code_exec_args.timeout_seconds = Some(BACKGROUND_DEFAULT_TIMEOUT_SECS);
            }
            return Ok(self.spawn_background(code_exec_args));
        }

        // Foreground (default) — delegate to CodeExecTool, blocking.
        self.inner.call(code_exec_args).await
    }
}

impl BashExecTool {
    /// Drive `code_exec_args` inside a detached task and return a `process_id`
    /// immediately. The task re-enters the current `SESSION_ID` scope (task
    /// locals don't propagate into `tokio::spawn`) so the sandbox still targets
    /// the right per-session workspace, and re-enters the ambient
    /// `CallIdentity` so a sandbox-elevation approval raised by the detached
    /// command still stamps the bash call that spawned it (otherwise the card
    /// reverts to the uncorrelated pre-identity state for exactly this path).
    fn spawn_background(&self, code_exec_args: CodeExecArgs) -> CodeExecOutput {
        let registry = process_registry();
        let caller = session_label();
        let sid = current_session();
        let identity = crate::approval::current_call_identity();
        let inner = self.inner.clone();
        let preview = code_exec_args.code.clone();

        // The task must not record completion before it has been registered
        // (a fast command could otherwise finish before `register_running`
        // inserts its slot, dropping the output). Gate the task on a oneshot
        // carrying its id; the foreground sends it only after registration.
        let (id_tx, id_rx) = tokio::sync::oneshot::channel::<u64>();
        let reg = registry.clone();
        let join = tokio::spawn(async move {
            let id = match id_rx.await {
                Ok(id) => id,
                // Foreground dropped the sender (registration failed) — nothing
                // to report against; abandon quietly.
                Err(_) => return,
            };
            // Background escapes the 180s per-tool budget wrapper (the spawn
            // call returned a process_id already), so it must NOT inherit the
            // foreground timeout clamp — a backgrounded `cargo build` may
            // legitimately run for the full 1h ceiling. `call_unclamped` runs
            // `execute` directly, bypassing the clamp in `AlephTool::call`.
            let result = crate::approval::with_call_identity(identity, async move {
                match sid {
                    Some(sid) => SESSION_ID.scope(sid, inner.call_unclamped(code_exec_args)).await,
                    None => inner.call_unclamped(code_exec_args).await,
                }
            })
            .await;
            let output = result
                .unwrap_or_else(|e| error_output(format!("bash: background task error: {e}")));
            reg.complete(id, output);
        });

        match registry.register_running(preview, caller, join.abort_handle()) {
            RegisterOutcome::Registered(id) => {
                let _ = id_tx.send(id);
                info_output(serde_json::json!({
                    "process_id": id,
                    "status": "running",
                    "background": true,
                    "message": format!(
                        "Started background process {id}. Poll with {{\"process_action\":\"poll\",\"process_id\":{id}}}."
                    ),
                }))
            }
            RegisterOutcome::TooManyRunning { limit } => {
                // Refuse without a slot. Dropping `id_tx` makes the gated task
                // exit on its `id_rx.await` error without ever touching the
                // sandbox; abort it too so the detached task is reaped promptly
                // rather than lingering until the channel drop is observed.
                drop(id_tx);
                join.abort();
                error_output(format!(
                    "bash: this session already has {limit} background processes running (the per-session cap). \
                     Poll or kill an existing one before starting another — \
                     {{\"process_action\":\"list\"}} to see them, then \
                     {{\"process_action\":\"kill\",\"process_id\":N}} to free a slot."
                ))
            }
        }
    }
}

/// Session label used to scope the process registry. Mirrors the JSON form the
/// sandbox uses for its workspace key so the value is stable for a session.
fn session_label() -> Option<String> {
    current_session().map(|sid| serde_json::to_string(&sid).unwrap_or_else(|_| format!("{sid:?}")))
}

/// Dispatch a `poll` / `wait` / `kill` / `list` management action against the
/// registry, scoped to the caller's session. `wait` is the only async branch
/// (it parks on the registry's completion notifier); the rest are synchronous
/// table reads.
async fn handle_process_action(
    action: &str,
    process_id: Option<u64>,
    timeout: Option<u64>,
) -> CodeExecOutput {
    let registry = process_registry();
    let caller = session_label();
    match action {
        "list" => {
            let rows = registry.list(caller.as_deref());
            info_output(serde_json::json!({ "processes": rows }))
        }
        "poll" => {
            let Some(id) = process_id else {
                return error_output("bash: process_action=poll requires `process_id`");
            };
            match registry.poll(id, caller.as_deref()) {
                // Surface the captured tool output verbatim once finished.
                PollOutcome::Done(out) => *out,
                PollOutcome::Running { elapsed_ms } => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "running",
                    "elapsed_ms": elapsed_ms,
                })),
                PollOutcome::Killed => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "killed",
                })),
                PollOutcome::NotFound => error_output(format!(
                    "bash: no background process #{id} for this session"
                )),
            }
        }
        "wait" => {
            let Some(id) = process_id else {
                return error_output("bash: process_action=wait requires `process_id`");
            };
            // Clamp the wait window under the foreground tool budget so the
            // blocking wait always returns a verdict instead of being killed.
            let secs = timeout
                .unwrap_or(WAIT_DEFAULT_TIMEOUT_SECS)
                .clamp(1, WAIT_MAX_TIMEOUT_SECS);
            match registry
                .wait(id, caller.as_deref(), Duration::from_secs(secs))
                .await
            {
                // Finished within the window — surface the captured output.
                WaitOutcome::Done(out) => *out,
                WaitOutcome::Killed => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "killed",
                })),
                WaitOutcome::TimedOut { elapsed_ms } => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "running",
                    "elapsed_ms": elapsed_ms,
                    "message": format!(
                        "Still running after waiting {secs}s. Wait again or poll later with \
                         {{\"process_action\":\"poll\",\"process_id\":{id}}}."
                    ),
                })),
                WaitOutcome::NotFound => error_output(format!(
                    "bash: no background process #{id} for this session"
                )),
            }
        }
        "kill" => {
            let Some(id) = process_id else {
                return error_output("bash: process_action=kill requires `process_id`");
            };
            match registry.kill(id, caller.as_deref()) {
                KillOutcome::Killed => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "killed",
                })),
                KillOutcome::AlreadyFinished => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "already_finished",
                })),
                KillOutcome::NotFound => error_output(format!(
                    "bash: no background process #{id} for this session"
                )),
            }
        }
        other => error_output(format!(
            "bash: unknown process_action '{other}' (expected poll|wait|kill|list)"
        )),
    }
}

/// Build a successful informational `CodeExecOutput` whose `stdout` carries a
/// JSON payload. Keeps the tool's Output type stable — background/management
/// responses ride inside the existing envelope rather than a new struct.
fn info_output(payload: serde_json::Value) -> CodeExecOutput {
    CodeExecOutput {
        success: true,
        exit_code: 0,
        stdout: payload.to_string(),
        stderr: String::new(),
        duration_ms: 0,
        language: "shell".to_string(),
        truncated: None,
        stdout_truncated_bytes: 0,
        stderr_truncated_bytes: 0,
        advisory: None,
    }
}

/// Build a failed `CodeExecOutput` carrying a human-readable error in `stderr`.
fn error_output(message: impl Into<String>) -> CodeExecOutput {
    CodeExecOutput {
        success: false,
        exit_code: -1,
        stdout: String::new(),
        stderr: message.into(),
        duration_ms: 0,
        language: "shell".to_string(),
        truncated: None,
        stdout_truncated_bytes: 0,
        stderr_truncated_bytes: 0,
        advisory: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TDD RED: `timeout_seconds` is the canonical spelling; the legacy bare
    /// `timeout` must still parse via `#[serde(alias = "timeout")]` so saved
    /// calls / prompts don't break (deserialize-only, no schema change).
    #[test]
    fn bash_exec_timeout_accepts_canonical_and_legacy_alias() {
        let a: BashExecArgs =
            serde_json::from_value(serde_json::json!({ "cmd": "true", "timeout_seconds": 30 }))
                .unwrap();
        assert_eq!(a.timeout_seconds, Some(30));
        let b: BashExecArgs =
            serde_json::from_value(serde_json::json!({ "cmd": "true", "timeout": 30 })).unwrap();
        assert_eq!(b.timeout_seconds, Some(30));
    }

    /// R9 (Intelligence Lives in the Prompt): the bash description is
    /// how the model learns the surface area. Lock the load-bearing
    /// teaching points so future edits can't accidentally drop them.
    #[test]
    fn description_teaches_stateless_sessions_and_partial_output() {
        let d = <BashExecTool as AlephTool>::DESCRIPTION;
        // Stateless reality
        assert!(
            d.contains("stateless"),
            "should warn about stateless sessions"
        );
        assert!(
            d.contains("do NOT carry over"),
            "should call out lost state"
        );
        // Multi-line + heredoc encouragement
        assert!(d.contains("Multi-line scripts"), "should bless multi-line");
        assert!(d.contains("heredoc"), "should mention heredoc pattern");
        // 32KB stdin auto-pipe
        assert!(
            d.contains("32 KB") || d.contains("32KB"),
            "should mention the stdin-pipe threshold"
        );
        assert!(d.contains("ARG_MAX"), "should explain why the pipe exists");
        // Timeout + POSIX exit-code-124 contract
        assert!(
            d.contains("60s") || d.contains("60 seconds"),
            "default timeout"
        );
        assert!(d.contains("180s"), "ceiling");
        assert!(d.contains("124"), "POSIX timeout exit code");
        // Partial-output guarantee
        assert!(
            d.contains("preserved"),
            "should promise partial output on kill"
        );
        assert!(
            d.contains("justification"),
            "should teach passing a justification when escalating"
        );
    }

    #[test]
    fn description_teaches_background_mode() {
        let d = <BashExecTool as AlephTool>::DESCRIPTION;
        assert!(
            d.contains("BACKGROUND MODE"),
            "should document backgrounding"
        );
        assert!(d.contains("process_id"), "should mention the handle");
        assert!(
            d.contains("process_action"),
            "should mention management verbs"
        );
        assert!(
            d.contains("\"poll\"")
                && d.contains("\"wait\"")
                && d.contains("\"kill\"")
                && d.contains("\"list\""),
            "should enumerate poll/wait/kill/list"
        );
        assert!(
            d.contains("1-hour"),
            "should teach the generous background timeout default"
        );
        assert!(
            d.contains("at most 8 running"),
            "should teach the per-session running cap"
        );
    }

    /// C (description hardening): the model should learn not to waste turns
    /// re-running identical commands, and to reach for the purpose-built tools.
    #[test]
    fn description_discourages_wasteful_repeats() {
        let d = <BashExecTool as AlephTool>::DESCRIPTION;
        assert!(
            d.contains("advisory"),
            "should teach the repeat-advisory field"
        );
        assert!(
            d.contains("already ran"),
            "should discourage re-running the same command"
        );
        assert!(
            d.contains("file_read") && d.contains("search"),
            "should redirect to the purpose-built read/search tools"
        );
    }

    fn bash(args: BashExecArgs) -> impl std::future::Future<Output = CodeExecOutput> {
        let tool = BashExecTool::new();
        async move { tool.call(args).await.expect("structured output, not Err") }
    }

    fn args_action(action: &str, id: Option<u64>) -> BashExecArgs {
        BashExecArgs {
            cmd: String::new(),
            working_dir: None,
            timeout_seconds: None,
            allow_network: false,
            allow_subprocess: false,
            extra_writable_paths: Vec::new(),
            background: false,
            process_action: Some(action.to_string()),
            process_id: id,
            justification: None,
        }
    }

    #[tokio::test]
    async fn poll_without_id_is_a_clear_error() {
        let out = bash(args_action("poll", None)).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("requires `process_id`"),
            "{}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn kill_without_id_is_a_clear_error() {
        let out = bash(args_action("kill", None)).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("requires `process_id`"),
            "{}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn unknown_action_is_rejected() {
        let out = bash(args_action("frobnicate", None)).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("unknown process_action"),
            "{}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn poll_unknown_id_reports_not_found() {
        let out = bash(args_action("poll", Some(u64::MAX))).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("no background process"),
            "{}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn wait_without_id_is_a_clear_error() {
        let out = bash(args_action("wait", None)).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("requires `process_id`"),
            "{}",
            out.stderr
        );
    }

    #[tokio::test]
    async fn wait_unknown_id_reports_not_found() {
        // A bounded wait on an unknown id resolves immediately to NotFound
        // (no busy spin) — proves the wait branch is wired through `call`.
        let out = bash(args_action("wait", Some(u64::MAX))).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("no background process"),
            "{}",
            out.stderr
        );
    }

    /// A (repeat advisory): a byte-identical foreground shell command run
    /// twice in the same session is unflagged the first time and carries a
    /// repeat `advisory` the second — and it still executes both times
    /// (advisory only, never a gate). Unique ephemeral session keeps the
    /// process-global ledger isolated from other tests.
    #[tokio::test]
    async fn repeated_foreground_command_surfaces_advisory() {
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-repeat-advisory");
        SESSION_ID
            .scope(session, async {
                let tool = BashExecTool::new();
                let mk = || BashExecArgs {
                    cmd: "echo hi".to_string(),
                    working_dir: None,
                    timeout_seconds: None,
                    allow_network: false,
                    allow_subprocess: false,
                    extra_writable_paths: Vec::new(),
                    background: false,
                    process_action: None,
                    process_id: None,
                    justification: None,
                };
                let first = tool.call(mk()).await.unwrap();
                assert!(first.advisory.is_none(), "first run is not a repeat");
                let second = tool.call(mk()).await.unwrap();
                let note = second.advisory.expect("second identical run is flagged");
                assert!(
                    note.contains("already ran this exact command"),
                    "advisory text: {note}"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn empty_cmd_without_action_errors() {
        let out = bash(BashExecArgs {
            cmd: String::new(),
            working_dir: None,
            timeout_seconds: None,
            allow_network: false,
            allow_subprocess: false,
            extra_writable_paths: Vec::new(),
            background: false,
            process_action: None,
            process_id: None,
            justification: None,
        })
        .await;
        assert!(!out.success);
        assert!(out.stderr.contains("`cmd` is required"), "{}", out.stderr);
    }

    /// End-to-end background round-trip with no sandbox wired: the spawned task
    /// completes with `CodeExecTool`'s structured "sandbox not configured"
    /// error, which `poll` then surfaces verbatim. Scoped under a unique
    /// session so the process-global registry stays isolated from other tests.
    #[tokio::test]
    async fn background_spawn_then_poll_round_trips_output() {
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-bg-e2e");
        let out = SESSION_ID
            .scope(session.clone(), async {
                let tool = BashExecTool::new();
                // Spawn.
                let spawn = tool
                    .call(BashExecArgs {
                        cmd: "echo hi".to_string(),
                        working_dir: None,
                        timeout_seconds: None,
                        allow_network: false,
                        allow_subprocess: false,
                        extra_writable_paths: Vec::new(),
                        background: true,
                        process_action: None,
                        process_id: None,
                        justification: None,
                    })
                    .await
                    .unwrap();
                assert!(spawn.success);
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");

                // Poll until it finishes (the no-sandbox path returns instantly).
                for _ in 0..200 {
                    let polled = tool
                        .call(BashExecArgs {
                            cmd: String::new(),
                            working_dir: None,
                            timeout_seconds: None,
                            allow_network: false,
                            allow_subprocess: false,
                            extra_writable_paths: Vec::new(),
                            background: false,
                            process_action: Some("poll".to_string()),
                            process_id: Some(id),
                            justification: None,
                        })
                        .await
                        .unwrap();
                    // While running the poll envelope reports status=running in
                    // stdout; once done, the captured tool output surfaces (here
                    // a structured error because no sandbox is wired).
                    if !polled.stdout.contains("\"status\":\"running\"") {
                        return polled;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                panic!("background process never completed");
            })
            .await;

        assert!(
            out.stderr.contains("sandbox not configured"),
            "poll should surface the captured task output verbatim: {}",
            out.stderr
        );
    }

    /// Drive one background job (with the given explicit `timeout`) to
    /// completion through a `MockSandbox` and report the wall-clock timeout
    /// the inner `CodeExecTool` actually handed to the sandbox.
    async fn background_sandbox_timeout(explicit: Option<u64>) -> Option<std::time::Duration> {
        use crate::sandbox::test_util::MockSandbox;
        use crate::sandbox::{Sandbox, SandboxOutput};

        let mock = MockSandbox::new(SandboxOutput {
            stdout: b"ok\n".to_vec(),
            exit_code: Some(0),
            duration_ms: 1,
            ..Default::default()
        });
        let sandbox: crate::sync_primitives::Arc<dyn Sandbox> = mock.clone();
        let tool = BashExecTool::new().with_sandbox(sandbox);
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-bg-timeout");

        SESSION_ID
            .scope(session, async {
                let spawn = tool
                    .call(BashExecArgs {
                        cmd: "echo ok".to_string(),
                        working_dir: None,
                        timeout_seconds: explicit,
                        allow_network: false,
                        allow_subprocess: false,
                        extra_writable_paths: Vec::new(),
                        background: true,
                        process_action: None,
                        process_id: None,
                        justification: None,
                    })
                    .await
                    .unwrap();
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");
                // Poll until the detached task has invoked the sandbox + finished.
                for _ in 0..200 {
                    let polled = tool.call(args_action("poll", Some(id))).await.unwrap();
                    if !polled.stdout.contains("\"status\":\"running\"") {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await;

        let calls = mock.calls.lock().await;
        calls.first().and_then(|c| c.timeout)
    }

    #[tokio::test]
    async fn background_without_timeout_gets_generous_default() {
        // Regression: a backgrounded build with no explicit timeout must NOT
        // inherit the 60s foreground default (which would SIGKILL it early).
        let t = background_sandbox_timeout(None).await;
        assert_eq!(
            t,
            Some(std::time::Duration::from_secs(
                BACKGROUND_DEFAULT_TIMEOUT_SECS
            )),
            "background default should be the generous ceiling, not 60s"
        );
    }

    #[tokio::test]
    async fn background_honours_explicit_timeout() {
        // An explicit `timeout` always wins over the background default.
        let t = background_sandbox_timeout(Some(30)).await;
        assert_eq!(t, Some(std::time::Duration::from_secs(30)));
    }
}
