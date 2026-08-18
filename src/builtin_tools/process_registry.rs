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
//!
//! Partial output: a running entry may also carry an
//! [`Arc<LiveTail>`](crate::sandbox::live_tail::LiveTail) attached by
//! [`attach_live`](ProcessRegistry::attach_live), which the platform drivers'
//! drain loops feed as the child writes. `poll` / `wait` hand a snapshot of it
//! back so a 20-minute build is observable mid-run. The handle is released the
//! instant the entry leaves `Running` — those raw rings must not sit next to an
//! already-truncated `CodeExecOutput` for the rest of the entry's retention.
//! **The snapshot bytes are PRE-scrub**; rendering one without re-running the
//! finished path's secret gate would make "poll a running job" a way to read
//! what the completed path refuses. The one gate all of them share lives in
//! [`partial_output`](super::partial_output).
//!
//! The rings are also the *only* record a job aborted mid-flight leaves behind
//! — an abort drops the future, so there is no `CodeExecOutput` at all — so
//! `kill` and `shutdown` take a gated snapshot on their way through, at the
//! last instant the bytes exist. See [`journal_live_tail`].
//!
//! Durability: this table is pure process memory, so a daemon restart used to
//! turn every id into `"no background process #N for this session"` — *"there
//! was never such a thing"*. Each state transition here is therefore mirrored
//! into [`process_journal`](super::process_journal), the on-disk sidecar that
//! lets a later daemon answer for an id it never issued. The journal is a
//! zero-I/O no-op until boot enables it, so nothing below changes behaviour in
//! tests, the CLI, or any embedding that never calls its boot reconcile.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

use crate::builtin_tools::code_exec::CodeExecOutput;
use crate::builtin_tools::partial_output;
use crate::builtin_tools::process_journal::{self, Verdict};
use crate::sandbox::live_tail::{LiveSnapshot, LiveTail};
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

/// BT-D-R4-17: per-daemon ceiling on *running* background processes across
/// all sessions. Without this gate, N sessions each at the per-session cap
/// still produce N*8 detached OS processes + N*8 detached `tokio::task`s
/// inside one daemon, which is a slow-burn DoS for the host (file
/// descriptors, kernel process table, scheduler) and a memory leak for the
/// daemon's own tables. We refuse to register once the daemon is already
/// hosting this many running jobs; the model should poll/kill across
/// sessions before spawning more.
const MAX_RUNNING_GLOBAL: usize = 32;

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
    /// Rolling tail of the child's output while it runs. `None` before
    /// [`ProcessRegistry::attach_live`] runs, and dropped again the moment the
    /// entry leaves `Running` (see the module docs).
    live: Option<Arc<LiveTail>>,
    /// Whether the model has already collected this job's terminal result
    /// through a `poll` / `wait`.
    ///
    /// Read by the proactive completion announcer
    /// (`gateway::process_announce`) before it spends a whole parent turn
    /// delivering a result the model already folded in — the same job
    /// `BackgroundAgentTracker::consumed` does for background sub-agents. Set
    /// by [`poll`](ProcessRegistry::poll) on a terminal read, which is also the
    /// read `wait` returns through, so both faces stamp it without a second
    /// call site to forget.
    reported: bool,
}

impl ProcEntry {
    /// Flip out of `Running` and release the live-tail rings in one step, so
    /// no state-transition site can keep the buffers alive by forgetting. The
    /// caller must already hold the table lock.
    fn retire(&mut self, state: ProcState) {
        self.state = state;
        self.live = None;
    }
}

/// Outcome of a [`ProcessRegistry::poll`] call.
pub enum PollOutcome {
    Running {
        elapsed_ms: u64,
        /// Partial output captured so far, when a live tail is attached.
        /// **PRE-scrub** — the caller must run the same secret gate the
        /// finished path runs before showing any of it.
        partial: Option<LiveSnapshot>,
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
        /// Partial output captured so far — same PRE-scrub contract as the
        /// `partial` field of [`PollOutcome::Running`].
        partial: Option<LiveSnapshot>,
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
    /// Whether this instance may write to the process-global
    /// [`process_journal`]. True for exactly one registry — the
    /// [`REGISTRY`] singleton.
    ///
    /// This is an INSTANCE property, not a global read, and the reason is a
    /// bug this flag exists to make unrepresentable: `next_id` starts at 1 in
    /// every `ProcessRegistry`, so two registries hand out the same ids. The
    /// journal is keyed by that id. A second registry writing into the same
    /// store therefore does not merely add rows — it *overwrites* the first
    /// one's row `#1` with a record carrying a different owner, and the
    /// original owner's `lookup(1)` then answers "no such job". The daemon has
    /// exactly one registry, so in production the question never arises; tests
    /// construct their own by the dozen, which is where it bit.
    journaled: bool,
}

impl ProcessRegistry {
    /// A standalone registry that never touches the durable journal.
    ///
    /// `#[cfg(test)]` on purpose, and that is the enforcement: production has
    /// exactly one registry and it must be the journaling one, so making the
    /// non-journaling constructor unreachable outside tests turns "do not
    /// build a second writer" from a convention into a compile error.
    #[cfg(test)]
    fn new() -> Self {
        Self::build(false)
    }

