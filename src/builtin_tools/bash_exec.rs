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
use super::partial_output::{self, PartialView};
use super::process_completion;
use super::process_journal::{self, JobPhase, RecoveredJob, Verdict};
use super::process_registry::{
    process_registry, KillOutcome, PollOutcome, RegisterOutcome, WaitOutcome,
};
use crate::error::Result;
use crate::sandbox::context::{LIVE_TAIL, SESSION_ID};
use crate::sandbox::live_tail::{LiveSnapshot, LiveTail};
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
    /// No-op: a shell already forks. (Real escalation on code_exec py/js.)
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

A command you already ran this session comes back with an `advisory` field
flagging the repeat; a re-run returns the same result, so prefer BACKGROUND
MODE's `wait`/`poll` for things that change over time.

SEARCH AND READ DO NOT BELONG HERE. `grep` and `find` beat `grep -r` / `rg` /
`find` / `ls -R`: they obey .gitignore, skip `.git` and binaries, cap and page
their output, and take several terms as ONE call via regex alternation
(`grep{pattern: "foo|bar|baz"}`). A shell search does none of that — one
recursive run pours every hit under node_modules/, target/ and dist/ straight
into the context window. Likewise `file_read` beats `cat`/`sed -n`/`head`,
including for a file outside the workspace whose path you already know, and
`file_edit` beats a `sed -i`. If a search genuinely has to run in the shell,
use `rg` rather than `grep` — it honours ignore files and skips binaries, so
its output is roughly an order of magnitude smaller — and bound it with a
`| head -n` or a `-m` cap.

`working_dir` (optional) resolves inside the session workspace; paths
outside the workspace are denied by the sandbox. If omitted the call lands
at the workspace root.

`timeout` defaults to 60s. Foreground calls are clamped to ~170s (just under
the 180s tool budget) so an over-long `timeout` returns a clean
`exit_code = 124` (POSIX `timeout(1)` convention) with partial output
preserved in `stdout` / `stderr` — even a runaway script tells you what it
accomplished. For longer runs use BACKGROUND MODE below.

ANSI colour codes and stray binary control bytes are stripped automatically
(no need for `--color=never` or `cat`); when a stream overflows its cap we
keep both the head and the tail with a `…[N bytes elided]…` marker between
them, and the response also carries `stdout_truncated_bytes` /
`stderr_truncated_bytes` so you know exactly how much was elided. Signal
deaths surface as `exit_code = 128 + N` with a `stderr` note naming the
signal — `137` (SIGKILL, usually OOM), `139` (SIGSEGV, a crash), `134`
(SIGABRT, an assertion/panic abort).

