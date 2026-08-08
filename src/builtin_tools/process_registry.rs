//! Background-process registry for the `bash` tool.
//!
//! Aleph's `Sandbox::execute` is one-shot and blocking: a `bash` call waits up
//! to the tool budget (180s ceiling) and then returns. That's fine for quick
//! commands but forces the agent loop to block on long builds / installs and
//! to *lose* anything that runs past the ceiling.
//!
//! This registry layers an async, fire-and-forget mode on top of the existing
//! Sandbox seam **without touching the `Sandbox`/`OsSandboxDriverTrait`
//! contracts**. A backgrounded command is driven inside a detached
//! `tokio::task`; we keep its [`AbortHandle`] plus the eventual
//! [`CodeExecOutput`] in a process-global table keyed by a small integer id.
//!
//! Killing leans on a property of the platform drivers: they spawn the child
//! with `kill_on_drop(true)`, so aborting the task drops the in-flight future,
//! drops the `tokio::process::Child`, and the OS reaps the real process with
//! `SIGKILL`. No extra cancellation plumbing is needed.
//!
//! Isolation: every entry carries the caller's session label. `poll` / `kill`
//! / `list` only ever see entries belonging to the same session, so one
//! session cannot observe or terminate another's processes.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

use crate::builtin_tools::code_exec::CodeExecOutput;
use crate::sync_primitives::{Arc, AtomicU64, Mutex, MutexGuard, Ordering};

/// Upper bound on retained entries. Once exceeded we evict the oldest
/// *finished* entry (running ones are never dropped). Keeps the table from
/// growing without bound across a long-lived daemon.
const MAX_ENTRIES: usize = 64;

/// Per-session ceiling on *running* background processes. `evict_if_needed`
/// only ever drops finished entries, so without this gate a single session
/// could spawn unbounded never-ending background jobs (`sleep 9999 &` in a
/// loop), pushing the table past `MAX_ENTRIES` without bound and leaking one
/// real OS process + one detached `tokio::task` apiece. We refuse to register a
/// new background job once a session already has this many running, telling the
/// model to `poll`/`kill` its existing jobs first (R7: the model decides which
/// to reap, the registry just enforces the resource floor).
const MAX_RUNNING_PER_SESSION: usize = 8;

/// Longest command preview we keep for `list` display.
const COMMAND_PREVIEW_MAX: usize = 120;

/// Lifecycle state of a backgrounded command.
enum ProcState {
    /// Task is still running.
    Running,
    /// Task finished naturally; carries the captured tool output.
    Done(Box<CodeExecOutput>),
    /// Task was aborted via [`ProcessRegistry::kill`].
    Killed,
}

struct ProcEntry {
    command: String,
    session_label: Option<String>,
    started: Instant,
    abort: AbortHandle,
    state: ProcState,
}

/// Outcome of a [`ProcessRegistry::poll`] call.
pub enum PollOutcome {
    Running {
        elapsed_ms: u64,
    },
    Done(Box<CodeExecOutput>),
    Killed,
    /// No such id for this caller (unknown, or owned by another session).
    NotFound,
}

/// Outcome of a [`ProcessRegistry::kill`] call.
pub enum KillOutcome {
    Killed,
    AlreadyFinished,
    NotFound,
}

/// Outcome of [`ProcessRegistry::register_running`].
pub enum RegisterOutcome {
    /// Slot allocated; carries the new process id.
    Registered(u64),
    /// This session already has [`MAX_RUNNING_PER_SESSION`] running jobs.
    /// The caller must not spawn — it should poll/kill an existing one first.
    TooManyRunning { limit: usize },
}

/// Outcome of an [`ProcessRegistry::wait`] call — like [`PollOutcome`] but with
/// `TimedOut` instead of `Running`, since `wait` only returns `Running` shape
/// when the bounded wait window elapsed before the job finished.
pub enum WaitOutcome {
    /// Job finished within the wait window; carries the captured output.
    Done(Box<CodeExecOutput>),
    Killed,
    /// Wait window elapsed and the job is still running.
    TimedOut {
        elapsed_ms: u64,
    },
    NotFound,
}

/// One row of [`ProcessRegistry::list`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcSummary {
    pub id: u64,
    pub command: String,
    /// `"running"` | `"done"` | `"killed"`.
    pub status: &'static str,
    pub elapsed_ms: u64,
    /// Present once the process has finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

