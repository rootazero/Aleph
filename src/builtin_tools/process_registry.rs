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
use std::time::Instant;

use once_cell::sync::Lazy;
use tokio::task::AbortHandle;

use crate::builtin_tools::code_exec::CodeExecOutput;
use crate::sync_primitives::{Arc, AtomicU64, Mutex, MutexGuard, Ordering};

/// Upper bound on retained entries. Once exceeded we evict the oldest
/// *finished* entry (running ones are never dropped). Keeps the table from
/// growing without bound across a long-lived daemon.
const MAX_ENTRIES: usize = 64;

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
}

impl ProcessRegistry {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            procs: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<u64, ProcEntry>> {
        // P7: never propagate lock poisoning — a panicked background task
        // must not wedge the whole registry.
        self.procs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a freshly-spawned background task and return its id. Must be
    /// called *before* the task can finish so a fast-completing task always
    /// finds its slot in [`complete`](Self::complete).
    pub fn register_running(
        &self,
        command: impl Into<String>,
        session_label: Option<String>,
        abort: AbortHandle,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut procs = self.lock();
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
        id
    }

    /// Record a task's final output. No-op if the entry was already killed or
    /// evicted — a `Killed` verdict wins over a late natural completion.
    pub fn complete(&self, id: u64, output: CodeExecOutput) {
        let mut procs = self.lock();
        if let Some(entry) = procs.get_mut(&id) {
            if matches!(entry.state, ProcState::Running) {
                entry.state = ProcState::Done(Box::new(output));
            }
        }
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
        }
    }

    /// A spawned-and-immediately-finished task gives us a real `AbortHandle`.
    async fn live_handle() -> AbortHandle {
        let jh = tokio::spawn(async {});
        let h = jh.abort_handle();
        let _ = jh.await;
        h
    }

    #[tokio::test]
    async fn register_then_complete_then_poll_returns_output() {
        let reg = ProcessRegistry::new();
        let id = reg.register_running("echo hi", Some("s1".into()), live_handle().await);
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
        let id = reg.register_running("sleep 1", Some("owner".into()), live_handle().await);
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
        let id = reg.register_running("sleep 9", None, live_handle().await);
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
        let a = reg.register_running("first", Some("s".into()), live_handle().await);
        let b = reg.register_running("second", Some("s".into()), live_handle().await);
        let _other = reg.register_running("hidden", Some("other".into()), live_handle().await);
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
            let id = reg.register_running(format!("c{i}"), None, live_handle().await);
            reg.complete(id, dummy_output(0, ""));
            if i == 0 {
                first_done = Some(id);
            }
        }
        // One more registration triggers eviction of the oldest finished.
        let newest = reg.register_running("newest", None, live_handle().await);
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
}