Capability escalations (`allow_network`, `extra_writable_paths`) trigger an
approval prompt the first time per session; subsequent same-or-narrower
requests reuse the grant. Forking is not one. When you
escalate, pass `justification` with a one-line reason WHY (e.g. "clone the
repo over https") — it is shown to the human approver so they can decide.

BACKGROUND MODE — for commands that outlive the 180s ceiling (builds,
installs, long test runs). Set `background: true` and the call returns a
`process_id` immediately. Background jobs escape the 180s foreground
ceiling: with no explicit `timeout` they get a generous 1-hour default
(pass `timeout` to raise or lower it), and you can stop one anytime with
`process_action: "kill"`. Manage it with `process_action`:
- `{"process_action": "poll", "process_id": N}` → status + an 8 KB
  `partial_stdout`/`partial_stderr` tail while running, or the full
  {exit_code, stdout, stderr} once finished.
- `{"process_action": "wait", "process_id": N}` → block until it finishes
  and return its full output, or the same `running` status if it is still going
  after the wait window (default 60s, set `timeout` to extend up to 170s).
  Prefer `wait` over a tight `poll` loop — it costs no round-trips while
  the job runs.
- `{"process_action": "kill", "process_id": N}` → terminate it (SIGKILL).
- `{"process_action": "list"}` → enumerate this session's background processes.
Background processes are scoped to your session; you cannot see or kill
another session's processes, and each session may have at most 8 running at
once — if you hit that cap, poll/kill an existing one before starting
another. Prefer foreground (blocking) execution for anything that finishes
quickly — backgrounding is only worth it past ~the timeout ceiling.

The child shell sees `ALEPH_SESSION_ID` (the per-session workspace key) and
`ALEPH_TOOL_NAME=bash` so scripts can self-identify (e.g. `[[ -n
"$ALEPH_SESSION_ID" ]] && ...`).
"#;

    type Args = BashExecArgs;
    type Output = super::code_exec::CodeExecOutput;

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
            // Defence in depth (audit-2026-08-26 BTT-3): background escapes
            // the budget wrapper entirely, so a runaway LLM passing
            // `u64::MAX` would register a job the registry never reaps
            // (kill requires the same session to come back and call
            // `process_action: "kill"`). Cap the worst case at 4 hours so
            // the daemon cannot be wedged indefinitely on a single bad
            // call; the 1h default still wins when the caller leaves the
            // field unset.
            const BACKGROUND_MAX_TIMEOUT_SECS: u64 = 4 * 3600;
            if let Some(t) = code_exec_args.timeout_seconds {
                if t > BACKGROUND_MAX_TIMEOUT_SECS {
                    tracing::warn!(
                        original = t,
                        capped_to = BACKGROUND_MAX_TIMEOUT_SECS,
                        "bash_exec: background timeout above ceiling, clamping"
                    );
                    code_exec_args.timeout_seconds = Some(BACKGROUND_MAX_TIMEOUT_SECS);
                }
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
    /// reverts to the uncorrelated pre-identity state for exactly this path),
    /// and enters a fresh `LIVE_TAIL` scope so the platform driver's drain
    /// loops tee a rolling tail that `poll` / `wait` can show mid-run.
    fn spawn_background(&self, code_exec_args: CodeExecArgs) -> CodeExecOutput {
        let registry = process_registry();
        let caller = session_label();
        let sid = current_session();
        // Captured on the CALLER's task: the authorised jail root is a
        // task-local like the rest, so reading it inside the detached task
        // would find nothing and the job would silently run in a different
        // directory than the foreground call that spawned it.
        let exec_workspace = crate::sandbox::context::current_exec_workspace();
        let identity = crate::approval::current_call_identity();
        let inner = self.inner.clone();
        let preview = code_exec_args.code.clone();
        // A second copy for the completion announce: `preview` moves into
        // `register_running` on this task, and the notice is built inside the
        // detached one.
        let command_for_announce = preview.clone();
        // One tail per background job: the detached task scopes it (so the
        // drivers can tee into it) and the registry holds it (so `poll` can
        // read it) until the entry retires.
        let live = Arc::new(LiveTail::new());
        let live_for_task = live.clone();

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
            // Cloned before `sid` moves into the exec scope below: the announce
            // must be addressed with the captured `SessionId` itself. The
            // registry's owner label is NOT a substitute — it is a serialized
            // `SessionId`, which `SessionKey::from_key_string` does not read.
            let announce_sid = sid.clone();
            // Background escapes the 180s per-tool budget wrapper (the spawn
            // call returned a process_id already), so it must NOT inherit the
            // foreground timeout clamp — a backgrounded `cargo build` may
            // legitimately run for the full 1h ceiling. `call_unclamped` runs
            // `execute` directly, bypassing the clamp in `AlephTool::call`.
            let result = crate::approval::with_call_identity(identity, async move {
                // Task-locals do NOT cross `tokio::spawn`, so the session
                // scope, the live-tail scope and the authorised exec workspace
                // all have to be re-entered HERE, in the spawned task —
                // entering them around `tokio::spawn` on the caller's task
                // would leave the driver seeing none of them. The guard that
                // catches a forgotten member is behavioural, not a name list:
                // `background_lands_in_the_same_directory_as_foreground`.
                LIVE_TAIL
                    .scope(live_for_task, async move {
                        crate::sandbox::context::with_exec_workspace(exec_workspace, async move {
                            match sid {
                                Some(sid) => {
                                    SESSION_ID
                                        .scope(sid, inner.call_unclamped(code_exec_args))
                                        .await
                                }
                                None => inner.call_unclamped(code_exec_args).await,
                            }
                        })
                        .await
                    })
                    .await
            })
            .await;
            let output = result
                .unwrap_or_else(|e| error_output(format!("bash: background task error: {e}")));
            // Built before `output` moves into the registry, broadcast after —
            // an announce-driven parent turn must find the job already `Done`
            // when it polls, the same ordering `subagent_tool::spawn` keeps
            // around `mark_completed`.
            let announcement = announce_sid.as_ref().map(|_| {
                process_completion::completion_event(
                    id,
                    &command_for_announce,
                    output.exit_code,
                    &output.stdout,
                    &output.stderr,
                )
            });
            // R5 proactive arrival: without this, a job that outlives the run
            // that started it is only ever seen if the model happens to poll —
            // and once that run ends, nobody looks. Guarded on the *effect*,
            // not the call: a completion landing after a `kill` changes nothing
            // in the registry, and announcing it would tell the session a job
            // finished that it had already stopped. No session (CLI / direct
            // callers) means nobody to announce to.
            let settled_now = reg.complete(id, output);
            if let (true, Some(sid), Some(event)) = (settled_now, announce_sid, announcement) {
                process_completion::broadcast(&sid, event).await;
            }
        });

        match registry.register_running(preview, caller, join.abort_handle()) {
            RegisterOutcome::Registered(id) => {
                registry.attach_live(id, live);
                // Defence in depth (audit-2026-08-26 BTT-4): the registry
                // accepted the slot and the foreground path returned
                // "running" to the model. If the spawned task already exited
                // (panic before the oneshot receive), the model would be
                // told the job is running while the registry holds an
                // orphan. Today the gate awaits the oneshot inside the
                // spawned task, so this cannot happen — but the pattern is
                // one refactor away from a real silent orphan. Log loudly
                // so any future semantic change surfaces in tests / logs
                // instead of leaking the slot.
                if id_tx.send(id).is_err() {
                    tracing::warn!(
                        process_id = id,
                        "bash_exec: background task abandoned before handoff — \
                         registry slot may be orphaned"
                    );
                }
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

/// Recover the owning session from a label [`session_label`] wrote.
///
/// Lives next to its producer because it is that function's inverse and the two
/// are only correct together. The label is **serde JSON**, not
/// `SessionKey::to_key_string()` — reaching for `from_key_string` here returns
/// `None` for every row, which reads exactly like "this job had no session".
///
/// The live announce path never needs this: it holds the captured `SessionId`.
/// The boot handback does — a row on disk is all a later daemon has.
pub(crate) fn session_key_from_label(label: &str) -> Option<crate::session::service::SessionId> {
    serde_json::from_str(label).ok()
}

/// Abort every still-running background job in the global process registry.
///
/// Wired into the daemon's graceful-shutdown path so background bash / build
/// jobs do not outlive the core when an operator runs `daemon.shutdown`, hits
/// `Ctrl-C`, or the process receives `SIGTERM`. The registry is the
/// authoritative reaper — `tokio::process::Child::kill_on_drop` is best-effort
/// once the runtime itself is tearing down, and the only way to guarantee no
/// orphaned `cargo build` keeps writing into the workspace after the core has
/// gone is to ask every tracked task to abort up front. Returns the number of
/// processes that were signalled (purely informational; logging is fine).
pub fn kill_all_running_background() -> usize {
    use crate::builtin_tools::process_registry::process_registry;
    let n = process_registry().shutdown();
    if n > 0 {
        tracing::info!(
            killed = n,
            "aborted background bash processes during daemon shutdown"
        );
    }
    n
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
            let live: Vec<u64> = rows.iter().map(|r| r.id).collect();
            let mut payload = serde_json::Map::new();
            payload.insert("processes".into(), serde_json::json!(rows));
            let recovered = resolve_forgotten(None, caller.as_deref(), &live);
            if !recovered.is_empty() {
                payload.insert(
                    "recovered".into(),
                    serde_json::Value::Array(
                        recovered
                            .iter()
                            .map(|job| serde_json::Value::Object(recovered_row(job)))
                            .collect(),
                    ),
                );
                payload.insert(
                    "recovered_note".into(),
                    serde_json::json!(
                        "`recovered` rows come from the on-disk execution journal, not from a \
                         live handle: they outlive the daemon that started them. Read each \
                         row's `status` and `advisory` before acting on it."
                    ),
                );
            }
            info_output(serde_json::Value::Object(payload))
        }
        "poll" => {
            let Some(id) = process_id else {
                return error_output("bash: process_action=poll requires `process_id`");
            };
            match registry.poll(id, caller.as_deref()) {
                // Surface the captured tool output verbatim once finished.
                PollOutcome::Done(out) => *out,
                PollOutcome::Running {
                    elapsed_ms,
                    partial,
                } => info_output(running_payload(id, elapsed_ms, partial, None)),
                PollOutcome::Killed => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "killed",
                })),
                PollOutcome::NotFound => recovered_or_unknown(id, caller.as_deref(), None),
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
            // A mid-loop steer lands in the session log and the running loop
            // reads it at its next turn boundary — but this park *is* the turn,
            // for up to `WAIT_MAX_TIMEOUT_SECS`. Without this arm the user's
            // "actually, skip the integration tests" is durably written, the
            // send is reported successful, and the agent ignores it for the
            // rest of the build. Same rule `subagent{action:"wait"}` follows;
            // the registry's own `wait` stays untouched, because a second
            // signal in its signature would be a second source for six other
            // call sites that do not want one.
            //
            // Armed before the `select!` so a steer landing in the gap between
            // here and the first poll is remembered rather than dropped.
            let mut steer = crate::session::steer_signal::watch_current_turn();
            let outcome = tokio::select! {
                biased;
                () = steer.steered() => {
                    // Re-read rather than report from before the park: what the
                    // model gets must be the frontier as of the moment we let
                    // go — the same reason the registry's own timeout arm
                    // re-polls instead of extrapolating.
                    match registry.poll(id, caller.as_deref()) {
                        PollOutcome::Running { elapsed_ms, partial } => {
                            return info_output(running_payload(
                                id,
                                elapsed_ms,
                                partial,
                                Some(format!(
                                    "The user sent new input, so this wait returned early \
                                     instead of using its full {secs}s window — their \
                                     message is in your context, read it before deciding \
                                     what to do next. The process was NOT killed; it is \
                                     still running and can be waited on again with \
                                     {{\"process_action\":\"wait\",\"process_id\":{id}}}."
                                )),
                            ));
                        }
                        // Finished on the very instant we were woken: the result
                        // beats the interruption. Reporting "still running" for
                        // a job that is done would cost the model another whole
                        // turn to discover otherwise.
                        PollOutcome::Done(out) => return *out,
                        PollOutcome::Killed => WaitOutcome::Killed,
                        PollOutcome::NotFound => WaitOutcome::NotFound,
                    }
                }
                outcome = registry.wait(id, caller.as_deref(), Duration::from_secs(secs)) => outcome,
            };
            match outcome {
                // Finished within the window — surface the captured output.
                WaitOutcome::Done(out) => *out,
                WaitOutcome::Killed => info_output(serde_json::json!({
                    "process_id": id,
                    "status": "killed",
                })),
                WaitOutcome::TimedOut {
                    elapsed_ms,
                    partial,
                } => info_output(running_payload(
                    id,
                    elapsed_ms,
                    partial,
                    Some(format!(
                        "Still running after waiting {secs}s. Wait again or poll later with \
                         {{\"process_action\":\"poll\",\"process_id\":{id}}}."
                    )),
                )),
                WaitOutcome::NotFound => recovered_or_unknown(id, caller.as_deref(), None),
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
                // A journaled job has no `AbortHandle` in this process, so the
                // kill is NOT attempted — and saying so is the whole point:
                // silence here would read as "terminated".
                KillOutcome::NotFound => recovered_or_unknown(
                    id,
                    caller.as_deref(),
                    Some(
                        "kill was NOT attempted: this process holds no handle for this job. If \
                         its OS process is still alive, terminate it yourself (e.g. `pkill -f`).",
                    ),
                ),
            }
        }
        other => error_output(format!(
            "bash: unknown process_action '{other}' (expected poll|wait|kill|list)"
        )),
    }
}