    /// The one registry allowed to journal. See [`journaled`](Self::journaled)
    /// for why this is not simply "the journal is enabled".
    fn new_journaled() -> Self {
        Self::build(true)
    }

    fn build(journaled: bool) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            procs: Mutex::new(HashMap::new()),
            completion: Notify::new(),
            journaled,
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
        let (id, preview) = {
            let mut procs = self.lock();
            // BT-D-R4-17: count BOTH per-session and global running jobs in
            // one pass so a single lock acquisition enforces both caps
            // atomically. The earlier code only checked the per-session
            // count, which let any number of sessions each at the per-session
            // cap accumulate a slow-burn resource leak.
            let mut per_session_running = 0usize;
            let mut global_running = 0usize;
            for e in procs.values() {
                if !matches!(e.state, ProcState::Running) {
                    continue;
                }
                global_running += 1;
                if e.session_label.as_deref() == session_label.as_deref() {
                    per_session_running += 1;
                }
            }
            if global_running >= MAX_RUNNING_GLOBAL {
                return RegisterOutcome::TooManyRunning {
                    limit: MAX_RUNNING_GLOBAL,
                };
            }
            if per_session_running >= MAX_RUNNING_PER_SESSION {
                return RegisterOutcome::TooManyRunning {
                    limit: MAX_RUNNING_PER_SESSION,
                };
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            // Durably reserve BEFORE the id can escape this function (including
            // via a concurrent `list` seeing the row we are about to insert).
            // Reserving after would re-issue this id to a later daemon that
            // crashed in between — the same collision one layer down. Costs one
            // small write per reservation block, and nothing at all while the
            // journal is off.
            if self.journaled {
                process_journal::reserve_id(id);
            }
            let preview = truncate_preview(command.into());
            evict_if_needed(&mut procs);
            procs.insert(
                id,
                ProcEntry {
                    command: preview.clone(),
                    session_label: session_label.clone(),
                    started: Instant::now(),
                    abort,
                    state: ProcState::Running,
                    live: None,
                    reported: false,
                },
            );
            (id, preview)
        };
        // Outside the table lock: this is the per-spawn file write.
        if self.journaled {
            process_journal::record_spawn(id, &preview, session_label.as_deref());
        }
        RegisterOutcome::Registered(id)
    }

    /// Raise the id allocator's floor so it never re-issues an id a previous
    /// daemon handed out. Monotonic — a lower floor is ignored.
    ///
    /// Sole production caller is `process_journal::init_and_reconcile`, which
    /// knows the durable high-water mark; wiring it there (rather than at the
    /// boot call site) means no future boot path can enable the journal and
    /// forget the allocator.
    pub fn seed_id_floor(&self, floor: u64) {
        self.next_id.fetch_max(floor, Ordering::Relaxed);
    }

    /// Attach the live output tail the job's drain loops are feeding, so
    /// `poll` / `wait` can show partial output while it runs.
    ///
    /// Deliberately a separate call rather than a fourth parameter on
    /// [`register_running`](Self::register_running): the tail is optional and
    /// orthogonal to slot allocation, and widening that signature would churn
    /// two dozen call sites (all but one of them tests) for a value they do not
    /// have. Attaching to an entry that is no longer `Running` is a no-op — a
    /// job that finished or was killed between spawn and attach must not have
    /// its rings resurrected.
    pub fn attach_live(&self, id: u64, live: Arc<LiveTail>) {
        let mut procs = self.lock();
        if let Some(entry) = procs.get_mut(&id) {
            if matches!(entry.state, ProcState::Running) {
                entry.live = Some(live);
            }
        }
    }