/// Process-global table of backgrounded `bash` commands.
pub struct ProcessRegistry {
    next_id: AtomicU64,
    procs: Mutex<HashMap<u64, ProcEntry>>,
    /// Fires on every state transition out of `Running` (complete / kill) so
    /// [`wait`](Self::wait) can sleep until *something* finishes instead of
    /// busy-polling. Coarse-grained on purpose: every waiter wakes and
    /// re-checks its own id (the running set is tiny — capped at
    /// [`MAX_RUNNING_PER_SESSION`] per session), so a shared notifier is
    /// cheaper than one channel per entry.
    completion: Notify,
}

impl ProcessRegistry {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            procs: Mutex::new(HashMap::new()),
            completion: Notify::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, ProcEntry>> {
        // P7: never propagate lock poisoning — a panicked background task
        // must not wedge the whole registry.
        self.procs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a freshly-spawned background task. Must be called *before* the
    /// task can finish so a fast-completing task always finds its slot in
    /// [`complete`](Self::complete).
    ///
    /// Enforces the per-session running cap atomically under the table lock: a
    /// session already at [`MAX_RUNNING_PER_SESSION`] running jobs gets
    /// [`RegisterOutcome::TooManyRunning`] and no slot — the count and the
    /// insert happen under one lock acquisition so concurrent spawns in the same
    /// session can't both slip past the gate.
    pub fn register_running(
        &self,
        command: impl Into<String>,
        session_label: Option<String>,
        abort: AbortHandle,
    ) -> RegisterOutcome {
        let mut procs = self.lock();
        let running = procs
            .values()
            .filter(|e| {
                e.session_label.as_deref() == session_label.as_deref()
                    && matches!(e.state, ProcState::Running)
            })
            .count();
        if running >= MAX_RUNNING_PER_SESSION {
            return RegisterOutcome::TooManyRunning {
                limit: MAX_RUNNING_PER_SESSION,
            };
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        evict_if_needed(&mut procs);
        procs.insert(
            id,
            ProcEntry {
                command: truncate_preview(command.into()),
                session_label,
                started: Instant::now(),
                abort,
                state: ProcState::Running,
            },
        );
        RegisterOutcome::Registered(id)
    }

    /// Record a task's final output. No-op if the entry was already killed or
    /// evicted — a `Killed` verdict wins over a late natural completion.
    pub fn complete(&self, id: u64, output: CodeExecOutput) {
        {
            let mut procs = self.lock();
            if let Some(entry) = procs.get_mut(&id) {
                if matches!(entry.state, ProcState::Running) {
                    entry.state = ProcState::Done(Box::new(output));
                }
            }
        }
        // Wake any `wait`ers so they re-check (and free a per-session slot).
        self.completion.notify_waiters();
    }

    /// Fetch a process's status / output. Only succeeds for entries owned by
    /// `caller` — a mismatch is reported as `NotFound` to avoid leaking the
    /// existence of another session's processes.
    pub fn poll(&self, id: u64, caller: Option<&str>) -> PollOutcome {
        let procs = self.lock();
        match procs.get(&id) {
            Some(entry) if owns(entry, caller) => match &entry.state {
                ProcState::Running => PollOutcome::Running {
                    elapsed_ms: elapsed_ms(entry.started),
                },
                ProcState::Done(out) => PollOutcome::Done(out.clone()),
                ProcState::Killed => PollOutcome::Killed,
            },
            _ => PollOutcome::NotFound,
        }
    }

    /// Abort a running process. Dropping the task fires `kill_on_drop` on the
    /// underlying child, so the real OS process is `SIGKILL`ed.
    pub fn kill(&self, id: u64, caller: Option<&str>) -> KillOutcome {
        let outcome = {
            let mut procs = self.lock();
            match procs.get_mut(&id) {
                Some(entry) if owns(entry, caller) => match entry.state {
                    ProcState::Running => {
                        entry.abort.abort();
                        entry.state = ProcState::Killed;
                        KillOutcome::Killed
                    }
                    _ => KillOutcome::AlreadyFinished,
                },
                _ => KillOutcome::NotFound,
            }
        };
        // A kill transitions out of `Running`, so unblock any `wait`ers and
        // free the per-session slot the killed job was holding.
        if matches!(outcome, KillOutcome::Killed) {
            self.completion.notify_waiters();
        }
        outcome
    }

    /// Block until process `id` (owned by `caller`) finishes, or until
    /// `timeout` elapses — whichever comes first. Unlike a `poll` loop this
    /// parks on the [`completion`](Self::completion) notifier and only re-checks
    /// when *some* job finishes, so it costs no CPU while waiting and returns
    /// the captured output the instant the job is done.
    ///
    /// Returns [`WaitOutcome::TimedOut`] (carrying elapsed time) when the window
    /// closes with the job still running — the caller can `wait` again or move
    /// on. `NotFound` semantics match [`poll`](Self::poll): another session's id
    /// is indistinguishable from an unknown one.
    pub async fn wait(&self, id: u64, caller: Option<&str>, timeout: Duration) -> WaitOutcome {
        let deadline = Instant::now() + timeout;
        loop {
            // Arm the notifier BEFORE inspecting state: `Notified::enable`
            // registers this waiter so a `complete`/`kill` racing between our
            // state read and our await still wakes us (no lost wakeup).
            let notified = self.completion.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            match self.poll(id, caller) {
                PollOutcome::Done(out) => return WaitOutcome::Done(out),
                PollOutcome::Killed => return WaitOutcome::Killed,
                PollOutcome::NotFound => return WaitOutcome::NotFound,
                PollOutcome::Running { elapsed_ms } => {
                    let now = Instant::now();
                    if now >= deadline {
                        return WaitOutcome::TimedOut { elapsed_ms };
                    }
                    let remaining = deadline - now;
                    tokio::select! {
                        () = &mut notified => { /* something finished — re-check */ }
                        () = tokio::time::sleep(remaining) => {
                            return WaitOutcome::TimedOut {
                                elapsed_ms: elapsed_ms.saturating_add(
                                    remaining.as_millis().try_into().unwrap_or(u64::MAX),
                                ),
                            };
                        }
                    }
                }
            }
        }
    }

    /// Abort every running entry the registry still tracks and return the
    /// count of processes that were signalled. Idempotent: a second call is
    /// cheap because the registry has already flipped each entry to
    /// `Killed` / evicted it. Wired by `BashExecTool`'s daemon-shutdown
    /// hook (`kill_all_running_background`) so background bash / build
    /// jobs do not outlive the core on `daemon.shutdown` / `SIGTERM` —
    /// `tokio::process::Child::kill_on_drop` is best-effort when the
    /// runtime itself is shutting down, so the registry has to be the
    /// authoritative reaper.
    pub fn shutdown(&self) -> usize {
        let mut procs = self.lock();
        let mut killed = 0usize;
        for entry in procs.values_mut() {
            if matches!(entry.state, ProcState::Running) {
                entry.abort.abort();
                entry.state = ProcState::Killed;
                killed += 1;
            }
        }
        if killed > 0 {
            self.completion.notify_waiters();
        }
        killed
    }

    /// Enumerate this caller's processes, newest first.
    pub fn list(&self, caller: Option<&str>) -> Vec<ProcSummary> {
        let procs = self.lock();
        let mut rows: Vec<(Instant, ProcSummary)> = procs
            .iter()
            .filter(|(_, e)| owns(e, caller))
            .map(|(id, e)| {
                let (status, exit_code) = match &e.state {
                    ProcState::Running => ("running", None),
                    ProcState::Done(out) => ("done", Some(out.exit_code)),
                    ProcState::Killed => ("killed", None),
                };
                (
                    e.started,
                    ProcSummary {
                        id: *id,
                        command: e.command.clone(),
                        status,
                        elapsed_ms: elapsed_ms(e.started),
                        exit_code,
                    },
                )
            })
            .collect();
        rows.sort_by_key(|x| std::cmp::Reverse(x.0));
        rows.into_iter().map(|(_, s)| s).collect()
    }
}

fn owns(entry: &ProcEntry, caller: Option<&str>) -> bool {
    entry.session_label.as_deref() == caller
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn truncate_preview(mut cmd: String) -> String {
    // First line only — multi-line scripts are summarised by their head.
    if let Some(nl) = cmd.find('\n') {
        cmd.truncate(nl);
    }
    if cmd.len() > COMMAND_PREVIEW_MAX {
        // UTF-8 safe truncation (P7): back off to a char boundary.
        let mut end = COMMAND_PREVIEW_MAX;
        while end > 0 && !cmd.is_char_boundary(end) {
            end -= 1;
        }
        cmd.truncate(end);
        cmd.push('…');
    }
    cmd
}

/// Evict the oldest *finished* entry when the table is full. Running entries
/// are sacrosanct — losing their handle would orphan a live OS process.
fn evict_if_needed(procs: &mut HashMap<u64, ProcEntry>) {
    if procs.len() < MAX_ENTRIES {
        return;
    }
    let victim = procs
        .iter()
        .filter(|(_, e)| !matches!(e.state, ProcState::Running))
        .min_by_key(|(_, e)| e.started)
        .map(|(id, _)| *id);
    if let Some(id) = victim {
        procs.remove(&id);
    }
}

/// Process-global registry shared by every `bash` tool instance.
static REGISTRY: Lazy<Arc<ProcessRegistry>> = Lazy::new(|| Arc::new(ProcessRegistry::new()));

/// Handle to the shared background-process registry.
pub fn process_registry() -> Arc<ProcessRegistry> {
    REGISTRY.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_output(exit: i32, stdout: &str) -> CodeExecOutput {
        CodeExecOutput {
            success: exit == 0,
            exit_code: exit,
            stdout: stdout.to_string(),
            stderr: String::new(),
            duration_ms: 1,
            language: "shell".into(),
            truncated: None,
            stdout_truncated_bytes: 0,
            stderr_truncated_bytes: 0,
            advisory: None,
        }
    }

    /// A spawned-and-immediately-finished task gives us a real `AbortHandle`.
    async fn live_handle() -> AbortHandle {
        let jh = tokio::spawn(async move {});
        let h = jh.abort_handle();
        let _ = jh.await;
        h
    }

    /// Unwrap a successful registration to its id, panicking if the per-session
    /// cap unexpectedly tripped. Keeps the existing tests terse.
    fn unwrap_id(outcome: RegisterOutcome) -> u64 {
        match outcome {
            RegisterOutcome::Registered(id) => id,
            RegisterOutcome::TooManyRunning { limit } => {
                panic!("unexpected per-session cap hit (limit {limit})")
            }
        }
    }

    #[tokio::test]
    async fn register_then_complete_then_poll_returns_output() {
        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("echo hi", Some("s1".into()), live_handle().await));
        // Before completion → Running.
        assert!(matches!(
            reg.poll(id, Some("s1")),
            PollOutcome::Running { .. }
        ));
        reg.complete(id, dummy_output(0, "hi\n"));
        match reg.poll(id, Some("s1")) {
            PollOutcome::Done(out) => {
                assert_eq!(out.exit_code, 0);
                assert_eq!(out.stdout, "hi\n");
            }
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn cross_session_poll_and_kill_are_not_found() {
        let reg = ProcessRegistry::new();
        let id =
            unwrap_id(reg.register_running("sleep 1", Some("owner".into()), live_handle().await));
        // A different session must not see it.
        assert!(matches!(
            reg.poll(id, Some("intruder")),
            PollOutcome::NotFound
        ));
        assert!(matches!(
            reg.kill(id, Some("intruder")),
            KillOutcome::NotFound
        ));
        // Owner still can.
        assert!(matches!(reg.kill(id, Some("owner")), KillOutcome::Killed));
    }

    #[tokio::test]
    async fn kill_marks_killed_and_blocks_late_completion() {
        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        assert!(matches!(reg.kill(id, None), KillOutcome::Killed));
        // A late natural completion must NOT overwrite the Killed verdict.
        reg.complete(id, dummy_output(0, "late"));
        assert!(matches!(reg.poll(id, None), PollOutcome::Killed));
        // Second kill is a no-op.
        assert!(matches!(reg.kill(id, None), KillOutcome::AlreadyFinished));
    }

    #[tokio::test]
    async fn list_is_session_scoped_and_newest_first() {
        let reg = ProcessRegistry::new();
        let a = unwrap_id(reg.register_running("first", Some("s".into()), live_handle().await));
        let b = unwrap_id(reg.register_running("second", Some("s".into()), live_handle().await));
        let _other =
            unwrap_id(reg.register_running("hidden", Some("other".into()), live_handle().await));
        let rows = reg.list(Some("s"));
        assert_eq!(rows.len(), 2, "only this session's procs");
        // Newest (b) first.
        assert_eq!(rows[0].id, b);
        assert_eq!(rows[1].id, a);
    }

    #[tokio::test]
    async fn eviction_drops_oldest_finished_but_spares_running() {
        let reg = ProcessRegistry::new();
        // Fill to capacity with finished entries.
        let mut first_done = None;
        for i in 0..MAX_ENTRIES {
            let id = unwrap_id(reg.register_running(format!("c{i}"), None, live_handle().await));
            reg.complete(id, dummy_output(0, ""));
            if i == 0 {
                first_done = Some(id);
            }
        }
        // One more registration triggers eviction of the oldest finished.
        let newest = unwrap_id(reg.register_running("newest", None, live_handle().await));
        assert!(matches!(
            reg.poll(first_done.unwrap(), None),
            PollOutcome::NotFound
        ));
        assert!(matches!(
            reg.poll(newest, None),
            PollOutcome::Running { .. }
        ));
    }

    #[test]
    fn preview_truncates_multiline_and_long() {
        assert_eq!(truncate_preview("line1\nline2".into()), "line1");
        let long = "x".repeat(200);
        let p = truncate_preview(long);
        assert!(p.chars().count() <= COMMAND_PREVIEW_MAX + 1);
        assert!(p.ends_with('…'));
    }

    #[tokio::test]
    async fn per_session_running_cap_refuses_excess_then_recovers() {
        let reg = ProcessRegistry::new();
        let sess = Some("capper".to_string());
        // Fill the session's running quota.
        let mut ids = Vec::new();
        for _ in 0..MAX_RUNNING_PER_SESSION {
            ids.push(unwrap_id(reg.register_running(
                "sleep 9",
                sess.clone(),
                live_handle().await,
            )));
        }
        // One past the cap is refused — no slot allocated.
        assert!(matches!(
            reg.register_running("sleep 9", sess.clone(), live_handle().await),
            RegisterOutcome::TooManyRunning {
                limit: MAX_RUNNING_PER_SESSION
            }
        ));
        // A *different* session is unaffected (cap is per-session).
        assert!(matches!(
            reg.register_running("sleep 9", Some("other".into()), live_handle().await),
            RegisterOutcome::Registered(_)
        ));
        // Finishing one frees a slot for this session.
        reg.complete(ids[0], dummy_output(0, "done\n"));
        assert!(matches!(
            reg.register_running("sleep 9", sess, live_handle().await),
            RegisterOutcome::Registered(_)
        ));
    }

    #[tokio::test]
    async fn wait_returns_output_once_complete() {
        let reg = Arc::new(ProcessRegistry::new());
        let id = unwrap_id(reg.register_running("echo hi", Some("w".into()), live_handle().await));
        // Complete it shortly after a waiter parks.
        let reg2 = reg.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            reg2.complete(id, dummy_output(0, "hi\n"));
        });
        match reg.wait(id, Some("w"), Duration::from_secs(5)).await {
            WaitOutcome::Done(out) => assert_eq!(out.stdout, "hi\n"),
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn wait_times_out_while_still_running() {
        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("sleep 9", Some("w".into()), live_handle().await));
        match reg.wait(id, Some("w"), Duration::from_millis(30)).await {
            WaitOutcome::TimedOut { .. } => {}
            _ => panic!("expected TimedOut for a still-running job"),
        }
    }

    #[tokio::test]
    async fn wait_is_session_scoped_not_found() {
        let reg = ProcessRegistry::new();
        let id =
            unwrap_id(reg.register_running("sleep 9", Some("owner".into()), live_handle().await));
        assert!(matches!(
            reg.wait(id, Some("intruder"), Duration::from_millis(10))
                .await,
            WaitOutcome::NotFound
        ));
    }

    #[tokio::test]
    async fn wait_observes_kill() {
        let reg = Arc::new(ProcessRegistry::new());
        let id = unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        let reg2 = reg.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            reg2.kill(id, None);
        });
        assert!(matches!(
            reg.wait(id, None, Duration::from_secs(5)).await,
            WaitOutcome::Killed
        ));
    }

    /// `shutdown` is the daemon-shutdown hook: it must abort every running
    /// entry, leave finished entries alone, and be safely callable twice
    /// (a graceful shutdown followed by `std::process::exit` re-entry, or
    /// a panic on the shutdown path). The `AbortHandle::abort` is a
    /// best-effort signal — the test asserts only the registry-side state,
    /// not that the detached task has actually reaped its child (covered
    /// by the platform drivers' `kill_on_drop`).
    #[tokio::test]
    async fn shutdown_aborts_running_but_spares_finished() {
        let reg = ProcessRegistry::new();
        let running = unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        let finished = unwrap_id(reg.register_running("true", None, live_handle().await));
        reg.complete(finished, dummy_output(0, ""));
        let n = reg.shutdown();
        assert_eq!(n, 1, "exactly one running entry aborted");
        assert!(matches!(reg.poll(running, None), PollOutcome::Killed));
        assert!(matches!(reg.poll(finished, None), PollOutcome::Done(_)));
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let reg = ProcessRegistry::new();
        unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        assert_eq!(reg.shutdown(), 1);
        // Second call: nothing running, no abort, returns 0.
        assert_eq!(reg.shutdown(), 0);
    }
}