/// **The** answer to "the in-memory registry cannot answer for this id".
///
/// One resolver for all four `process_action` faces: `poll` / `wait` / `kill`
/// ask it by id from their `NotFound` arms, `list` asks it (with `target =
/// None`) for everything this caller owns that the live table did not already
/// show. Answering that question per face is how a directory ends up
/// contradicting itself — the sub-agent tool shipped exactly that bug, `list`
/// rendering an id as recovered while `check_status` on the same id said "no
/// such thing", and `agents::subagent_tool::recovery::resolve_forgotten` is the
/// single-chokepoint shape being copied here.
///
/// Scoping is the journal's, which is strict equality on the owning session
/// label and refuses an unscoped caller outright.
fn resolve_forgotten(target: Option<u64>, caller: Option<&str>, live: &[u64]) -> Vec<RecoveredJob> {
    match target {
        Some(id) => process_journal::lookup(id, caller).into_iter().collect(),
        None => process_journal::list_for_scope(caller, live),
    }
}

/// Render a journaled job for a by-id face, or fall back to the pre-existing
/// unknown-id error when the journal has nothing either.
///
/// **Always [`info_output`], never [`error_output`].** A restart is not a
/// verdict on the call the model is making now: `success: false` /
/// `exit_code: -1` would teach it that *this* poll failed, when the poll
/// succeeded and its answer is "that job belonged to a daemon that is gone".
///
/// `skipped` names an action this face could not perform on a recovered row
/// (P7 / house rule: a fail-soft skip has to be stated in the result the model
/// reads, not inferred from a missing key).
fn recovered_or_unknown(id: u64, caller: Option<&str>, skipped: Option<&str>) -> CodeExecOutput {
    let Some(job) = resolve_forgotten(Some(id), caller, &[]).into_iter().next() else {
        return error_output(format!(
            "bash: no background process #{id} for this session"
        ));
    };
    let mut row = recovered_row(&job);
    if let Some(note) = skipped {
        row.insert("skipped".into(), serde_json::json!(note));
    }
    info_output(serde_json::Value::Object(row))
}