    /// Record a task's final output. No-op if the entry was already killed or
    /// evicted — a `Killed` verdict wins over a late natural completion.
    ///
    /// Returns whether **this** call performed the `Running → Done` transition.
    /// The caller needs the effect, not the call: a late completion landing
    /// after a `kill` leaves no trace here, and announcing it would tell the
    /// session a job finished that it had already stopped. The registry
    /// deliberately does not broadcast that itself — it holds no bus and no
    /// session key (its owner label is a serialized `SessionId`, not an
    /// announce-addressable one) — so it reports the transition and lets the
    /// spawner, which holds both, decide.
    pub fn complete(&self, id: u64, output: CodeExecOutput) -> bool {
        let mut landed = false;
        let journaled = {
            let mut procs = self.lock();
            match procs.get_mut(&id) {
                Some(entry) if matches!(entry.state, ProcState::Running) => {
                    landed = true;
                    // The journal's copy is taken here, under the same lock that
                    // decides the transition, so a racing `kill` cannot make the
                    // disk say "completed" while memory says "killed". Only paid
                    // when the journal is on: every test and non-daemon binary
                    // skips the clone entirely. The bytes are the sandbox's
                    // finished-path output, i.e. already scrubbed and gated.
                    let copy = (self.journaled && process_journal::is_enabled()).then(|| {
                        (
                            output.exit_code,
                            output.stdout.clone(),
                            output.stderr.clone(),
                        )
                    });
                    // Same lock acquisition drops the live rings: the finished
                    // entry keeps only its (already truncated) CodeExecOutput.
                    entry.retire(ProcState::Done(Box::new(output)));
                    copy
                }
                _ => None,
            }
        };
        if let Some((exit_code, stdout, stderr)) = journaled {
            process_journal::record_settled(
                id,
                Verdict::Completed,
                Some(exit_code),
                &stdout,
                &stderr,
            );
        }
        // Wake any `wait`ers so they re-check (and free a per-session slot).
        self.completion.notify_waiters();
        landed
    }

    /// Fetch a process's status / output. Only succeeds for entries owned by
    /// `caller` — a mismatch is reported as `NotFound` to avoid leaking the
    /// existence of another session's processes.
    ///
    /// A terminal read stamps [`reported`](ProcEntry::reported): from here on
    /// the model has the result, so the proactive announcer must not spend a
    /// parent turn re-delivering it. `wait` returns through this same read, so
    /// both collecting faces stamp it.
    pub fn poll(&self, id: u64, caller: Option<&str>) -> PollOutcome {
        let mut procs = self.lock();
        let Some(entry) = procs.get_mut(&id).filter(|e| owns(e, caller)) else {
            return PollOutcome::NotFound;
        };
        match &entry.state {
            ProcState::Running => PollOutcome::Running {
                elapsed_ms: elapsed_ms(entry.started),
                partial: entry.live.as_ref().map(|t| t.snapshot()),
            },
            ProcState::Done(out) => {
                let out = out.clone();
                entry.reported = true;
                PollOutcome::Done(out)
            }
            ProcState::Killed => {
                entry.reported = true;
                PollOutcome::Killed
            }
        }
    }

    /// Whether the model already collected this job's terminal result.
    ///
    /// The announcer's dedup predicate. `false` for unknown / still-running /
    /// evicted ids — nothing to suppress, and the fail direction is a
    /// duplicate notice rather than a silently withheld one.
    #[must_use]
    pub fn is_reported(&self, id: u64) -> bool {
        self.lock().get(&id).is_some_and(|e| e.reported)
    }