/// One journal row as the model sees it.
///
/// The advisory rides in the *response* rather than in the tool DESCRIPTION on
/// purpose (R9, second ruler): it is a runtime fact about one specific id, so
/// paying for it in every request's prompt would buy nothing.
fn recovered_row(job: &RecoveredJob) -> serde_json::Map<String, serde_json::Value> {
    let record = &job.record;
    let mut obj = serde_json::Map::new();
    obj.insert("process_id".into(), serde_json::json!(record.id));
    obj.insert(
        "status".into(),
        serde_json::json!(process_journal::settled_label(record)),
    );
    obj.insert("recovered".into(), serde_json::json!(true));
    obj.insert("command".into(), serde_json::json!(record.command));
    obj.insert("started_at_ms".into(), serde_json::json!(record.started_ms));
    if let Some(ended) = record.ended_ms {
        obj.insert("ended_at_ms".into(), serde_json::json!(ended));
    }
    if let Some(outcome) = record.outcome.as_deref() {
        obj.insert("outcome".into(), serde_json::json!(outcome));
    }
    // Deliberately NOT the envelope's `exit_code`: `info_output` stamps 0, and
    // two exit codes in one response is how the model learns the wrong one.
    if let Some(code) = record.exit_code {
        obj.insert("recorded_exit_code".into(), serde_json::json!(code));
    }
    if job.recorded_output.is_empty() {
        obj.insert(
            "recorded_output_absent".into(),
            serde_json::json!(no_output_reason(record.phase, record.outcome.as_deref())),
        );
    } else {
        obj.insert(
            "recorded_output".into(),
            serde_json::json!(job.recorded_output),
        );
        // A live capture is a WINDOW, not a result. Handing it over unlabelled
        // is how a model concludes a build succeeded because the last line it
        // can see happens not to be an error.
        if job.output_is_live_capture {
            obj.insert("recorded_output_is_partial".into(), serde_json::json!(true));
            obj.insert(
                "recorded_output_note".into(),
                serde_json::json!(
                    "`recorded_output` is a snapshot of what this job had printed while it was \
                     still running — NOT its final output. It holds at most the last 8 KB per \
                     stream, so the beginning is likely missing, anything printed after the last \
                     snapshot is absent, and there is no exit code behind it. Treat it as \
                     evidence of progress, never as a result."
                ),
            );
            obj.insert(
                "recorded_output_as_of_ms".into(),
                serde_json::json!(job.last_activity_ms),
            );
        }
    }
    obj.insert(
        "advisory".into(),
        serde_json::json!(advisory(record.phase, record.outcome.as_deref())),
    );
    obj
}

/// Why a recovered row carries no output. Silence would read as "the command
/// printed nothing", which is a different (and usually false) claim.
///
/// Each arm names the mechanism that would have produced output, and why it did
/// not: a job that outran no snapshot interval and a job that genuinely printed
/// nothing are the same empty string, and the model cannot tell them apart
/// without being told.
fn no_output_reason(phase: JobPhase, outcome: Option<&str>) -> &'static str {
    match (phase, outcome) {
        // The word comes from the writer's own vocabulary, never from a literal
        // spelled again here: two spellings of one verdict is one drift away
        // from an arm that quietly stops matching.
        (JobPhase::Settled, Some(o)) if o == Verdict::Killed.label() => {
            "No output was recorded: the job was killed before it produced a final result, and \
             the snapshot taken as it was stopped was empty — either it had printed nothing yet, \
             or what it had printed was withheld by the secret gate."
        }
        (JobPhase::Settled, _) => {
            "This job recorded no output — its stdout and stderr were both empty."
        }
        _ => {
            "No output was recorded. A job that does not reach a terminal state in the daemon \
             that ran it can only leave behind a periodic snapshot of its live output, and this \
             one has none: most likely it stopped within the first snapshot interval, or it had \
             printed nothing by then."
        }
    }
}

/// What the model must understand about a row that came off disk.
///
/// The interrupted case says more than the sub-agent sidecar's equivalent
/// because it knows less: a background `bash` child is a real OS process that
/// can outlive a `SIGKILL`ed daemon, no pid is recorded anywhere, and nothing
/// here probes for one. Claiming it died would be inventing a verdict.
///
/// Takes the outcome as well as the phase for the same reason
/// [`process_journal::settled_label`] takes the record: a terminal row is
/// either a completion or a `kill`, and the sentence that tells the model what
/// it is looking at may not answer the same way for both.
fn advisory(phase: JobPhase, outcome: Option<&str>) -> &'static str {
    match (phase, outcome) {
        (JobPhase::Settled, Some(o)) if o == Verdict::Killed.label() => {
            "Recovered from the on-disk execution journal: this job was STOPPED — Aleph killed \
             it, either on a `kill` action or when the daemon shut down. It did not run to \
             completion, so whatever output is here is partial by construction, and nothing about \
             the command itself was judged. Decide for yourself whether the work still needs doing."
        }
        (JobPhase::Settled, Some(o)) if o == Verdict::Completed.label() => {
            "Recovered from the on-disk execution journal: this job reached a terminal state \
             either in an earlier daemon or before its live entry was evicted. The fields here \
             are what was recorded then; there is no live handle to it."
        }
        (JobPhase::Settled, _) => {
            "Recovered from the on-disk execution journal: this row reached a terminal state but \
             recorded no outcome this daemon recognises, so it cannot tell you whether the job \
             finished or was stopped. Treat the fields below as evidence, not as a result."
        }
        (JobPhase::Interrupted, _) => {
            "This job was still running when the previous daemon stopped. Aleph no longer holds a \
             handle to it and did NOT check whether the OS process is still alive — it may still \
             be running, it may have finished, or it may have died with the daemon. This is not a \
             verdict on the command: nothing about it failed. Check yourself (e.g. `ps`) before \
             assuming either way, and before re-running work that may already be done."
        }
        (JobPhase::Running, _) => {
            "This job's journal row still says running, but this process holds no handle for it. \
             Aleph did NOT check whether the OS process is still alive."
        }
    }
}

/// Assemble the `status: "running"` payload for `poll` / `wait`.
///
/// The partial bytes go under their own `partial_stdout` / `partial_stderr`
/// keys and are deliberately NOT promoted into the envelope's `stdout` /
/// `stderr`: [`info_output`] hardcodes `success: true` / `exit_code: 0`, and an
/// `exit_code: 0` sitting next to a compiler error — while the tool description
/// teaches the model to read exit codes — is a worse bug than the silence this
/// replaces.
fn running_payload(
    id: u64,
    elapsed_ms: u64,
    partial: Option<LiveSnapshot>,
    message: Option<String>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("process_id".into(), serde_json::json!(id));
    obj.insert("status".into(), serde_json::json!("running"));
    obj.insert("elapsed_ms".into(), serde_json::json!(elapsed_ms));
    if let Some(msg) = message {
        obj.insert("message".into(), serde_json::json!(msg));
    }
    if let Some(snap) = partial {
        // Full byte counts, ring-independent: `bytes_so_far` minus what the
        // tail shows is what scrolled past.
        obj.insert(
            "bytes_so_far".into(),
            serde_json::json!({ "stdout": snap.stdout_total, "stderr": snap.stderr_total }),
        );
        let out_elided = snap.stdout_total.saturating_sub(snap.stdout.len() as u64);
        let err_elided = snap.stderr_total.saturating_sub(snap.stderr.len() as u64);
        if out_elided > 0 || err_elided > 0 {
            obj.insert(
                "partial_elided_bytes".into(),
                serde_json::json!({ "stdout": out_elided, "stderr": err_elided }),
            );
        }
        match partial_output::gate(&snap) {
            // Running, nothing printed yet: `bytes_so_far` already says 0/0.
            PartialView::Empty => {}
            PartialView::Text { stdout, stderr } => {
                if !stdout.is_empty() {
                    obj.insert("partial_stdout".into(), serde_json::json!(stdout));
                }
                if !stderr.is_empty() {
                    obj.insert("partial_stderr".into(), serde_json::json!(stderr));
                }
            }
            // Say what was skipped — silence here would read as "it printed
            // nothing", which is the opposite of what happened.
            PartialView::Withheld => {
                obj.insert(
                    "partial_withheld".into(),
                    serde_json::json!(
                        "Partial output withheld: block-class secret material was detected in \
                         the output so far. Poll again later, or inspect the job's own log file."
                    ),
                );
            }
        }
    }
    serde_json::Value::Object(obj)
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
    use crate::sandbox::live_tail::LIVE_TAIL_BYTES;

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

    /// The description promises an "8 KB" tail; the ring is sized by
    /// `LIVE_TAIL_BYTES`. Two statements of one fact — pin them together so a
    /// resize cannot leave the model reading a stale promise.
    #[test]
    fn description_partial_tail_size_matches_the_ring() {
        let d = <BashExecTool as AlephTool>::DESCRIPTION;
        assert_eq!(LIVE_TAIL_BYTES, 8 * 1024);
        assert!(d.contains("8 KB"), "should state the tail size");
        assert!(
            d.contains("partial_stdout") && d.contains("partial_stderr"),
            "should name the keys the model has to read"
        );
        assert!(
            !d.contains("not streamed mid-run"),
            "the old claim that output is only captured at the end is now false"
        );
    }

    fn snapshot(stdout: &[u8], stderr: &[u8], out_total: u64, err_total: u64) -> LiveSnapshot {
        LiveSnapshot {
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
            stdout_total: out_total,
            stderr_total: err_total,
        }
    }

    #[test]
    fn running_payload_carries_partial_under_its_own_keys() {
        let v = running_payload(
            7,
            1234,
            Some(snapshot(b"Compiling\n", b"warning: x\n", 10, 11)),
            None,
        );
        assert_eq!(v["status"], "running");
        assert_eq!(v["partial_stdout"], "Compiling\n");
        assert_eq!(v["partial_stderr"], "warning: x\n");
        assert_eq!(v["bytes_so_far"]["stdout"], 10);
        assert_eq!(v["bytes_so_far"]["stderr"], 11);
        // Nothing elided yet, so the key stays off the wire.
        assert!(v.get("partial_elided_bytes").is_none());
        // The envelope must NOT be told these are the command's real streams:
        // `info_output` stamps exit_code 0, and a 0 next to a compiler error
        // would teach the model the build passed.
        assert!(v.get("stdout").is_none() && v.get("stderr").is_none());
        let envelope = info_output(v);
        assert_eq!(envelope.exit_code, 0);
        assert!(
            envelope.stdout.contains("partial_stdout"),
            "partial rides inside the JSON payload, not the envelope streams"
        );
    }

    #[test]
    fn running_payload_reports_how_much_scrolled_past_the_ring() {
        let v = running_payload(1, 0, Some(snapshot(b"tail", b"", 4096, 0)), None);
        assert_eq!(v["bytes_so_far"]["stdout"], 4096);
        assert_eq!(v["partial_elided_bytes"]["stdout"], 4092);
        assert_eq!(v["partial_elided_bytes"]["stderr"], 0);
    }

    #[test]
    fn running_payload_with_no_output_yet_says_zero_not_nothing() {
        let v = running_payload(1, 5, Some(snapshot(b"", b"", 0, 0)), None);
        assert_eq!(v["bytes_so_far"]["stdout"], 0);
        assert!(v.get("partial_stdout").is_none());
        assert!(v.get("partial_withheld").is_none());
    }

    /// No live tail attached (a job registered before the tail was wired, or a
    /// backend that never tees) ⇒ no partial keys at all. An empty
    /// `partial_stdout` would claim the child printed nothing.
    #[test]
    fn running_payload_without_a_tail_omits_every_partial_key() {
        let v = running_payload(1, 5, None, None);
        assert!(v.get("bytes_so_far").is_none());
        assert!(v.get("partial_stdout").is_none());
    }

    /// Rider (a): the drain bytes are PRE-scrub. Ordinary secrets are redacted
    /// like the finished path redacts them...
    #[test]
    fn partial_output_is_scrubbed_like_the_finished_path() {
        let raw = b"token=ghp_0123456789abcdefghijklmnopqrstuvwx\n";
        let v = running_payload(1, 0, Some(snapshot(raw, b"", raw.len() as u64, 0)), None);
        let shown = v["partial_stdout"].as_str().expect("partial shown");
        assert!(
            !shown.contains("ghp_0123456789abcdefghijklmnopqrstuvwx"),
            "raw secret must not survive the scrub: {shown}"
        );
        assert!(shown.contains("[REDACTED:"), "redaction marker: {shown}");
    }

    /// ...and block-class material (which makes the FINISHED call fail closed)
    /// refuses the partial outright rather than handing back redacted text.
    /// The refusal is stated, not silent — silence would read as "no output".
    #[test]
    fn block_class_secret_withholds_the_partial_instead_of_redacting_it() {
        let raw = b"-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n";
        let v = running_payload(1, 0, Some(snapshot(raw, b"", raw.len() as u64, 0)), None);
        assert!(v.get("partial_stdout").is_none(), "must not render it");
        assert!(
            v["partial_withheld"]
                .as_str()
                .is_some_and(|s| s.contains("withheld")),
            "the skip must be stated in the result the model reads"
        );
        // Still a plain running status — the job itself is unaffected.
        assert_eq!(v["status"], "running");
        assert_eq!(v["bytes_so_far"]["stdout"], raw.len());
    }

    /// ANSI colour codes from a live build must be stripped on the partial path
    /// too — the finished path already does it, and a half-cleaned twin is how
    /// two views of one stream drift.
    #[test]
    fn partial_output_is_ansi_sanitized() {
        let raw = b"\x1b[32mok\x1b[0m\n";
        let v = running_payload(1, 0, Some(snapshot(raw, b"", raw.len() as u64, 0)), None);
        assert_eq!(v["partial_stdout"], "ok\n");
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

    /// A sandbox that writes into whatever live tail is in scope and then
    /// blocks until released — i.e. it behaves like a build that has printed
    /// its first lines but is nowhere near done.
    struct TeeThenBlockSandbox {
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl Sandbox for TeeThenBlockSandbox {
        async fn execute(
            &self,
            _cmd: crate::sandbox::SandboxCommand,
        ) -> std::result::Result<crate::sandbox::SandboxOutput, crate::sandbox::SandboxError>
        {
            let tail = crate::sandbox::context::current_live_tail()
                .expect("the background spawner must re-enter LIVE_TAIL inside the spawned task");
            tail.push(crate::sandbox::LiveStream::Stdout, b"Compiling alephcore\n");
            self.release.notified().await;
            Ok(crate::sandbox::SandboxOutput {
                stdout: b"Finished\n".to_vec(),
                exit_code: Some(0),
                ..Default::default()
            })
        }
    }

    /// The whole wire, end to end: `background: true` scopes a live tail inside
    /// the detached task (task-locals do not cross `tokio::spawn`), the sandbox
    /// sees it, the registry holds it, and `poll` renders a partial for a job
    /// that has NOT finished — the black box this change exists to open.
    #[tokio::test]
    async fn polling_a_still_running_background_job_shows_partial_output() {
        let release = Arc::new(tokio::sync::Notify::new());
        let sandbox: Arc<dyn Sandbox> = Arc::new(TeeThenBlockSandbox {
            release: release.clone(),
        });
        let tool = BashExecTool::new().with_sandbox(sandbox);
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-bg-partial");

        SESSION_ID
            .scope(session, async {
                let spawn = tool
                    .call(BashExecArgs {
                        cmd: "cargo build".to_string(),
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
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");

                // Poll until the detached task has reached the sandbox. The job
                // is still blocked, so this is a genuinely mid-run poll.
                let mut partial = None;
                for _ in 0..400 {
                    let polled = tool.call(args_action("poll", Some(id))).await.unwrap();
                    let v: serde_json::Value = serde_json::from_str(&polled.stdout).unwrap();
                    assert_eq!(v["status"], "running", "the job must still be blocked");
                    if v.get("partial_stdout").is_some() {
                        partial = Some(v);
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                let v = partial.expect("a running job must surface its output so far");
                assert_eq!(v["partial_stdout"], "Compiling alephcore\n");
                assert_eq!(v["bytes_so_far"]["stdout"], 20);

                // Release it: the final output takes over and the partial keys
                // are gone (the finished envelope is the authoritative answer).
                release.notify_one();
                for _ in 0..400 {
                    let polled = tool.call(args_action("poll", Some(id))).await.unwrap();
                    if !polled.stdout.contains("\"status\":\"running\"") {
                        assert_eq!(polled.exit_code, 0);
                        assert_eq!(polled.stdout, "Finished\n");
                        assert!(!polled.stdout.contains("partial_stdout"));
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                panic!("released job never completed");
            })
            .await;
    }

    /// Round-10 — a parked `wait` must come back when the user interjects.
    ///
    /// `wait` sleeps for up to `WAIT_MAX_TIMEOUT_SECS` (170 s). A mid-loop steer
    /// is written to the session log and the running loop reads that log at its
    /// next turn boundary — but this park *is* the turn, so without the steer
    /// arm the user's correction sits unread for the rest of the build while
    /// the client is told the send succeeded.
    ///
    /// Asserted on the consumer end and against a wall clock: the wait really
    /// returned early, it says why, and it says the process is untouched.
    /// Removing the `steer.steered()` arm makes this run the full 170 s and
    /// blow the 15 s bound.
    #[tokio::test]
    async fn a_steer_cuts_a_parked_process_wait_short() {
        use crate::routing::session_key::SessionKey;
        use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

        let release = Arc::new(tokio::sync::Notify::new());
        let sandbox: Arc<dyn Sandbox> = Arc::new(TeeThenBlockSandbox {
            release: release.clone(),
        });
        let tool = BashExecTool::new().with_sandbox(sandbox);
        let session = SessionKey::ephemeral("bash-steer-wait");
        let turn = TurnContext {
            session_key: session.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        };

        SESSION_ID
            .scope(session.clone(), async {
                let spawn = tool
                    .call(BashExecArgs {
                        cmd: "cargo build".to_string(),
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
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");

                let steered = session.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    crate::session::steer_signal::note_steer(&steered);
                });

                // `TURN_CONTEXT` is what production scopes at the tool-dispatch
                // chokepoint, and it is where the watch reads its session.
                // Bounded, not merely measured: with the arm removed this parks
                // for its full 170 s, and a test that wedges costs the suite its
                // signal — a red test has to be red quickly.
                let out = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    TURN_CONTEXT.scope(turn, handle_process_action("wait", Some(id), Some(170))),
                )
                .await
                .expect("a steered wait must not run out its window");

                let v: serde_json::Value = serde_json::from_str(&out.stdout)
                    .unwrap_or_else(|_| panic!("wait did not answer with a payload: {out:?}"));
                assert_eq!(
                    v["status"], "running",
                    "the job keeps running; only the wait ended"
                );
                let msg = v["message"].as_str().unwrap_or_default();
                assert!(
                    msg.contains("user sent new input"),
                    "the report must say WHY it came back early, or a 3-second wait \
                     reads as 'everything finished': {msg}"
                );
                assert!(
                    msg.contains("NOT killed"),
                    "the report must say the process survived: {msg}"
                );
                release.notify_one();
            })
            .await;
    }

    /// The other half of the same arm: with no `TURN_CONTEXT` (cron, internal
    /// runs, every other test in this file) the watch is inert and the wait
    /// must park normally. An inert arm that resolved immediately would turn
    /// every headless `wait` into a hot loop.
    #[tokio::test]
    async fn an_unscoped_process_wait_still_parks_for_its_window() {
        use crate::routing::session_key::SessionKey;

        let release = Arc::new(tokio::sync::Notify::new());
        let sandbox: Arc<dyn Sandbox> = Arc::new(TeeThenBlockSandbox {
            release: release.clone(),
        });
        let tool = BashExecTool::new().with_sandbox(sandbox);
        let session = SessionKey::ephemeral("bash-steer-inert");

        SESSION_ID
            .scope(session, async {
                let spawn = tool
                    .call(BashExecArgs {
                        cmd: "cargo build".to_string(),
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
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");

                let out = handle_process_action("wait", Some(id), Some(1)).await;
                let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
                assert_eq!(v["status"], "running");
                assert!(
                    v["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("Still running after waiting"),
                    "an unscoped wait must report its own timeout, not a steer: {}",
                    out.stdout
                );
                release.notify_one();
            })
            .await;
    }

    // ========================================================================
    // R5 completion announce — the producer half
    // ========================================================================

    /// Watches the global bus for `ProcessCompleted` events.
    ///
    /// Attached **before** the job is spawned: the announce is broadcast from
    /// the detached task the instant it settles, and a receiver that subscribes
    /// afterwards sees nothing — which would make every one of these tests pass
    /// for the wrong reason.
    struct CompletionWatch {
        rx: tokio::sync::broadcast::Receiver<crate::event::GlobalEvent>,
    }

    impl CompletionWatch {
        fn attach() -> Self {
            Self {
                rx: crate::event::GlobalBus::global().subscribe_broadcast(),
            }
        }

        /// Every completion announced for `id`, after draining the bus for
        /// `grace`. Drains for the whole window rather than returning on the
        /// first hit, because "exactly one" is the assertion that matters.
        async fn seen_for(
            &mut self,
            id: u64,
            grace: std::time::Duration,
        ) -> Vec<(String, crate::event::ProcessCompletionEvent)> {
            let deadline = std::time::Instant::now() + grace;
            let mut out = Vec::new();
            while std::time::Instant::now() < deadline {
                // Everything else is a non-event: `Closed` is impossible (the
                // bus holds the sender), `Lagged` only means other tests were
                // noisy, and the elapsed timeout is just an idle tick — all
                // three keep draining until the window is up.
                if let Ok(Ok(ev)) =
                    tokio::time::timeout(std::time::Duration::from_millis(20), self.rx.recv()).await
                {
                    if let crate::event::AlephEvent::ProcessCompleted(done) = &ev.event {
                        if done.process_id == id {
                            out.push((ev.source_session_id.clone(), done.clone()));
                        }
                    }
                }
            }
            out
        }
    }

    /// The gap this closes: a background job that finishes after the run which
    /// started it has ended used to reach nobody. One natural completion, one
    /// announce, addressed to the session that owns the job.
    ///
    /// RED without the broadcast in `spawn_background`: zero events.
    #[tokio::test]
    async fn a_session_scoped_job_announces_its_completion_exactly_once() {
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-announce-one");
        let mut watch = CompletionWatch::attach();

        let (id, seen) = SESSION_ID
            .scope(session.clone(), async {
                let tool = BashExecTool::new();
                let spawn = tool
                    .call(args_background("echo hi"))
                    .await
                    .expect("spawn succeeds");
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");
                let seen = watch.seen_for(id, std::time::Duration::from_secs(2)).await;
                (id, seen)
            })
            .await;

        assert_eq!(
            seen.len(),
            1,
            "a natural completion must announce exactly once"
        );
        let (announced_to, done) = &seen[0];
        assert_eq!(
            announced_to,
            &session.to_key_string(),
            "the announce must be addressed with the session key the announcer parses, \
             not the registry's serialized owner label"
        );
        assert_eq!(done.process_id, id);
        assert!(
            done.command.contains("echo hi"),
            "the notice names the job: {}",
            done.command
        );
    }

    /// A killed job is the owner's own synchronous action — its outcome is
    /// already in that tool call's return, so announcing it would spend a whole
    /// parent turn telling a session about a job it had just stopped. The
    /// registry's `Killed` verdict also wins over any late natural completion,
    /// so there is nothing to announce even if the task got that far.
    #[tokio::test]
    async fn a_killed_job_announces_nothing() {
        let release = Arc::new(tokio::sync::Notify::new());
        let sandbox: Arc<dyn Sandbox> = Arc::new(TeeThenBlockSandbox {
            release: release.clone(),
        });
        let tool = BashExecTool::new().with_sandbox(sandbox);
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-announce-kill");
        let mut watch = CompletionWatch::attach();

        SESSION_ID
            .scope(session, async {
                let spawn = tool
                    .call(args_background("cargo build"))
                    .await
                    .expect("spawn succeeds");
                let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
                let id = v["process_id"].as_u64().expect("process_id");

                let killed = tool.call(args_action("kill", Some(id))).await.unwrap();
                assert!(
                    killed.stdout.contains("\"status\":\"killed\""),
                    "{killed:?}"
                );

                release.notify_one();
                let seen = watch.seen_for(id, std::time::Duration::from_secs(1)).await;
                assert!(
                    seen.is_empty(),
                    "you asked for it to stop, so its outcome is not news: {seen:?}"
                );
            })
            .await;
    }

    /// No session means nobody to announce to. A CLI or library caller still
    /// gets the job, the registry entry and the poll face — it simply produces
    /// no event, because an event scoped to nothing has no addressee.
    #[tokio::test]
    async fn an_unscoped_job_announces_nothing() {
        let mut watch = CompletionWatch::attach();
        let tool = BashExecTool::new();

        let spawn = tool
            .call(args_background("echo hi"))
            .await
            .expect("spawn succeeds");
        let v: serde_json::Value = serde_json::from_str(&spawn.stdout).unwrap();
        let id = v["process_id"].as_u64().expect("process_id");

        // The job really does run and settle — this is not a test that passes
        // because nothing happened.
        let mut settled = false;
        for _ in 0..200 {
            if matches!(
                process_registry().poll(id, None),
                PollOutcome::Done(_) | PollOutcome::Killed
            ) {
                settled = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            settled,
            "the unscoped job must still reach a terminal state"
        );

        let seen = watch
            .seen_for(id, std::time::Duration::from_millis(500))
            .await;
        assert!(seen.is_empty(), "no session, no addressee: {seen:?}");
    }

    /// The label written into the registry and the journal is serde JSON, not
    /// `SessionKey::to_key_string()`. The boot handback is the one reader that
    /// has nothing but the label, so the two functions have to be inverses —
    /// and reaching for `from_key_string` there returns `None` for every row,
    /// which reads exactly like "this job had no session".
    #[tokio::test]
    async fn the_owner_label_round_trips_back_to_its_session_key() {
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-label-rt");
        let label = SESSION_ID
            .scope(session.clone(), async { session_label() })
            .await
            .expect("a scoped call has a label");

        assert_eq!(
            session_key_from_label(&label),
            Some(session.clone()),
            "session_label and session_key_from_label must be inverses"
        );
        assert!(
            crate::routing::session_key::SessionKey::from_key_string(&label).is_none(),
            "the label is NOT a key string; pinning this is what stops the boot \
             handback from silently deciding every recovered job is unowned"
        );
    }

    /// Spawn args for a background job.
    fn args_background(cmd: &str) -> BashExecArgs {
        BashExecArgs {
            cmd: cmd.to_string(),
            working_dir: None,
            timeout_seconds: None,
            allow_network: false,
            allow_subprocess: false,
            extra_writable_paths: Vec::new(),
            background: true,
            process_action: None,
            process_id: None,
            justification: None,
        }
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

    /// W5: every `process_action` face must reach the one journal resolver.
    ///
    /// A face left unwired keeps its own not-found arm, and bash reproduces
    /// verbatim the self-contradiction the sub-agent recovery module documents:
    /// `list` showing an id while `poll` on that same id insists it never
    /// existed. Each assertion names the face it is speaking for.
    #[tokio::test]
    async fn every_process_action_face_reaches_the_journal_resolver() {
        let _g = process_journal::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        let session = crate::routing::session_key::SessionKey::ephemeral("bash-journal-faces");

        SESSION_ID
            .scope(session, async {
                let owner = session_label().expect("the test runs inside a session scope");

                // A job the previous daemon left running...
                process_journal::enable_for_test(tmp.path().to_path_buf());
                process_journal::record_spawn(4242, "cargo build --release", Some(&owner));
                process_journal::disable_for_test();
                // ...and a fresh daemon booting over the same directory.
                process_journal::init_and_reconcile(tmp.path().to_path_buf());

                for face in ["poll", "wait", "kill"] {
                    let out = handle_process_action(face, Some(4242), Some(1)).await;
                    let v: serde_json::Value =
                        serde_json::from_str(&out.stdout).unwrap_or_else(|_| {
                            panic!("`{face}` did not answer with a payload: {out:?}")
                        });
                    assert_eq!(
                        v["status"], "interrupted_by_restart_liveness_unknown",
                        "`{face}` never reached the journal resolver: {}",
                        out.stdout
                    );
                    // The envelope has to be the `info_output` one: `error_output`
                    // stamps success:false / exit_code:-1, i.e. a verdict on the
                    // call the model is making right now.
                    assert!(out.success, "`{face}`: {}", out.stderr);
                    assert_eq!(out.exit_code, 0, "`{face}` used the failure envelope");
                    assert!(
                        out.stderr.is_empty(),
                        "`{face}` used the failure envelope: {}",
                        out.stderr
                    );
                }
                // `kill` additionally has to admit it did not kill anything.
                let killed = handle_process_action("kill", Some(4242), None).await;
                assert!(
                    killed.stdout.contains("kill was NOT attempted"),
                    "`kill` must state the skip, not imply success: {}",
                    killed.stdout
                );

                let listed = handle_process_action("list", None, None).await;
                let v: serde_json::Value = serde_json::from_str(&listed.stdout).unwrap();
                assert!(
                    v["recovered"]
                        .as_array()
                        .is_some_and(|rows| rows.iter().any(|r| r["process_id"] == 4242)),
                    "`list` never reached the journal resolver: {}",
                    listed.stdout
                );

                // Scoping still holds on the recovered path: an id that exists
                // for somebody else is still an unknown id here.
                process_journal::enable_for_test(tmp.path().to_path_buf());
                process_journal::record_spawn(4243, "sleep 9", Some("another-session"));
                let foreign = handle_process_action("poll", Some(4243), None).await;
                assert!(
                    !foreign.success && foreign.stderr.contains("no background process"),
                    "another session's journaled job must stay invisible: {foreign:?}"
                );
                process_journal::disable_for_test();
            })
            .await;
    }

    /// With the journal off (every test, every non-daemon binary) the four
    /// faces answer exactly as they did before it existed.
    #[tokio::test]
    async fn an_unknown_id_is_still_an_error_while_the_journal_is_off() {
        let _g = process_journal::test_gate();
        process_journal::disable_for_test();
        let out = bash(args_action("poll", Some(u64::MAX))).await;
        assert!(!out.success);
        assert!(
            out.stderr.contains("no background process"),
            "{}",
            out.stderr
        );
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