    /// Abort a running process. Dropping the task fires `kill_on_drop` on the
    /// underlying child, so the real OS process is `SIGKILL`ed.
    pub fn kill(&self, id: u64, caller: Option<&str>) -> KillOutcome {
        let (outcome, live) = {
            let mut procs = self.lock();
            match procs.get_mut(&id) {
                Some(entry) if owns(entry, caller) => match entry.state {
                    ProcState::Running => {
                        entry.abort.abort();
                        // Take the tail BEFORE retiring: `retire` releases the
                        // rings under this same lock, and this is the last
                        // instant the bytes exist anywhere.
                        let live = entry.live.clone();
                        entry.retire(ProcState::Killed);
                        (KillOutcome::Killed, live)
                    }
                    _ => (KillOutcome::AlreadyFinished, None),
                },
                _ => (KillOutcome::NotFound, None),
            }
        };
        // A kill transitions out of `Running`, so unblock any `wait`ers and
        // free the per-session slot the killed job was holding.
        if matches!(outcome, KillOutcome::Killed) {
            if self.journaled {
                // Aborting drops the in-flight future, so there is no
                // `CodeExecOutput` — but there IS a live tail, and this is the
                // moment it stops existing. Gating and writing happen outside
                // the table lock: the gate runs secret patterns over 16 KiB and
                // the write touches the disk, neither of which belongs under a
                // lock every poll contends for.
                journal_live_tail(id, live.as_deref());
                process_journal::record_settled(id, Verdict::Killed, None, "", "");
            }
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
    /// Returns [`WaitOutcome::TimedOut`] (carrying elapsed time and whatever
    /// partial output the live tail holds) when the window closes with the job
    /// still running — the caller can `wait` again or move on. `NotFound`
    /// semantics match [`poll`](Self::poll): another session's id is
    /// indistinguishable from an unknown one.
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
                PollOutcome::Running {
                    elapsed_ms,
                    partial,
                } => {
                    let now = Instant::now();
                    if now >= deadline {
                        return WaitOutcome::TimedOut {
                            elapsed_ms,
                            partial,
                        };
                    }
                    let remaining = deadline - now;
                    tokio::select! {
                        () = &mut notified => { /* something finished — re-check */ }
                        () = tokio::time::sleep(remaining) => {
                            // Re-read rather than extrapolate: the partial we
                            // return must be the frontier as of the moment the
                            // window closed, not the one from the top of the
                            // loop a whole wait-window ago. Re-reading also
                            // catches a job that finished right on the deadline
                            // (Done beats TimedOut for the same instant).
                            return match self.poll(id, caller) {
                                PollOutcome::Done(out) => WaitOutcome::Done(out),
                                PollOutcome::Killed => WaitOutcome::Killed,
                                PollOutcome::NotFound => WaitOutcome::NotFound,
                                PollOutcome::Running { elapsed_ms, partial } => {
                                    WaitOutcome::TimedOut { elapsed_ms, partial }
                                }
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
        let reaped: Vec<(u64, Option<Arc<LiveTail>>)> = {
            let mut procs = self.lock();
            let mut reaped = Vec::new();
            for (id, entry) in procs.iter_mut() {
                if matches!(entry.state, ProcState::Running) {
                    entry.abort.abort();
                    // Take the tail before `retire` releases it (see `kill`).
                    let live = entry.live.clone();
                    entry.retire(ProcState::Killed);
                    reaped.push((*id, live));
                }
            }
            reaped
        };
        // An orderly shutdown is a verdict Aleph earned by aborting the task,
        // so these rows must not be left `Running` on disk — the next boot
        // would tombstone them as "still running when the daemon stopped,
        // liveness unknown", which is exactly the thing we DO know is false.
        // They keep what they had printed, though: an operator who stops the
        // daemon mid-build still gets to see how far it had got.
        if self.journaled {
            for (id, live) in &reaped {
                journal_live_tail(*id, live.as_deref());
                process_journal::record_settled(*id, Verdict::Killed, None, "", "");
            }
        }
        if !reaped.is_empty() {
            self.completion.notify_waiters();
        }
        reaped.len()
    }

    /// Every running job that has a live tail, for the journal's periodic
    /// flusher. Not scoped to a caller: the flusher writes into each job's own
    /// owner-scoped row and nothing here is rendered to anyone.
    ///
    /// Sole consumer is `process_journal::spawn_partial_flusher`. If that ever
    /// goes away, so does this (R10) — it is not a general-purpose accessor and
    /// must not become one, because handing out another session's live output
    /// is precisely what `poll`'s owner check exists to prevent.
    pub(crate) fn running_live_tails(&self) -> Vec<(u64, Arc<LiveTail>)> {
        self.lock()
            .iter()
            .filter(|(_, e)| matches!(e.state, ProcState::Running))
            .filter_map(|(id, e)| e.live.clone().map(|t| (*id, t)))
            .collect()
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

/// Persist what a job had printed, at the moment Aleph stopped it.
///
/// The two callers ([`ProcessRegistry::kill`] and
/// [`ProcessRegistry::shutdown`]) are the paths where a job ends **without
/// producing a result**: the future is aborted, so there is no
/// `CodeExecOutput`, and until this existed the recovered row could only say
/// "no output was recorded" — technically true and useless, since the job had
/// very often printed thousands of lines.
///
/// The bytes are PRE-scrub, so they go through
/// [`partial_output::durable_text`], which is the same gate `bash`'s own `poll`
/// runs. That is the invariant that makes this safe: nothing lands on disk that
/// a live poll would have refused, so recovering a row cannot become a way
/// around the refusal.
///
/// Must be called with no registry lock held — the gate is regex work over the
/// whole tail and the write touches the disk.
fn journal_live_tail(id: u64, live: Option<&LiveTail>) {
    let Some(live) = live else { return };
    let snapshot = live.snapshot();
    if let Some(text) = partial_output::durable_text(&snapshot) {
        process_journal::record_partial(id, &text);
    }
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
///
/// The ONLY journaling registry in the process — see
/// [`ProcessRegistry::journaled`]. Any other instance (i.e. every test)
/// allocates ids from 1 as well, and the journal is id-keyed, so a second
/// writer would silently overwrite this one's rows.
static REGISTRY: Lazy<Arc<ProcessRegistry>> =
    Lazy::new(|| Arc::new(ProcessRegistry::new_journaled()));

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
        // A late natural completion must NOT overwrite the Killed verdict, and
        // must SAY it changed nothing: the announce hangs off that answer, and
        // a completion that lost to a kill has no news in it.
        assert!(
            !reg.complete(id, dummy_output(0, "late")),
            "a completion that lost to a kill did not transition anything"
        );
        assert!(matches!(reg.poll(id, None), PollOutcome::Killed));
        // Second kill is a no-op.
        assert!(matches!(reg.kill(id, None), KillOutcome::AlreadyFinished));
    }

    /// `complete` reports the **effect**, not the call. The announce producer
    /// guards on this: an unknown or already-settled id must not put a
    /// completion notice on the bus.
    #[tokio::test]
    async fn complete_reports_whether_the_transition_landed() {
        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("true", None, live_handle().await));
        assert!(
            reg.complete(id, dummy_output(0, "ok")),
            "the first completion performs the Running → Done transition"
        );
        assert!(
            !reg.complete(id, dummy_output(0, "again")),
            "a second completion changes nothing"
        );
        assert!(
            !reg.complete(9_999_999, dummy_output(0, "ghost")),
            "an id the registry never issued transitions nothing"
        );
    }

    /// The announce dedup: once the model has collected a result itself, a
    /// proactive notice would spend a whole parent turn re-stating what it has
    /// already folded in. `poll` is where that is stamped, and `wait` returns
    /// through the same read.
    #[tokio::test]
    async fn a_terminal_read_marks_the_job_reported() {
        let reg = ProcessRegistry::new();
        let handle = live_handle().await;
        let id = unwrap_id(reg.register_running("cargo build", Some("s".into()), handle));

        // Mid-run polls are not collection — the result does not exist yet.
        assert!(matches!(
            reg.poll(id, Some("s")),
            PollOutcome::Running { .. }
        ));
        assert!(!reg.is_reported(id), "a running job has nothing to report");

        reg.complete(id, dummy_output(0, "done\n"));
        assert!(
            !reg.is_reported(id),
            "completing is the announce's trigger, not its dedup"
        );

        // A foreign session's poll is a NotFound and must not stamp anything —
        // otherwise one session could silence another's announce.
        assert!(matches!(
            reg.poll(id, Some("intruder")),
            PollOutcome::NotFound
        ));
        assert!(!reg.is_reported(id));

        assert!(matches!(reg.poll(id, Some("s")), PollOutcome::Done(_)));
        assert!(
            reg.is_reported(id),
            "the owner collected the result; a proactive announce is now redundant"
        );
    }

    /// `list` is a directory, not a collection: it shows status and exit code,
    /// never the output. Stamping it reported would let a routine `list` cancel
    /// the announce for a result the model has not actually read.
    #[tokio::test]
    async fn listing_a_finished_job_does_not_count_as_collecting_it() {
        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("true", Some("s".into()), live_handle().await));
        reg.complete(id, dummy_output(0, "done\n"));
        assert_eq!(reg.list(Some("s")).len(), 1);
        assert!(
            !reg.is_reported(id),
            "seeing a job in a directory is not the same as reading its output"
        );
    }

    /// `wait` returns through `poll`, so the other collecting face stamps it
    /// too — without a second call site anyone could forget.
    #[tokio::test]
    async fn a_wait_that_returns_the_output_also_marks_it_reported() {
        let reg = Arc::new(ProcessRegistry::new());
        let handle = live_handle().await;
        let id = unwrap_id(reg.register_running("echo hi", Some("w".into()), handle));
        let reg2 = reg.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            reg2.complete(id, dummy_output(0, "hi\n"));
        });
        assert!(matches!(
            reg.wait(id, Some("w"), Duration::from_secs(5)).await,
            WaitOutcome::Done(_)
        ));
        assert!(reg.is_reported(id));
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

    /// End-to-end of the partial-output surface: a running job with a live tail
    /// attached reports a tail + a byte count on `poll`, and once it completes
    /// the final output takes over AND the raw rings are released (they must not
    /// sit in the table next to an already-truncated `CodeExecOutput`).
    #[tokio::test]
    async fn attached_live_tail_surfaces_partial_then_is_released_on_complete() {
        use crate::sandbox::live_tail::LiveStream;

        let reg = ProcessRegistry::new();
        let id =
            unwrap_id(reg.register_running("cargo build", Some("s".into()), live_handle().await));

        // Before attach: running, but nothing to show (honest `None`, not an
        // empty snapshot pretending the job printed nothing).
        match reg.poll(id, Some("s")) {
            PollOutcome::Running { partial, .. } => assert!(partial.is_none()),
            _ => panic!("expected Running"),
        }

        let tail = Arc::new(LiveTail::new());
        reg.attach_live(id, tail.clone());
        tail.push(LiveStream::Stdout, b"Compiling alephcore\n");
        tail.push(LiveStream::Stderr, b"warning: unused\n");

        match reg.poll(id, Some("s")) {
            PollOutcome::Running { partial, .. } => {
                let snap = partial.expect("attached tail must surface");
                assert_eq!(
                    String::from_utf8_lossy(&snap.stdout),
                    "Compiling alephcore\n"
                );
                assert_eq!(String::from_utf8_lossy(&snap.stderr), "warning: unused\n");
                assert_eq!(snap.stdout_total, 20);
                assert_eq!(snap.stderr_total, 16);
            }
            _ => panic!("expected Running with a partial"),
        }

        assert_eq!(Arc::strong_count(&tail), 2, "test + registry hold it");
        reg.complete(id, dummy_output(0, "Finished\n"));
        match reg.poll(id, Some("s")) {
            PollOutcome::Done(out) => assert_eq!(out.stdout, "Finished\n"),
            _ => panic!("expected Done"),
        }
        assert_eq!(
            Arc::strong_count(&tail),
            1,
            "the registry must drop the live rings when the entry retires"
        );
    }

    /// A `kill` retires the entry the same way `complete` does — the rings go
    /// with it, in the same lock acquisition that flips the state.
    #[tokio::test]
    async fn kill_and_shutdown_release_the_live_tail() {
        let reg = ProcessRegistry::new();
        let killed_id = unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        let killed_tail = Arc::new(LiveTail::new());
        reg.attach_live(killed_id, killed_tail.clone());
        assert!(matches!(reg.kill(killed_id, None), KillOutcome::Killed));
        assert_eq!(
            Arc::strong_count(&killed_tail),
            1,
            "kill releases the rings"
        );

        let shut_id = unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        let shut_tail = Arc::new(LiveTail::new());
        reg.attach_live(shut_id, shut_tail.clone());
        assert_eq!(reg.shutdown(), 1);
        assert_eq!(
            Arc::strong_count(&shut_tail),
            1,
            "shutdown releases the rings"
        );
    }

    /// Attaching after the entry already retired must not resurrect a partial
    /// view onto a job that is over.
    #[tokio::test]
    async fn attach_after_retirement_is_a_noop() {
        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("true", None, live_handle().await));
        reg.complete(id, dummy_output(0, "done"));
        let tail = Arc::new(LiveTail::new());
        reg.attach_live(id, tail.clone());
        assert_eq!(Arc::strong_count(&tail), 1, "no slot to attach to");
    }

    /// `wait` timing out mid-job must carry the same partial view `poll` does —
    /// otherwise "block for 60s then learn nothing" is still the experience.
    #[tokio::test]
    async fn wait_timeout_carries_the_partial_tail() {
        use crate::sandbox::live_tail::LiveStream;

        let reg = ProcessRegistry::new();
        let id = unwrap_id(reg.register_running("sleep 9", Some("w".into()), live_handle().await));
        let tail = Arc::new(LiveTail::new());
        reg.attach_live(id, tail.clone());
        tail.push(LiveStream::Stdout, b"step 1 of 9\n");

        match reg.wait(id, Some("w"), Duration::from_millis(30)).await {
            WaitOutcome::TimedOut { partial, .. } => {
                let snap = partial.expect("live tail must ride along on timeout");
                assert_eq!(String::from_utf8_lossy(&snap.stdout), "step 1 of 9\n");
            }
            _ => panic!("expected TimedOut"),
        }
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

    /// THE id trap: `next_id` starts at 1 in every daemon, so without a durable
    /// high-water mark a journal row `#3` left by the previous process and a
    /// live job `#3` are the same address for the same caller. Two simulated
    /// boots, each with its own registry seeded exactly the way
    /// `process_journal::init_and_reconcile` seeds the real one.
    #[tokio::test]
    async fn a_second_boot_never_re_issues_an_id_the_first_boot_issued() {
        let _g = process_journal::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        let owner = Some("boot-owner".to_string());

        async fn boot(dir: &std::path::Path, owner: Option<String>) -> Vec<u64> {
            process_journal::init_and_reconcile(dir.to_path_buf());
            // `new_journaled`, not `new`: this test is standing in for the
            // daemon's singleton, and only that instance writes rows.
            let reg = ProcessRegistry::new_journaled();
            reg.seed_id_floor(process_journal::id_floor());
            let mut ids = Vec::new();
            for i in 0..3 {
                ids.push(unwrap_id(reg.register_running(
                    format!("job {i}"),
                    owner.clone(),
                    live_handle().await,
                )));
            }
            ids
        }

        let first = boot(tmp.path(), owner.clone()).await;
        // Simulate a SIGKILL: nothing is flushed, the process simply vanishes.
        process_journal::disable_for_test();
        let second = boot(tmp.path(), owner.clone()).await;

        let highest_first = first.iter().copied().max().expect("ids were issued");
        assert!(
            second.iter().all(|id| *id > highest_first),
            "boot 2 re-issued an id boot 1 handed out: first={first:?} second={second:?}"
        );
        // ...and boot 1's rows are all still addressable, which is the reason
        // the collision would have mattered in the first place.
        for id in first {
            assert!(
                process_journal::lookup(id, owner.as_deref()).is_some(),
                "row #{id} must still answer after the restart"
            );
        }
        process_journal::disable_for_test();
    }

    /// The other half of the id trap, and the one that actually bit: a
    /// non-singleton registry must not reach the durable store at all.
    ///
    /// Every `ProcessRegistry` allocates from 1 and the journal is id-keyed,
    /// so a second writer does not append — it OVERWRITES row `#1` with a row
    /// carrying its own owner, and the real owner's `lookup(1)` then answers
    /// "no such job". In production there is exactly one registry so the
    /// question never arises; the test suite builds dozens, in parallel, and
    /// that is how the bug surfaced: green when run alone, red in the full
    /// run. Each writer's own test passes in isolation, and the interleaving
    /// is the only thing that fails — so the isolated tests were the ones
    /// telling the comfortable lie.
    #[tokio::test]
    async fn a_non_singleton_registry_never_writes_to_the_journal() {
        let _g = process_journal::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        process_journal::init_and_reconcile(tmp.path().to_path_buf());

        // A plain `new()` registry — what every other test in this module
        // builds — spawns and settles a job while the journal is ENABLED.
        let reg = ProcessRegistry::new();
        let id = unwrap_id(
            reg.register_running("cat /etc/shadow", Some("squatter".into()), {
                live_handle().await
            }),
        );
        reg.complete(id, dummy_output(0, "secrets\n"));

        assert!(
            process_journal::lookup(id, Some("squatter")).is_none(),
            "a non-journaled registry wrote row #{id} into the process-global store; \
             ids restart at 1 in every registry, so this is how one test clobbers another"
        );
        process_journal::disable_for_test();
    }

    #[tokio::test]
    async fn shutdown_is_idempotent() {
        let reg = ProcessRegistry::new();
        unwrap_id(reg.register_running("sleep 9", None, live_handle().await));
        assert_eq!(reg.shutdown(), 1);
        // Second call: nothing running, no abort, returns 0.
        assert_eq!(reg.shutdown(), 0);
    }

    /// Attach a live tail carrying `text` on stdout to a running entry.
    fn attach_tail(reg: &ProcessRegistry, id: u64, text: &str) {
        use crate::sandbox::live_tail::LiveStream;
        let tail = Arc::new(LiveTail::new());
        tail.push(LiveStream::Stdout, text.as_bytes());
        reg.attach_live(id, tail);
    }

    /// A killed job must keep what it had printed.
    ///
    /// Aborting drops the in-flight future, so there is no `CodeExecOutput` and
    /// never will be — the live tail is the ONLY record that ever existed, and
    /// `kill` is the last instant it exists at all (`retire` releases the rings
    /// under the same lock). Before this the recovered row said "no output was
    /// recorded", which is technically true and useless for a build that had
    /// printed thousands of lines.
    ///
    /// Asserts on the re-read through `lookup`, not on a call count: capturing
    /// the snapshot and then dropping it on the floor would satisfy any
    /// assertion about the capture *happening*.
    #[tokio::test]
    async fn killing_a_job_records_the_tail_it_had_printed() {
        let _g = process_journal::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        process_journal::init_and_reconcile(tmp.path().to_path_buf());
        let reg = ProcessRegistry::new_journaled();
        reg.seed_id_floor(process_journal::id_floor());
        let owner = Some("kill-owner".to_string());

        let id = unwrap_id(reg.register_running("cargo build", owner.clone(), live_handle().await));
        attach_tail(&reg, id, "   Compiling alephcore v26.7.31\n");
        assert!(matches!(
            reg.kill(id, owner.as_deref()),
            KillOutcome::Killed
        ));

        let found = process_journal::lookup(id, owner.as_deref()).expect("the row is journaled");
        assert_eq!(found.record.outcome.as_deref(), Some("killed"));
        assert!(
            found.recorded_output.contains("Compiling alephcore"),
            "a killed job's last output is the only thing it leaves: {:?}",
            found.recorded_output
        );
        assert!(
            found.output_is_live_capture,
            "and it must be labelled a window, not a result"
        );
        process_journal::disable_for_test();
    }

    /// Same capture on the orderly-shutdown path. Not redundant with the `kill`
    /// test: these are two different call sites, and the reaper's is the one an
    /// operator hits by stopping the daemon mid-build.
    #[tokio::test]
    async fn shutdown_records_the_tail_of_every_job_it_reaps() {
        let _g = process_journal::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        process_journal::init_and_reconcile(tmp.path().to_path_buf());
        let reg = ProcessRegistry::new_journaled();
        reg.seed_id_floor(process_journal::id_floor());
        let owner = Some("reaped-owner".to_string());

        let id = unwrap_id(reg.register_running("make -j8", owner.clone(), live_handle().await));
        attach_tail(&reg, id, "[ 87%] Building CXX object\n");
        assert_eq!(reg.shutdown(), 1);

        let found = process_journal::lookup(id, owner.as_deref()).expect("row");
        assert!(
            found.recorded_output.contains("87%"),
            "stopping the daemon must not erase how far the build got: {:?}",
            found.recorded_output
        );
        process_journal::disable_for_test();
    }

    /// The interrupted population's only mechanism: a crash runs nothing, so
    /// the answer has to already be on disk. One flusher tick, driven directly.
    ///
    /// Also pins the idempotence the tick relies on — an unchanged job must not
    /// be rewritten, because the loop runs forever and a `fsync` per idle job
    /// per interval is a cost with no reader.
    #[tokio::test]
    async fn a_flusher_tick_persists_running_output_and_skips_unchanged_jobs() {
        use crate::sandbox::live_tail::LiveStream;
        let _g = process_journal::test_gate();
        let tmp = tempfile::tempdir().unwrap();
        process_journal::init_and_reconcile(tmp.path().to_path_buf());
        let reg = ProcessRegistry::new_journaled();
        reg.seed_id_floor(process_journal::id_floor());
        let owner = Some("flush-owner".to_string());

        let id = unwrap_id(reg.register_running("pytest -x", owner.clone(), live_handle().await));
        let tail = Arc::new(LiveTail::new());
        tail.push(LiveStream::Stdout, b"collected 812 items\n");
        reg.attach_live(id, tail.clone());

        let mut written = std::collections::HashMap::new();
        assert_eq!(
            process_journal::flush_running_partials_for_test(&reg, &mut written),
            1,
            "a running job with new bytes must be flushed"
        );
        let found = process_journal::lookup(id, owner.as_deref()).expect("row");
        assert!(
            found.recorded_output.contains("collected 812 items"),
            "got: {:?}",
            found.recorded_output
        );
        assert!(found.output_is_live_capture);
        assert_eq!(
            found.record.phase,
            process_journal::JobPhase::Running,
            "flushing must not settle the job — it is still going"
        );

        // Nothing new: the file already says exactly this.
        assert_eq!(
            process_journal::flush_running_partials_for_test(&reg, &mut written),
            0,
            "an idle job must not be rewritten every tick"
        );
        // ...and new bytes bring it back.
        tail.push(LiveStream::Stdout, b"812 passed\n");
        assert_eq!(
            process_journal::flush_running_partials_for_test(&reg, &mut written),
            1
        );
        assert!(process_journal::lookup(id, owner.as_deref())
            .expect("row")
            .recorded_output
            .contains("812 passed"));

        // A finished job leaves the running set, and its bookkeeping with it.
        reg.complete(id, dummy_output(0, "812 passed\n"));
        assert_eq!(
            process_journal::flush_running_partials_for_test(&reg, &mut written),
            0
        );
        assert!(
            written.is_empty(),
            "the per-job cursor must be pruned, or a long-lived daemon accumulates \
             one entry per job it has ever run"
        );
        process_journal::disable_for_test();
    }
}
