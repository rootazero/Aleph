//! Cross-process execution journal for background `bash` jobs.
//!
//! [`ProcessRegistry`](super::process_registry::ProcessRegistry) is pure
//! process memory: a `HashMap` behind a `Lazy`, nothing on disk. A daemon
//! restart therefore erased every background job, and the next
//! `{"process_action":"poll","process_id":3}` got
//! `"bash: no background process #3 for this session"` — i.e. *"there was never
//! such a thing"*, when the truth is *"it belonged to a daemon that no longer
//! exists, and here is what was recorded about it"*. A purely in-memory
//! registry does not go **empty** across a restart, it **lies**, and the
//! caller's reasonable response to "never existed" is to redo the work.
//!
//! This module is the sidecar that closes that gap. Its shape is deliberately
//! copied from [`crate::agents::background_persistence`], which already solves
//! the identical problem for background sub-agents, rather than invented:
//!
//! * layout `<dir>/job-<id>/state.json` (atomically rewritten via
//!   [`crate::utils::atomic_io::write_atomic`] three times over a job's own
//!   life — intent at spawn, pid once the driver has the child, terminal —
//!   plus the boot passes that tombstone or stamp it) and an append-only
//!   `output.txt` trail;
//! * a three-value [`JobPhase`] whose crash verdict is deliberately **not**
//!   "failed";
//! * [`init_and_reconcile`] overwrites every `Running` row with a terminal
//!   **tombstone** instead of deleting it: a mechanism that only records "it
//!   finished" cannot tell "it never ran" from "it ran and the write was lost";
//! * unconditional [`SecretMasker`] on every byte, because the reader is a
//!   *later process* — redaction cannot be gated on the writing run's
//!   attendedness;
//! * a 7-day retention gate swept **at boot only** (per-write would turn every
//!   spawn into an O(jobs) stat storm);
//! * persistence is **opt-in**: until [`init_and_reconcile`] runs, every entry
//!   point here is a zero-I/O no-op, so tests, the CLI and every non-daemon
//!   embedding behave exactly as they did before.
//!
//! ## What does NOT transfer from the sub-agent sidecar
//!
//! **1. There is a pid, so there is a liveness probe — and it never kills.**
//! `background_persistence` may assert "every `Running` record at boot is an
//! orphan" because its runs are in-process `tokio` tasks: if the process is
//! gone, the run is gone. A background `bash` job is a **real OS process**. It
//! is spawned with `kill_on_drop(true)`, so an orderly teardown reaps it — but
//! a `SIGKILL`ed daemon never drops anything, and the child can outlive it.
//! So [`record_child`] stamps the row with the child's pid and creation time
//! once the driver has the child (the third write), and at boot
//! [`probe_liveness`] asks the OS whether that exact process — same pid, same
//! creation time — still exists. The answer lands on the
//! [`JobPhase::Interrupted`] row as one of three arms:
//! [`Tombstone::ExitedDuringRestart`], [`Tombstone::StillRunningUnattached`]
//! (the orphan is **recorded, never signalled or reaped** — Aleph holds no
//! handle to it and does not take one), or no tombstone at all, which keeps
//! the `interrupted_by_restart_liveness_unknown` wording for a row that never
//! got a pid or whose probe could not answer — an unknown is never spelled as
//! an exit. It is never reported as a failure — nothing about the command was
//! judged.
//!
//! **2. Newlines are preserved.** `background_persistence::mask_line` collapses
//! newlines because it stores single-line progress notes. A stdout trail must
//! not be flattened, so [`append_block`] masks **per line** and writes one
//! trail line per output line (keeping the 4000-char per-line cap), and
//! [`read_trail`] rebuilds the line structure on the way back out.
//!
//! ## What is (and is not) in the trail
//!
//! **Two files, two provenances, and they are never mixed.**
//!
//! * `output.txt` is the append-only trail of a job that reached a **natural
//!   completion**: the `CodeExecOutput` it produced has already been through
//!   the sandbox's `scrub_and_gate_output`, so it is safe to persist verbatim.
//! * `partial.txt` is a **rewritten-in-place** capture of the job's live tail,
//!   for the population `output.txt` structurally cannot serve — a job that was
//!   killed, or one whose daemon died under it. It is written by
//!   [`record_partial`], and only ever with text that has cleared
//!   [`crate::builtin_tools::partial_output::gate`], i.e. the exact floor
//!   `bash`'s own `poll` enforces. That is what makes persisting a PRE-scrub
//!   ring safe: **nothing reaches this directory that a live `poll` would have
//!   refused**, so "restart the daemon" is not a way around the poll refusal.
//!
//! The split is not fastidiousness. Both halves describe the same job, and an
//! append-only file fed by two producers double-counts — the finished-path
//! bytes would land on top of the live windows that already carried them. A
//! rewritten file has no such failure mode, and costs nothing the reader would
//! have seen anyway: [`read_trail`] only ever returns the last
//! [`OUTPUT_TAIL_BYTES`], which is exactly what the live ring holds.
//!
//! [`read_trail`] prefers `output.txt` and falls back to `partial.txt`, marking
//! the fallback so the renderer can say "this is a mid-run snapshot, not a
//! result". A row with neither still says so explicitly rather than showing an
//! empty string.
//!
//! ## Id collisions
//!
//! `ProcessRegistry::next_id` starts at 1 in every daemon, so a resurrected row
//! `#3` and a freshly-spawned job `#3` would be the same address for the same
//! caller. The fix is a monotonic high-water allocator, not a uuid (the model
//! re-types this id in every poll) and not a display-id/durable-key pair (two
//! ids for one thing hides the collision instead of solving it):
//! [`reserve_id`] persists a whole block of ids **before** the registry hands
//! one out, and [`init_and_reconcile`] seeds the registry's allocator above
//! every id the journal has ever reserved. Allocating N and *then* persisting
//! the mark would reuse N after a crash in between — the same bug one layer
//! down.
//!
//! R10 note: this is scaffolding, not cognition. It answers the mechanical
//! question "did this process_id exist in a previous daemon, and what was
//! recorded about it?". It makes no judgement about the work.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::exec::masker::SecretMasker;
use crate::sync_primitives::{Mutex, MutexGuard};

/// State file name inside a job's directory.
const STATE_FILE: &str = "state.json";
/// Append-only output trail file name inside a job's directory.
const OUTPUT_FILE: &str = "output.txt";
/// Rewritten-in-place live-tail capture inside a job's directory. See the
/// module docs for why this is a second file and not more lines in
/// [`OUTPUT_FILE`].
const PARTIAL_FILE: &str = "partial.txt";
/// Id high-water mark, at the store root (not inside a job directory).
const ID_WATERMARK_FILE: &str = "id_watermark.json";

/// How often the background flusher rewrites each running job's live-tail
/// capture.
///
/// This is the resolution of the answer a crashed daemon leaves behind: a job
/// killed by `SIGKILL` 14s after its last flush loses those 14s of output.
/// Tightening it buys sharper crash forensics and costs one small atomic write
/// per running job per tick — and only for jobs whose byte counters actually
/// moved, so an idle job costs a `snapshot()` and nothing else.
const PARTIAL_FLUSH_INTERVAL: Duration = Duration::from_secs(15);

/// How long a terminal row is kept on disk. Pruned at boot only — the sweep
/// walks the whole directory, so doing it per-write would turn every spawn
/// into an O(jobs) stat storm. Same window as the sub-agent sidecar.
const RECORD_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Bytes of output trail retained per job when reading it back. Matches the
/// live tail's budget so "poll a running job" and "poll a job the previous
/// daemon ran" hand back the same amount of text.
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// How recently a settled-but-unannounced job must have ended for the boot
/// handback to still deliver its notice.
///
/// The notice opens a real model turn, so its value decays: "your build
/// finished" is worth a turn minutes later and is noise a day later, when the
/// row is still readable through `poll` / `list` anyway. The window also bounds
/// the one-off cost of [`JobRecord::announced_boot`] being `#[serde(default)]`
/// — without it, the first boot after that field shipped would announce every
/// completed job inside the whole 7-day retention window. Rows older than this
/// are left **unstamped and uncounted**: claiming they were announced would be
/// a lie, spending an attempt on a notice nobody sent would be another, and the
/// age test only ever gets truer, so they cannot come round again.
const ANNOUNCE_HANDBACK_MAX_AGE_MS: u64 = 60 * 60 * 1000;

/// How many boots may try to hand one completion to its owner before the row
/// stops asking.
///
/// The freshness window above bounds *how long* the retry may go on; this
/// bounds *how often* inside that window. Both are needed: a daemon that
/// crash-loops inside the hour would otherwise re-queue the same notice on
/// every boot, and the row cannot tell a delivery that failed from one whose
/// broadcast landed on a session that had already gone away. Three is the same
/// shape as `process_announce`'s 0/30/120s ladder — try, try again, then stop
/// claiming. Giving up on the proactive notice is not giving up on the answer:
/// the row stays readable through `poll` / `list` for its whole retention.
const MAX_ANNOUNCE_ATTEMPTS: u8 = 3;

/// Hard cap on one appended trail line.
const MAX_LINE_CHARS: usize = 4_000;

/// How many ids one durable reservation covers. Bigger = fewer writes but a
/// bigger id jump across a restart; 64 matches the registry's `MAX_ENTRIES`, so
/// a daemon that never exceeds its own table size pays exactly one watermark
/// write per boot.
const ID_RESERVATION_BLOCK: u64 = 64;

/// Shared masker. `SecretMasker` is a zero-sized handle — both the vendor floor
/// and the operator's `[[security.mask_patterns]]` live in process-wide statics
/// inside `exec::masker` — so this exists only to avoid re-constructing the
/// empty wrapper per line, and it inherits configured patterns without knowing
/// they exist.
static MASKER: LazyLock<SecretMasker> = LazyLock::new(SecretMasker::new);

/// Root directory for the journal. `None` = persistence disabled (every entry
/// point is a no-op), which is the state in CLI processes, tests, and any
/// embedding that never calls [`init_and_reconcile`].
static STORE_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// Rows visible to [`lookup`] / [`list_for_scope`]: everything loaded from disk
/// at boot, plus every job started in this process. Terminal rows stay here for
/// as long as their tombstone survives on disk, so an id keeps answering for
/// exactly as long as its record exists.
static INDEX: LazyLock<Mutex<HashMap<u64, JobRecord>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Highest id durably reserved. `0` = nothing reserved (also the value while
/// persistence is off, which makes [`id_floor`] answer `1` — the pre-existing
/// allocator start).
static RESERVED_THROUGH: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(0));

/// Rows [`init_and_reconcile`] found settled-but-unannounced, waiting for
/// [`take_undelivered_settled`] to drain them.
///
/// A stash rather than a return value because `init_and_reconcile` is sync and
/// is called directly by tests and by any embedding that wants durability
/// without a bus, while the handback needs an async broadcast. Draining is
/// destructive, so the rows can only be delivered once per boot however many
/// callers there are.
static UNDELIVERED: LazyLock<Mutex<Vec<JobRecord>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Lifecycle phase of a journaled background job.
///
/// Deliberately **separate** from `ProcessRegistry`'s in-memory `ProcState`:
/// `Killed` there is a verdict Aleph earned by calling `abort()`, while a
/// `Running` row found on disk at boot earned nothing at all. Collapsing the
/// two vocabularies is how a restart starts reading as a decision somebody made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    /// Registered and (as far as the writing process knew) still executing.
    Running,
    /// Reached a terminal state in the daemon that started it — see `outcome`.
    Settled,
    /// Found `Running` on disk with no daemon behind it. A statement about the
    /// **previous process**, never about the command: see [`init_and_reconcile`].
    /// What became of the OS process is a separate fact, [`JobRecord::tombstone`].
    Interrupted,
}

/// Which driver owns a row. Pre-existing rows carry no `kind` and decode as
/// `Bash`, which is what every row was before the field existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    /// A background `bash` job, addressed by its registry id (`job-<id>`).
    #[default]
    Bash,
    /// A PTY session, addressed by its uuid ([`JobRecord::pty_session_id`]).
    Pty,
}

/// What boot learned about a `Running` row's OS process. `None` on an
/// `Interrupted` row = no pid, or the probe could not answer: today's
/// "liveness unknown" wording stays as the third arm (criterion #8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tombstone {
    /// No process with that pid and creation time exists any more.
    ExitedDuringRestart,
    /// The process is still there and Aleph holds no handle to it. **Recorded
    /// only**: nothing in this module signals, kills or reaps it — the pid is
    /// carried so a later face can *tell* the owner, and that is all.
    StillRunningUnattached { pid: u32 },
}

/// One answer from [`probe_liveness`]. `Unknown` is a first-class value, not a
/// fallback: it is what the instrument says when it cannot say `Exited`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Exited,
    StillRunning,
    Unknown,
}

/// Wire label handed to the model for one journal row.
///
/// Takes the **record**, not the phase, for the same reason
/// [`crate::agents::background_persistence::settled_label`] does — the twin
/// question, answered in the same shape with this module's own words.
/// `Settled` alone names no outcome: it says the job reached a terminal state
/// in the daemon that owned it, and that state is either a completion or a
/// `kill` Aleph performed. The phase-only label answered `"recorded"` for both,
/// so the one word the model reads first could not tell "your build finished"
/// from "you stopped it half way", and the `outcome` key that could was
/// optional and easy to miss beside it.
///
/// An unrecognised or absent outcome is `settled_unknown`, never a success
/// word: a label the model reads as "it finished" has to be earned by a
/// producer that wrote one. The words come from [`Verdict::label`] rather than
/// from literals here, so the writer's vocabulary and the reader's cannot drift.
///
/// `Interrupted` says more than the sub-agent sidecar's `interrupted_by_restart`
/// on purpose: a `bash` child is a real OS process that can outlive a
/// `SIGKILL`ed daemon, so the label reads the [`Tombstone`] the boot probe
/// wrote — `exited_during_restart` / `still_running_unattached` — and falls
/// back to `interrupted_by_restart_liveness_unknown` when there is none: no
/// pid was ever recorded, or the probe could not answer. An unknown is never
/// spelled as an exit. None of the non-terminal labels reads as a failure.
#[must_use]
pub(crate) fn settled_label(record: &JobRecord) -> &'static str {
    match record.phase {
        JobPhase::Running => "running_unconfirmed",
        JobPhase::Interrupted => match record.tombstone {
            Some(Tombstone::ExitedDuringRestart) => "exited_during_restart",
            Some(Tombstone::StillRunningUnattached { .. }) => "still_running_unattached",
            None => "interrupted_by_restart_liveness_unknown",
        },
        JobPhase::Settled => match record.outcome.as_deref() {
            Some(o) if o == Verdict::Completed.label() => "completed",
            Some(o) if o == Verdict::Killed.label() => "killed",
            _ => "settled_unknown",
        },
    }
}

/// How a job left the registry, for a [`JobPhase::Settled`] row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The task ran to completion and produced a `CodeExecOutput`.
    Completed,
    /// Aleph aborted it — `process_action: "kill"` or daemon shutdown.
    Killed,
}

impl Verdict {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Killed => "killed",
        }
    }
}

/// One background `bash` job as recorded on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: u64,
    /// Owning session label — the exact string `bash_exec::session_label()`
    /// renders, so an id stays addressable by the same caller after a restart.
    /// Never empty: [`record_spawn`] refuses to journal an unowned job, because
    /// a persisted row with no owner is readable by every later caller (the
    /// fail-open leak `background_persistence::addressable` was already fixed
    /// for).
    pub owner: String,
    /// Masked, truncated command preview — the same text `list` shows.
    pub command: String,
    pub started_ms: u64,
    pub phase: JobPhase,
    /// Unix ms at which the row reached `Settled` / `Interrupted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// [`Verdict::label`] for a `Settled` row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Exit code, for a job that completed naturally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// File name (NOT a path) of the output trail inside this job's directory.
    /// Stored as a name so the row stays valid when `ALEPH_HOME` moves; resolve
    /// it with [`JobRecord::output_path`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
    /// File name (NOT a path) of the live-tail capture. `None` on rows written
    /// before this file existed, which is why it is `#[serde(default)]`: an old
    /// row must keep loading, it simply has no capture to offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_file: Option<String>,
    /// How many boots have *tried* to hand this completion to its owner.
    ///
    /// `phase` answers "did it finish"; [`Self::announced_boot`] answers "does
    /// anyone know"; this answers "how often have we asked". The gap between
    /// the first two is a real window: `bash_exec` broadcasts the completion
    /// after the row is settled, and `gateway::process_announce` retries at
    /// 0/30/120s while the session is busy. A daemon that dies inside those two
    /// and a half minutes leaves a `Settled` row nobody was told about, and the
    /// promise the spawn receipt makes is withdrawn in silence with the result
    /// sitting on disk. [`take_undelivered_settled`] is the reader.
    ///
    /// A **count of attempts, not a receipt** — the same split the sub-agent
    /// sidecar's `announce_attempts` makes, for the same reason: only the
    /// delivery may say a notice landed, so everything before it can record no
    /// more than that it was tried. Counting is also what bounds the retry: a
    /// row whose owning session no longer resolves would otherwise be re-queued
    /// at every boot for as long as it stays fresh.
    ///
    /// `#[serde(default)]` reads every pre-existing row as zero attempts /
    /// nobody-told, which is the fail-safe direction (duplicate-visible beats
    /// loss-silent); the boot handback's freshness window keeps that from
    /// turning an upgrade into a week of stale notices.
    #[serde(default)]
    pub announce_attempts: u8,
    /// The boot that actually delivered this completion, as that boot's
    /// wall-clock ms. `None` means nobody has been told yet.
    ///
    /// Written only by [`record_announced`], and only after the broadcast it
    /// describes has returned — never in advance of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announced_boot: Option<u64>,
    /// Which driver owns the row. `#[serde(default)]` = `Bash`, so every row
    /// written before the field existed keeps decoding as what it was.
    #[serde(default)]
    pub kind: JournalKind,
    /// The PTY session's uuid, for a [`JournalKind::Pty`] row. `None` on every
    /// Bash row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_session_id: Option<String>,
    /// OS pid of the child, written by [`record_child`] once the driver has
    /// it. `None` until then — and forever on a row whose daemon died in the
    /// gap between the intent write and the child's arrival, which is why the
    /// boot probe can only run on rows that carry one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Unix ms at which the OS says that pid's process was created, read
    /// through the same routine [`probe_liveness`] compares against. The
    /// anti-pid-reuse signal: a recycled pid points at a different process
    /// with a different creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_created_at_ms: Option<u64>,
    /// What boot learned about the OS process of an `Interrupted` row. `None`
    /// = no pid, or the probe could not answer — the liveness-unknown wording,
    /// never a verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone: Option<Tombstone>,
}

impl JobRecord {
    /// Absolute path of this job's output trail, given the journal root.
    #[must_use]
    pub fn output_path(&self, dir: &Path) -> Option<PathBuf> {
        let file = self.output_file.as_ref()?;
        Some(job_dir(dir, self.id).join(file))
    }

    /// Absolute path of this job's live-tail capture, given the journal root.
    #[must_use]
    pub fn partial_path(&self, dir: &Path) -> Option<PathBuf> {
        let file = self.partial_file.as_ref()?;
        Some(job_dir(dir, self.id).join(file))
    }
}

/// A journal row plus whatever output was recorded for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredJob {
    pub record: JobRecord,
    /// Tail of the (already-masked) recorded output. Empty when nothing was
    /// recorded — which the renderer must state, not paper over.
    pub recorded_output: String,
    /// True when [`recorded_output`](Self::recorded_output) came from the live
    /// tail rather than from a completed run.
    ///
    /// The distinction is the renderer's whole job here: a finished-path trail
    /// is *the result*, while a live capture is a window that ends wherever the
    /// last flush landed and may be missing the job's opening entirely.
    /// Presenting the second as the first is how a model concludes a build
    /// succeeded because the last line it can see is not an error.
    pub output_is_live_capture: bool,
    /// Unix ms of the last recorded trail line or live capture, or
    /// `started_ms` when there was neither.
    pub last_activity_ms: u64,
}

// ============================================================================
// Boot
// ============================================================================

/// Enable the journal at `dir`, reconcile whatever a previous daemon left
/// behind, and seed the registry's id allocator above every id ever reserved.
/// Returns the number of rows tombstoned by this call.
///
/// The live set is empty by construction: this runs at boot, before any `bash`
/// job can be spawned, so *every* `Running` row on disk belonged to a daemon
/// that is gone. Each one gets a terminal [`JobPhase::Interrupted`] state
/// written over it (never deleted): a record that only ever says "finished"
/// cannot distinguish "never ran" from "ran and the write was lost". A row
/// that carries a pid is also asked of the OS through [`probe_liveness`], and
/// the answer — exited, still running, or no answer — rides along as its
/// [`Tombstone`]; a still-running orphan is re-asked on every later boot and
/// rewritten only by a definite exit.
///
/// This function itself **broadcasts nothing** — an interrupted row drives no
/// proactive turn, it is simply there the next time the model polls, so this
/// half has no ordering dependency on any event subscriber. It does claim the
/// second recovered population on the way past: a row that reached `Settled`
/// with no `announced_boot` is a completion whose notice died with the
/// previous daemon, and [`take_undelivered_settled`] hands those to
/// [`init_and_announce`], which is the half that does have an ordering
/// dependency.
///
/// Idempotent. Returns 0 when the directory cannot be created — persistence
/// stays off rather than failing boot (P7).
pub fn init_and_reconcile(dir: PathBuf) -> usize {
    reconcile_with(dir, &probe_liveness)
}

/// Test-only: [`init_and_reconcile`] with the liveness probe scripted, so a
/// test can boot against "exited" / "still running" / "unknown" without owning
/// a real orphan. The production entry stays the one above — this is the same
/// body with one argument swapped, never a second reconcile path.
#[cfg(test)]
pub(crate) fn init_and_reconcile_with_probe(
    dir: PathBuf,
    probe: &dyn Fn(u32, Option<u64>) -> Liveness,
) -> usize {
    reconcile_with(dir, probe)
}

/// What an `Interrupted` row's tombstone should say, given what the probe
/// answers for its pid. `None` twice over: a row with no pid has nothing to
/// ask about, and an `Unknown` answer is not allowed to become a verdict —
/// `Unknown` and `Exited` look identical to a caller who only checks "is the
/// pid gone", which is exactly why the probe distinguishes them (criterion #8).
fn tombstone_for(
    record: &JobRecord,
    probe: &dyn Fn(u32, Option<u64>) -> Liveness,
) -> Option<Tombstone> {
    let pid = record.pid?;
    match probe(pid, record.process_created_at_ms) {
        Liveness::Exited => Some(Tombstone::ExitedDuringRestart),
        Liveness::StillRunning => Some(Tombstone::StillRunningUnattached { pid }),
        Liveness::Unknown => None,
    }
}

/// sysinfo reports start times in whole seconds on every platform, so a
/// creation time recorded at spawn and one read at boot can differ by the
/// rounding and nothing else.
const CREATION_TIME_TOLERANCE_MS: u64 = 2_000;

/// Pure boundary: one sysinfo refresh of `pid`. `Unknown` whenever the
/// instrument cannot answer — it cannot see THIS process (so it cannot be
/// trusted about any other), the pid is present with no creation time to
/// compare, or sysinfo reports a start time of 0 (its "could not open the
/// process" value on Windows). `Exited` only when no process has the pid, it
/// is a zombie, or it started at another time — a recycled pid is a different
/// process. Reads the creation time through the same refresh
/// [`crate::utils::process_alive::process_start_time`] uses, so the value
/// [`record_child`] stored and the value compared here are one derivation.
///
/// Never signals: the answer is recorded on the row, and the orphan is left
/// exactly as it was found.
#[must_use]
pub fn probe_liveness(pid: u32, process_created_at_ms: Option<u64>) -> Liveness {
    use crate::utils::process_alive::{default_refresh_kind, with_process_specifics};
    use sysinfo::ProcessStatus::{Dead, Zombie};
    let facts = |p: &sysinfo::Process| (p.start_time(), p.status());
    if with_process_specifics(std::process::id(), default_refresh_kind(), facts).is_none() {
        return Liveness::Unknown;
    }
    match (
        with_process_specifics(pid, default_refresh_kind(), facts),
        process_created_at_ms,
    ) {
        (None, _) | (Some((_, Zombie | Dead)), _) => Liveness::Exited,
        (Some((0, _)), _) | (Some(_), None) => Liveness::Unknown,
        (Some((start_s, _)), Some(created))
            if (start_s.saturating_mul(1000)).abs_diff(created) <= CREATION_TIME_TOLERANCE_MS =>
        {
            Liveness::StillRunning
        }
        (Some(_), Some(_)) => Liveness::Exited,
    }
}

/// The body of [`init_and_reconcile`], with the liveness probe injected.
fn reconcile_with(dir: PathBuf, probe: &dyn Fn(u32, Option<u64>) -> Liveness) -> usize {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "process_journal: disabled (cannot create store dir)");
        return 0;
    }

    let now = now_ms();
    let mut index: HashMap<u64, JobRecord> = HashMap::new();
    let mut tombstoned = 0usize;
    let mut undelivered: Vec<JobRecord> = Vec::new();
    // Every id the journal has ever *shown*, retention-swept rows included:
    // their ids were handed out, so they must not come round again.
    //
    // Seeded from the DIRECTORY NAMES, before any row is parsed. A `state.json`
    // that will not deserialize is kept on disk (see `read_all`) but yields no
    // record — so a floor derived only from parsed rows steps straight over
    // that id, and the next daemon reissues it onto a directory that is still
    // there. `job-<id>` is written by `job_dir` from a `u64`, so the name is
    // the id whether or not the file inside it is readable.
    let mut highest_row_id = highest_dir_id(&dir);

    for record in read_all(&dir) {
        highest_row_id = highest_row_id.max(record.id);

        // Retention sweep. Terminal rows outlive their job so a restart can
        // still answer for them; they do not outlive the retention window.
        if record.phase != JobPhase::Running
            && record
                .ended_ms
                .is_some_and(|t| now.saturating_sub(t) > RECORD_RETENTION_MS)
        {
            let _ = std::fs::remove_dir_all(job_dir(&dir, record.id));
            continue;
        }

        if record.phase == JobPhase::Running {
            let tombstone = JobRecord {
                phase: JobPhase::Interrupted,
                ended_ms: Some(now),
                tombstone: tombstone_for(&record, probe),
                ..record
            };
            write_state(&dir, &tombstone);
            tombstoned += 1;
            index.insert(tombstone.id, tombstone);
        } else if matches!(
            record.tombstone,
            Some(Tombstone::StillRunningUnattached { .. })
        ) && tombstone_for(&record, probe) == Some(Tombstone::ExitedDuringRestart)
        {
            // Re-ask an orphan that outlived one restart. Only a definite
            // `Exited` rewrites; `Unknown` keeps the previous answer, and
            // `StillRunning` is the previous answer. `ended_ms` is NOT
            // re-stamped — it dates the restart that orphaned it, and the
            // re-ask is not a second orphaning.
            let record = JobRecord {
                tombstone: Some(Tombstone::ExitedDuringRestart),
                ..record
            };
            write_state(&dir, &record);
            index.insert(record.id, record);
        } else if is_undelivered_completion(&record, now) {
            // BT-D-R4-16: the delivery stamp belongs to `record_announced`,
            // called by `init_and_announce` AFTER the broadcast returns — never
            // to this pass. Stamping here marked a notice delivered that a
            // crash in the gap then never sent: silent loss. What this pass may
            // record is that it *tried*, which is what bounds the retry.
            if record.announce_attempts >= MAX_ANNOUNCE_ATTEMPTS {
                // Out of attempts. The row stays on disk and stays poll-able;
                // only the proactive notice is given up on.
                index.insert(record.id, record);
                continue;
            }
            let attempted = JobRecord {
                announce_attempts: record.announce_attempts.saturating_add(1),
                ..record
            };
            write_state(&dir, &attempted);
            undelivered.push(attempted.clone());
            index.insert(attempted.id, attempted);
        } else {
            index.insert(record.id, record);
        }
    }

    // The watermark file is the authority (it covers ids reserved but never
    // used); the rows are the belt-and-braces floor for a store whose watermark
    // write was lost or predates this file.
    let reserved = read_watermark(&dir).unwrap_or(0).max(highest_row_id);

    *store_lock() = Some(dir);
    *index_lock() = index;
    *reserved_lock() = reserved;
    *undelivered_lock() = undelivered;

    // Seeded here rather than at the boot call site so no future boot path can
    // enable the journal and forget the allocator — a resurrected row and a
    // live job sharing one id is the failure this pairs with. Same argument for
    // the flusher on the next line: enabling the journal without it would ship
    // a tombstone that can never carry output.
    super::process_registry::process_registry().seed_id_floor(id_floor());
    spawn_partial_flusher();

    if tombstoned > 0 {
        tracing::info!(
            interrupted = tombstoned,
            "process_journal: tombstoned background bash jobs left running by a previous process"
        );
    }
    tombstoned
}

/// Boot entry point: reconcile, then hand back the completions the previous
/// daemon finished but never announced. Returns the tombstone count
/// [`init_and_reconcile`] returns.
///
/// **Ordering:** this broadcasts, so it must run *after* the completion
/// announcer has subscribed (§9 — reconcile after subscribing). The tombstone
/// half has no such requirement and never did; the handback half does, and the
/// two travel together on purpose so no boot path can take one without the
/// other.
///
/// One event per job rather than one per session, unlike the sub-agent orphan
/// sweep: that one groups because a crash orphans a whole fan-out at once,
/// while this population is bounded by [`ANNOUNCE_HANDBACK_MAX_AGE_MS`] and by
/// the per-session running cap — the jobs that settled in the couple of minutes
/// a daemon spent dying inside the retry ladder.
pub async fn init_and_announce(dir: PathBuf) -> usize {
    let tombstoned = init_and_reconcile(dir);
    for job in take_undelivered_settled() {
        let Some(session) = super::bash_exec::session_key_from_label(&job.record.owner) else {
            tracing::debug!(
                id = job.record.id,
                "process_journal: recovered completion has no addressable session; it stays poll-only"
            );
            continue;
        };
        let event = super::process_completion::recovered_completion_event(
            job.record.id,
            &job.record.command,
            job.record.exit_code.unwrap_or_default(),
            &job.recorded_output,
        );
        tracing::info!(
            id = job.record.id,
            "process_journal: announcing a background job that finished before the previous daemon stopped"
        );
        super::process_completion::broadcast(&session, event).await;
        // BT-D-R4-16: the delivery stamp goes on AFTER the broadcast
        // returns. A crash mid-broadcast leaves the row unstamped on disk
        // (with one attempt spent), so the next boot retries the handback.
        // Previously the stamp ran in init_and_reconcile, before this
        // broadcast was reached, which silently lost completions.
        record_announced(job.record.id);
    }
    tombstoned
}

/// Lowest id a fresh registry may hand out: one past everything ever reserved.
/// `1` while persistence is off, i.e. the allocator's historical start.
pub(crate) fn id_floor() -> u64 {
    reserved_lock().saturating_add(1)
}

/// Does this row describe a completion the owning session was never told about,
/// recently enough that telling it now is news rather than history?
///
/// Three conditions, and each excludes a different population:
///
/// * `Settled` — an `Interrupted` row is a statement about the *previous
///   daemon* and, through its tombstone, about the OS process — never about
///   what the command achieved; announcing "it was interrupted" would spend a
///   turn on a verdict nobody reached. Those rows stay poll-able, which is
///   the recorded decision.
/// * `outcome == completed` — a killed job is the owner's own action, so its
///   outcome is not news (the same stance `subagent_tool::spawn` takes for a
///   cancelled child). Without this test every `kill` would queue an announce
///   for the next boot, since nothing ever stamps those rows delivered.
/// * fresh — see [`ANNOUNCE_HANDBACK_MAX_AGE_MS`].
///
/// A fourth condition lives in the caller rather than here, because it is about
/// this boot and not about the row's contents: [`MAX_ANNOUNCE_ATTEMPTS`].
fn is_undelivered_completion(record: &JobRecord, now: u64) -> bool {
    record.phase == JobPhase::Settled
        && record.announced_boot.is_none()
        && record.outcome.as_deref() == Some(Verdict::Completed.label())
        && record
            .ended_ms
            .is_some_and(|t| now.saturating_sub(t) <= ANNOUNCE_HANDBACK_MAX_AGE_MS)
}

/// Drain the completions [`init_and_reconcile`] found undelivered, hydrated
/// with whatever output was recorded for them.
///
/// Destructive: within a boot this is the one chance to deliver them. The rows
/// are stamped delivered on disk only by `init_and_announce` AFTER the
/// broadcast returns (BT-D-R4-16), so a boot that drains without announcing
/// hands the completion back again next boot — up to [`MAX_ANNOUNCE_ATTEMPTS`]
/// times, which is the bound on "again". Empty for every boot that had nothing
/// to hand back, which is the overwhelming majority.
pub(crate) fn take_undelivered_settled() -> Vec<RecoveredJob> {
    let Some(dir) = store_dir() else {
        return Vec::new();
    };
    let records = std::mem::take(&mut *undelivered_lock());
    let mut out: Vec<RecoveredJob> = records
        .into_iter()
        .map(|record| hydrate(&dir, record))
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.record.started_ms));
    out
}

// ============================================================================
// Write path (called by ProcessRegistry)
// ============================================================================

/// Durably reserve ids through `id` **before** the registry hands it out.
///
/// Cheap by design: one atomic write per [`ID_RESERVATION_BLOCK`] ids, none at
/// all while persistence is off. Reserving *after* allocation would reuse the
/// id on a crash in between, which is the collision this exists to prevent.
///
/// A failed write leaves the in-memory mark where it was, so the next spawn
/// retries. Stated honestly: if the store stays unwritable, ids can repeat
/// after a restart. Two things stop that from becoming a wrong answer rather
/// than merely an ugly one, and both are needed —
///
/// * [`init_and_reconcile`] floors the allocator at
///   `max(watermark, highest_dir_id, highest_row_id)`, so any id whose
///   *directory* landed is covered even when this write did not;
/// * [`record_spawn`] discards a directory it finds already occupied, so an id
///   that repeats anyway cannot hand its new owner the previous owner's
///   `output.txt`.
///
/// What is left is the honest residual: a repeated id destroys the older row's
/// recoverable history. That is the correct direction to fail — serving one
/// session's output to another is worse than losing it.
pub(crate) fn reserve_id(id: u64) {
    let Some(dir) = store_dir() else { return };
    let mut reserved = reserved_lock();
    if id <= *reserved {
        return;
    }
    let target = id.saturating_add(ID_RESERVATION_BLOCK - 1);
    match write_watermark(&dir, target) {
        Ok(()) => *reserved = target,
        Err(e) => {
            tracing::warn!(error = %e, id, "process_journal: id watermark write failed; ids may repeat after a restart");
        }
    }
}

/// Is the journal on? Lets the registry skip cloning output it would only
/// throw away — every test and every non-daemon binary takes that branch.
pub(crate) fn is_enabled() -> bool {
    store_lock().is_some()
}

/// Record a freshly-registered background job.
///
/// **Refuses an unowned job.** The registry allows `session_label: None` (a
/// direct/library caller with no session), and a persisted row with no owner
/// would be readable by every later unscoped caller — the precise fail-open bug
/// `background_persistence::addressable` was fixed for. Such a job keeps its
/// old, purely in-memory behaviour.
pub(crate) fn record_spawn(id: u64, command: &str, owner: Option<&str>) {
    let Some(dir) = store_dir() else { return };
    let Some(owner) = owner.filter(|o| !o.is_empty()) else {
        return;
    };
    // An id must never inherit the previous holder's files. `write_state`
    // overwrites, but `append_block` APPENDS — so a reissued id would hand the
    // new owner a trail containing another session's output, which is a
    // cross-session leak wearing the costume of a numbering bug. The floor in
    // [`init_and_reconcile`] is supposed to make this unreachable; this is the
    // belt, and it is loud, because reaching it means the floor did not hold.
    let dir_for_job = job_dir(&dir, id);
    if dir_for_job.exists() {
        tracing::warn!(
            id,
            "process_journal: reusing an id whose directory already exists — discarding the \
             stale row rather than letting a new owner inherit its output trail"
        );
        let _ = std::fs::remove_dir_all(&dir_for_job);
    }
    let record = JobRecord {
        id,
        owner: owner.to_string(),
        // A command line is model-authored and routinely carries a credential
        // (`curl -H "Authorization: …"`); it lands in the same store the trail
        // does, so it takes the same gate.
        command: mask_block(command),
        started_ms: now_ms(),
        phase: JobPhase::Running,
        ended_ms: None,
        outcome: None,
        exit_code: None,
        output_file: Some(OUTPUT_FILE.to_string()),
        partial_file: Some(PARTIAL_FILE.to_string()),
        announce_attempts: 0,
        announced_boot: None,
        kind: JournalKind::Bash,
        pty_session_id: None,
        pid: None,
        process_created_at_ms: None,
        tombstone: None,
    };
    write_state(&dir, &record);
    index_lock().insert(id, record);
}

/// The creation time a child row carries, in unix ms — off the SAME routine
/// the boot probe compares against
/// ([`crate::utils::process_alive::process_start_time`], seconds, scaled to
/// ms here and in [`probe_liveness`] alike), so the stored value and the
/// compared value are one derivation. A start time of 0 is sysinfo's "could
/// not open the process", not a time: it reads as `None`, so the probe
/// answers `Unknown` for it instead of comparing against a number that was
/// never one. Every row kind that records a child goes through this.
fn creation_time_ms(pid: u32) -> Option<u64> {
    crate::utils::process_alive::process_start_time(i32::try_from(pid).unwrap_or(-1))
        .filter(|s| *s != 0)
        .map(|s| s.saturating_mul(1000))
}

/// Third write of a job's life: the driver has the OS child, and the row
/// gains its pid and [`creation_time_ms`].
///
/// No-op without an intent row — the reconcile can only tombstone what was
/// written before the crash, so the intent must come first and this may
/// only ever *add* to it. An upsert here would let a pid-only row exist with
/// no owner, no command and no start, and that row would read as a job.
pub(crate) fn record_child(id: u64, pid: u32) {
    let Some(dir) = store_dir() else { return };
    let created = creation_time_ms(pid);
    let record = {
        let mut index = index_lock();
        let Some(r) = index.get_mut(&id) else { return };
        r.pid = Some(pid);
        r.process_created_at_ms = created;
        r.clone()
    };
    write_state(&dir, &record);
}

/// Rewrite a job's live-tail capture — "here is what it had produced".
///
/// `text` MUST already have cleared
/// [`crate::builtin_tools::partial_output::gate`]; this function does not gate,
/// it only masks (again — the reader is a later process) and writes. Callers
/// are [`spawn_partial_flusher`] and the registry's kill / shutdown paths,
/// which are the two moments a job stops producing without producing a result.
///
/// Rewritten in place, never appended: two producers on one append-only file
/// double-count, and the reader only ever shows the last
/// [`OUTPUT_TAIL_BYTES`] anyway, so there is nothing to accumulate.
pub(crate) fn record_partial(id: u64, text: &str) {
    let Some(dir) = store_dir() else { return };
    let Some(record) = index_lock().get(&id).cloned() else {
        // Never journaled (unowned, or started before the journal was enabled).
        return;
    };
    let Some(path) = record.partial_path(&dir) else {
        return;
    };
    let run_dir = job_dir(&dir, id);
    if let Err(e) = std::fs::create_dir_all(&run_dir) {
        tracing::debug!(error = %e, "process_journal: cannot create job dir");
        return;
    }
    let body = format!("{}\n{}", now_ms(), mask_block(text));
    if let Err(e) = crate::utils::atomic_io::write_atomic(&path, body.as_bytes()) {
        tracing::debug!(error = %e, id, "process_journal: partial capture write failed");
    }
}

/// Guards against a second [`init_and_reconcile`] starting a second flusher.
static FLUSHER_STARTED: AtomicBool = AtomicBool::new(false);

/// Start the background task that keeps every running job's live-tail capture
/// on disk, so a job whose daemon is `SIGKILL`ed still has an answer to "and
/// what had it printed?".
///
/// **This is the only mechanism that can serve the interrupted population.**
/// The kill and shutdown paths capture the tail synchronously because they run
/// *as* the job stops; a crash runs nothing at all, so the last durable answer
/// has to have been written before it. That is the whole reason this loop
/// exists and the reason its interval is the resolution of the answer.
///
/// Started from [`init_and_reconcile`] rather than from the daemon's boot
/// script, for the same reason `seed_id_floor` is: a future boot path can
/// enable the journal, and it must not be able to enable it and forget this.
/// No async runtime (CLI, tests, `#[test]` callers of the reconcile) means no
/// flusher and no error — the journal simply keeps its pre-existing behaviour.
fn spawn_partial_flusher() {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::debug!("process_journal: no tokio runtime, live-tail flusher not started");
        return;
    };
    if FLUSHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tracing::debug!(
        interval_secs = PARTIAL_FLUSH_INTERVAL.as_secs(),
        "process_journal: live-tail flusher started"
    );
    handle.spawn(async move {
        // Last byte totals written per job. Purely an I/O saver: losing it
        // costs one redundant rewrite, never a wrong file.
        let mut written: HashMap<u64, (u64, u64)> = HashMap::new();
        loop {
            tokio::time::sleep(PARTIAL_FLUSH_INTERVAL).await;
            flush_running_partials(&super::process_registry::process_registry(), &mut written);
        }
    });
}

/// One flusher tick. Returns how many captures were rewritten.
///
/// Separated from the loop so it can be driven directly, and takes the registry
/// explicitly rather than reaching for the process-global singleton: a tick
/// that can only be tested through a `sleep(15s)` is a tick nobody tests.
///
/// `written` is pruned against the live set on every call, so a long-lived
/// daemon does not accumulate entries for jobs that finished hours ago.
fn flush_running_partials(
    registry: &super::process_registry::ProcessRegistry,
    written: &mut HashMap<u64, (u64, u64)>,
) -> usize {
    if !is_enabled() {
        return 0;
    }
    let live = registry.running_live_tails();
    written.retain(|id, _| live.iter().any(|(other, _)| other == id));
    let mut flushed = 0usize;
    for (id, tail) in live {
        let snapshot = tail.snapshot();
        let totals = (snapshot.stdout_total, snapshot.stderr_total);
        // Nothing new since the last write: the file on disk already says
        // exactly this, and rewriting it would cost a `fsync` per idle job.
        if written.get(&id) == Some(&totals) {
            continue;
        }
        if let Some(text) = crate::builtin_tools::partial_output::durable_text(&snapshot) {
            record_partial(id, &text);
            written.insert(id, totals);
            flushed += 1;
        }
    }
    flushed
}

/// Record a job's terminal state. The tombstone stays on disk (and in the
/// index) for the retention window, so the id keeps answering.
///
/// `stdout` / `stderr` must already have been through the sandbox's finished-
/// path scrub; pass empty strings when there is no output to record (a killed
/// job has none). They are masked again here regardless — a later process is
/// the reader.
pub(crate) fn record_settled(
    id: u64,
    verdict: Verdict,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) {
    let Some(dir) = store_dir() else { return };
    if !index_lock().contains_key(&id) {
        // Never journaled (unowned, or started before the journal was enabled).
        return;
    }
    append_block(&dir, id, "stdout", stdout);
    append_block(&dir, id, "stderr", stderr);
    let record = {
        let mut index = index_lock();
        let Some(record) = index.get_mut(&id) else {
            return;
        };
        record.phase = JobPhase::Settled;
        record.ended_ms = Some(now_ms());
        record.outcome = Some(verdict.label().to_string());
        record.exit_code = exit_code;
        record.clone()
    };
    write_state(&dir, &record);
}

/// Stamp "the owning session was told about this job", with the boot that told
/// it.
///
/// The **only** writer of [`JobRecord::announced_boot`], and it is called only
/// from a delivery that has already returned: the announcer's success arm and
/// the boot handback's post-broadcast line. Nothing that is merely *about to*
/// announce may write this — that is what `announce_attempts` is for. Without
/// the stamp, a restart inside the retry ladder re-delivers a notice the
/// session already received.
///
/// No-op for an id this process never journaled, and for a row already stamped
/// — the first boot that delivered it is the true answer to "who told them".
pub(crate) fn record_announced(id: u64) {
    let Some(dir) = store_dir() else { return };
    let record = {
        let mut index = index_lock();
        let Some(record) = index.get_mut(&id) else {
            return;
        };
        if record.announced_boot.is_some() {
            return;
        }
        record.announced_boot = Some(now_ms());
        record.clone()
    };
    write_state(&dir, &record);
}

// ============================================================================
// Read path (called by the bash tool's not-found / directory faces)
// ============================================================================

/// Look up a job this process's registry does not know about, scoped to
/// `caller` exactly the way the live table is scoped.
///
/// `pub(crate)` on purpose: [`init_and_reconcile`] is the only entry point the
/// binary needs, and every read goes through the bash tool's single resolver
/// (`bash_exec::resolve_forgotten`) so no future surface can grow a second,
/// differently-scoped way to read these rows.
#[must_use]
pub(crate) fn lookup(id: u64, caller: Option<&str>) -> Option<RecoveredJob> {
    let dir = store_dir()?;
    let record = index_lock().get(&id).cloned()?;
    if !addressable(&record, caller) {
        return None;
    }
    Some(hydrate(&dir, record))
}

/// Every journaled job this caller owns, minus the ids the live table already
/// answered for, newest first.
#[must_use]
pub(crate) fn list_for_scope(caller: Option<&str>, exclude: &[u64]) -> Vec<RecoveredJob> {
    let Some(dir) = store_dir() else {
        return Vec::new();
    };
    let records: Vec<JobRecord> = index_lock()
        .values()
        .filter(|r| addressable(r, caller) && !exclude.contains(&r.id))
        .cloned()
        .collect();
    let mut out: Vec<RecoveredJob> = records
        .into_iter()
        .map(|record| hydrate(&dir, record))
        .collect();
    // Newest first, matching the live `list` face — an enumeration that ordered
    // the two halves differently would read as two directories.
    out.sort_by_key(|r| std::cmp::Reverse(r.record.started_ms));
    out
}

/// May a caller owning `caller` see this row?
///
/// Strict equality, and **`None` sees nothing** — the one deliberate divergence
/// from `background_persistence::addressable`, which lets an unscoped caller
/// (its CLI face) see everything. There is no unscoped face for background
/// `bash` jobs: `session_label()` is `None` only for a caller with no session
/// at all, and letting that caller read every session's job history out of a
/// process-global store is the fail-open direction. The write side refuses the
/// same case, so no row can be owned by "nobody" either.
fn addressable(record: &JobRecord, caller: Option<&str>) -> bool {
    caller.is_some_and(|want| record.owner == want)
}

// ============================================================================
// Internals
// ============================================================================

fn store_lock() -> MutexGuard<'static, Option<PathBuf>> {
    STORE_DIR.lock().unwrap_or_else(|e| e.into_inner())
}

fn index_lock() -> MutexGuard<'static, HashMap<u64, JobRecord>> {
    INDEX.lock().unwrap_or_else(|e| e.into_inner())
}

fn reserved_lock() -> MutexGuard<'static, u64> {
    RESERVED_THROUGH.lock().unwrap_or_else(|e| e.into_inner())
}

fn undelivered_lock() -> MutexGuard<'static, Vec<JobRecord>> {
    UNDELIVERED.lock().unwrap_or_else(|e| e.into_inner())
}

fn store_dir() -> Option<PathBuf> {
    store_lock().clone()
}

/// Wall-clock unix ms, borrowed from the sibling sidecar's helper so both
/// stores stamp their rows off one clock reading routine.
fn now_ms() -> u64 {
    crate::agents::subagent_tree_events::now_ms()
}

/// Directory holding one job's row.
///
/// No sanitizer, unlike `background_persistence::slug`: that module's key is a
/// model-visible *string* which could contain `../`, whereas this one is a
/// `u64` the registry allocates. `format!` over an integer cannot escape the
/// root.
fn job_dir(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{JOB_DIR_PREFIX}{id}"))
}

/// The one spelling of a job directory's name. Written by [`job_dir`], read
/// back by [`highest_dir_id`] — two literals here would let the id floor stop
/// recognising the directories the writer creates, silently.
const JOB_DIR_PREFIX: &str = "job-";

/// Mask + bound text before it can reach the disk, **keeping line structure**.
///
/// The sub-agent sidecar flattens newlines because it stores one-line progress
/// notes. A stdout trail flattened into one line is unreadable and blows the
/// per-line cap, so each line is masked and capped on its own.
fn mask_block(text: &str) -> String {
    text.lines()
        .map(|line| {
            let bounded: String = line.chars().take(MAX_LINE_CHARS).collect();
            MASKER.mask(&bounded)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_state(dir: &Path, record: &JobRecord) {
    let run_dir = job_dir(dir, record.id);
    if let Err(e) = std::fs::create_dir_all(&run_dir) {
        tracing::debug!(error = %e, "process_journal: cannot create job dir");
        return;
    }
    let bytes = match serde_json::to_vec_pretty(record) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(error = %e, "process_journal: cannot serialize row");
            return;
        }
    };
    if let Err(e) = crate::utils::atomic_io::write_atomic(&run_dir.join(STATE_FILE), &bytes) {
        tracing::debug!(error = %e, "process_journal: state write failed");
    }
}

/// Append one labelled block of output. Each source line becomes its own
/// `<unix_ms>\t<masked line>` trail line, so [`read_trail`] can rebuild the
/// original line structure while still finding a timestamp on the last line.
fn append_block(dir: &Path, id: u64, label: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let run_dir = job_dir(dir, id);
    if let Err(e) = std::fs::create_dir_all(&run_dir) {
        tracing::debug!(error = %e, "process_journal: cannot create job dir");
        return;
    }
    let stamp = now_ms();
    let mut buf = format!("{stamp}\t[{label}]\n");
    for line in mask_block(text).lines() {
        buf.push_str(&format!("{stamp}\t{line}\n"));
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join(OUTPUT_FILE))
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(buf.as_bytes()) {
                tracing::debug!(error = %e, "process_journal: trail append failed");
            }
        }
        Err(e) => tracing::debug!(error = %e, "process_journal: trail open failed"),
    }
}

#[derive(Default)]
struct Trail {
    text: String,
    last_ms: Option<u64>,
    from_live_capture: bool,
}

impl Trail {
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Read back a job's recorded output: the finished-path trail if it has one,
/// otherwise the live-tail capture.
///
/// **Order matters and is not arbitrary.** A completed job's `output.txt` is
/// its result; its `partial.txt` is a stale window from partway through the
/// same run. Preferring the capture would replace an answer with a guess.
///
/// Both files are located through the names the row itself carries
/// ([`JobRecord::output_path`] / [`JobRecord::partial_path`]) — not by
/// re-deriving the layout convention here.
fn read_trail(dir: &Path, record: &JobRecord) -> Trail {
    let finished = read_output_trail(dir, record);
    if !finished.is_empty() {
        return finished;
    }
    read_partial_capture(dir, record).unwrap_or(finished)
}

/// The append-only finished-path trail, restoring line structure.
fn read_output_trail(dir: &Path, record: &JobRecord) -> Trail {
    let Some(path) = record.output_path(dir) else {
        return Trail::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Trail::default();
    };
    let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
    let last_ms = lines
        .last()
        .and_then(|l| l.split_once('\t'))
        .and_then(|(ts, _)| ts.parse::<u64>().ok());

    let rendered: String = lines
        .iter()
        .map(|l| l.split_once('\t').map_or(*l, |(_, body)| body))
        .collect::<Vec<_>>()
        .join("\n");
    Trail {
        text: keep_tail(rendered),
        last_ms,
        from_live_capture: false,
    }
}

/// The rewritten-in-place live capture: `<unix_ms>\n<already-masked text>`.
///
/// The stamp is stored in the file rather than taken from its mtime because
/// mtime is a property of the filesystem, not of the record — a copy, a
/// restore, or a `tar -x` would silently re-date the job's last known activity.
fn read_partial_capture(dir: &Path, record: &JobRecord) -> Option<Trail> {
    let path = record.partial_path(dir)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    let (stamp, body) = raw.split_once('\n')?;
    if body.is_empty() {
        return None;
    }
    Some(Trail {
        text: keep_tail(body.to_string()),
        last_ms: stamp.parse::<u64>().ok(),
        from_live_capture: true,
    })
}

/// Keep the TAIL, not the head: what the job printed last is the actionable
/// part. UTF-8 safe by character boundary (P7) — walking backwards the
/// predicate holds for every index in the tail, so the boundary wanted is
/// the SMALLEST satisfying index (`take_while(..).last()`, never `find`).
fn keep_tail(rendered: String) -> String {
    if rendered.len() <= OUTPUT_TAIL_BYTES {
        return rendered;
    }
    let start = rendered
        .char_indices()
        .rev()
        .map(|(i, _)| i)
        .take_while(|i| rendered.len() - i <= OUTPUT_TAIL_BYTES)
        .last()
        .unwrap_or(0);
    format!("…{}", &rendered[start..])
}

fn hydrate(dir: &Path, record: JobRecord) -> RecoveredJob {
    let trail = read_trail(dir, &record);
    RecoveredJob {
        last_activity_ms: trail.last_ms.unwrap_or(record.started_ms),
        recorded_output: trail.text,
        output_is_live_capture: trail.from_live_capture,
        record,
    }
}

/// Highest id that has a directory under `dir`, readable row or not.
///
/// The complement to [`read_all`]: that function answers "which rows can I
/// serve", this one answers "which ids have ever been handed out", and the
/// second set is strictly larger. Only the second one may be used as an id
/// floor — a corrupt row is still a claimed id, and a claimed id whose
/// directory survives is exactly the one a reissue would collide with.
fn highest_dir_id(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_prefix(JOB_DIR_PREFIX))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
}

fn read_all(dir: &Path) -> Vec<JobRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let state = entry.path().join(STATE_FILE);
        let Ok(bytes) = std::fs::read(&state) else {
            // Also the path taken by the watermark file at the store root,
            // which is a file and has no `state.json` under it.
            continue;
        };
        match serde_json::from_slice::<JobRecord>(&bytes) {
            Ok(record) => out.push(record),
            Err(e) => {
                // Fail-open on a corrupt row: a bad file must never block boot.
                // Logged rather than deleted so it is still there to diagnose.
                tracing::warn!(error = %e, path = %state.display(), "process_journal: unreadable row");
            }
        }
    }
    out
}

#[derive(Serialize, Deserialize)]
struct Watermark {
    reserved_through: u64,
}

fn read_watermark(dir: &Path) -> Option<u64> {
    let bytes = std::fs::read(dir.join(ID_WATERMARK_FILE)).ok()?;
    serde_json::from_slice::<Watermark>(&bytes)
        .ok()
        .map(|w| w.reserved_through)
}

fn write_watermark(dir: &Path, value: u64) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(&Watermark {
        reserved_through: value,
    })
    .map_err(std::io::Error::other)?;
    crate::utils::atomic_io::write_atomic(&dir.join(ID_WATERMARK_FILE), &bytes)
}

/// Serializes every test that points the journal somewhere: the store root is
/// process-global by design (one daemon, one store), so two tests aiming it at
/// different tempdirs at once would read each other's rows. Exposed
/// crate-wide because the tool-level faces test lives in `bash_exec::tests`.
#[cfg(test)]
static TEST_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`TEST_GATE`]. Poison-tolerant (P7): a panicking test must not wedge
/// every later one.
#[cfg(test)]
pub(crate) fn test_gate() -> std::sync::MutexGuard<'static, ()> {
    TEST_GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Test-only: drive one flusher tick against `registry`.
///
/// Exposed rather than duplicated because the tick's two behaviours — writes
/// when the counters moved, does not when they have not — are the whole
/// contract, and a test that could only observe them through a 15-second
/// `sleep` is a test nobody writes. Lives with `flush_running_partials` so the
/// spawned loop and the tested path are the same function, not two.
#[cfg(test)]
pub(crate) fn flush_running_partials_for_test(
    registry: &super::process_registry::ProcessRegistry,
    written: &mut HashMap<u64, (u64, u64)>,
) -> usize {
    flush_running_partials(registry, written)
}

/// Test-only: point the journal at `dir` without running the boot reconcile,
/// and drop whatever a previous test left in the index.
#[cfg(test)]
pub(crate) fn enable_for_test(dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("test store dir");
    *store_lock() = Some(dir);
    index_lock().clear();
    *reserved_lock() = 0;
    undelivered_lock().clear();
}

/// Test-only: turn persistence back off so unrelated tests keep their zero-I/O
/// behaviour once this test's tempdir is gone.
#[cfg(test)]
pub(crate) fn disable_for_test() {
    *store_lock() = None;
    index_lock().clear();
    *reserved_lock() = 0;
    undelivered_lock().clear();
}

#[cfg(test)]
mod tests {
    use super::test_gate as gate;
    use super::*;

    const OWNER: &str = "{\"Ephemeral\":{\"agent_id\":\"a\",\"ephemeral_id\":\"j\"}}";

    // The id high-water allocator is exercised where it lives, against two
    // freshly-constructed registries:
    // `process_registry::tests::a_second_boot_never_re_issues_an_id_the_first_boot_issued`.

    /// The core of W5: a job left `Running` on disk by a dead daemon must come
    /// back as a row that still EXISTS and reads as interrupted, never failed.
    ///
    /// Asserts on the re-read of `state.json` (via `lookup`), not on the return
    /// count alone — discarding the `write_state` inside the reconcile would
    /// still produce a non-zero count, and the next boot would re-tombstone the
    /// same job forever.
    #[test]
    fn boot_reconcile_tombstones_orphans_instead_of_forgetting_them() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(3, "cargo build --release", Some(OWNER));
        disable_for_test();

        // A fresh daemon boots against the same directory.
        init_and_reconcile(tmp.path().to_path_buf());

        let found = lookup(3, Some(OWNER)).expect("the row must survive the restart");
        assert_eq!(found.record.phase, JobPhase::Interrupted);
        let label = settled_label(&found.record);
        assert_eq!(label, "interrupted_by_restart_liveness_unknown");
        assert!(
            !label.contains("fail"),
            "a restart is not a verdict on the command: {label}"
        );
        assert_eq!(found.record.command, "cargo build --release");

        // The tombstone is durable, so a SECOND boot re-tombstones nothing.
        init_and_reconcile(tmp.path().to_path_buf());
        let again = lookup(3, Some(OWNER)).expect("still addressable");
        assert_eq!(again.record.phase, JobPhase::Interrupted);
        assert_eq!(
            again.record.ended_ms, found.record.ended_ms,
            "a tombstoned row must not be re-stamped on every later boot"
        );
        disable_for_test();
    }

    // ========================================================================
    // The pid, the probe, and the two-arm tombstone
    // ========================================================================

    /// Every row written before `kind` / `pid` / `tombstone` existed must keep
    /// decoding — as the Bash row it always was, with nothing probed.
    #[test]
    fn an_old_row_without_the_new_fields_still_decodes_as_a_bash_row() {
        let r: JobRecord = serde_json::from_str(
            r#"{"id":7,"owner":"o","command":"c","started_ms":1,"phase":"running"}"#,
        )
        .unwrap();
        assert_eq!(
            (
                r.kind,
                r.pid,
                r.process_created_at_ms,
                r.tombstone,
                r.pty_session_id
            ),
            (JournalKind::Bash, None, None, None, None)
        );
    }

    /// The third write may only ADD to an intent row: a pid that arrives for
    /// an id nobody journaled is dropped, not upserted into a row with no
    /// owner. And the creation time it stamps is real — asserted on the bytes
    /// on disk, since the probe at the next boot reads those, not the index.
    #[test]
    fn record_child_needs_the_intent_row_and_stamps_pid_plus_creation_time() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        let me = std::process::id();
        record_child(5, me); // before the intent: no-op
        assert!(lookup(5, Some(OWNER)).is_none());
        // Asked of the disk as well as the index: an upsert that invents a row
        // with no owner is invisible to `lookup` (nothing is addressable by
        // nobody) but still lands a `state.json` — and `record_spawn` would
        // then discard it as a reissued id, silently, so the index alone
        // cannot tell "no-op" from "wrote a row nobody can read".
        assert!(
            !tmp.path().join("job-5").exists(),
            "a pid for an id nobody journaled must not create a row"
        );
        record_spawn(5, "sleep 300", Some(OWNER));
        assert_eq!(
            lookup(5, Some(OWNER)).unwrap().record.pid,
            None,
            "the intent row carries no pid"
        );
        record_child(5, me);
        let on_disk: JobRecord = serde_json::from_slice(
            &std::fs::read(tmp.path().join("job-5").join(STATE_FILE)).unwrap(),
        )
        .unwrap();
        assert_eq!((on_disk.pid, on_disk.kind), (Some(me), JournalKind::Bash));
        assert!(
            on_disk.process_created_at_ms.is_some(),
            "sysinfo must report this process's start time"
        );
        disable_for_test();
    }

    /// The instrument, against the one process every test can vouch for.
    /// `Unknown` is a real answer: a pid that is present but cannot be
    /// verified is not a verdict either way.
    #[test]
    fn probe_liveness_answers_all_three_ways() {
        let me = std::process::id();
        let created = crate::utils::process_alive::process_start_time(me as i32).map(|s| s * 1000);
        assert_eq!(probe_liveness(me, created), Liveness::StillRunning);
        assert_eq!(
            probe_liveness(me, created.map(|c| c + 60_000)),
            Liveness::Exited,
            "a recycled pid is not this process"
        );
        assert_eq!(
            probe_liveness(me, None),
            Liveness::Unknown,
            "present but unverifiable is not a verdict"
        );
        assert_eq!(probe_liveness(u32::MAX - 7, Some(1)), Liveness::Exited);
    }

    /// Boot against a scripted probe and read row 9 back.
    fn boot_with(dir: &std::path::Path, answer: Liveness) -> JobRecord {
        init_and_reconcile_with_probe(dir.to_path_buf(), &move |_, _| answer);
        lookup(9, Some(OWNER)).expect("row survives").record
    }

    /// The two arms, and the re-ask: an orphan found still running is asked
    /// again at the next boot, and a definite exit rewrites it — without
    /// re-dating the restart that orphaned it. An exit is final.
    #[test]
    fn reconcile_writes_the_probed_arm_and_reasks_a_still_running_one() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(9, "sleep 300", Some(OWNER));
        record_child(9, std::process::id());
        disable_for_test();
        let r = boot_with(tmp.path(), Liveness::StillRunning);
        assert_eq!(
            (r.phase, r.tombstone),
            (
                JobPhase::Interrupted,
                Some(Tombstone::StillRunningUnattached {
                    pid: std::process::id()
                })
            )
        );
        assert_eq!(settled_label(&r), "still_running_unattached");
        let stamp = r.ended_ms;
        let r = boot_with(tmp.path(), Liveness::Exited); // the orphan died between boots
        assert_eq!(
            (r.tombstone, settled_label(&r), r.ended_ms),
            (
                Some(Tombstone::ExitedDuringRestart),
                "exited_during_restart",
                stamp
            )
        );
        let r = boot_with(tmp.path(), Liveness::StillRunning); // exited is final: not re-asked
        assert_eq!(r.tombstone, Some(Tombstone::ExitedDuringRestart));
        disable_for_test();
    }

    /// Criterion #8: an answer the instrument could not give is not an exit.
    /// A row with a pid whose probe says `Unknown`, and a row that never got a
    /// pid, both keep the pre-existing liveness-unknown wording — and none of
    /// the three labels reads as a failure.
    #[test]
    fn an_unknown_probe_and_a_pidless_row_keep_the_liveness_unknown_wording() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(9, "sleep 300", Some(OWNER));
        record_child(9, std::process::id());
        record_spawn(10, "sleep 300", Some(OWNER)); // never got a pid
        disable_for_test();
        let r = boot_with(tmp.path(), Liveness::Unknown);
        assert_eq!(
            (r.tombstone, settled_label(&r)),
            (None, "interrupted_by_restart_liveness_unknown")
        );
        let ten = lookup(10, Some(OWNER)).unwrap().record;
        assert_eq!((ten.phase, ten.tombstone), (JobPhase::Interrupted, None));
        for l in [
            "exited_during_restart",
            "still_running_unattached",
            settled_label(&ten),
        ] {
            assert!(!l.contains("fail"), "{l}");
        }
        disable_for_test();
    }

    // ========================================================================
    // The label the model reads first
    // ========================================================================

    fn labelled(phase: JobPhase, outcome: Option<&str>) -> &'static str {
        settled_label(&JobRecord {
            id: 1,
            owner: OWNER.to_string(),
            command: "cargo build".to_string(),
            started_ms: 1,
            phase,
            ended_ms: Some(2),
            outcome: outcome.map(str::to_string),
            exit_code: None,
            output_file: None,
            partial_file: None,
            announce_attempts: 0,
            announced_boot: None,
            kind: JournalKind::Bash,
            pty_session_id: None,
            pid: None,
            process_created_at_ms: None,
            tombstone: None,
        })
    }

    /// C2's twin question, asked of this vocabulary: `phase == Settled` is not
    /// `outcome == completed`. The phase-only label answered `"recorded"` for a
    /// job that finished and for one the owner killed half way, so the first
    /// word the model reads could not tell them apart.
    ///
    /// An outcome word this daemon does not recognise reads `settled_unknown`,
    /// never a success word: a label that says "it finished" has to be earned
    /// by a producer that wrote one.
    #[test]
    fn a_settled_label_reads_the_outcome_not_just_the_phase() {
        assert_eq!(labelled(JobPhase::Settled, Some("completed")), "completed");
        assert_eq!(labelled(JobPhase::Settled, Some("killed")), "killed");
        assert_eq!(labelled(JobPhase::Settled, None), "settled_unknown");
        assert_eq!(
            labelled(JobPhase::Settled, Some("something-newer")),
            "settled_unknown"
        );
        // And the two non-terminal phases still refuse to read as verdicts.
        assert_eq!(
            labelled(JobPhase::Interrupted, None),
            "interrupted_by_restart_liveness_unknown"
        );
        assert_eq!(labelled(JobPhase::Running, None), "running_unconfirmed");
        for phase in [JobPhase::Running, JobPhase::Interrupted] {
            assert!(
                !labelled(phase, None).contains("fail"),
                "a restart is not a verdict on the command"
            );
        }
    }

    /// The words are the writer's, not a second spelling of them: a verdict
    /// this module can record must be a verdict the label can name.
    #[test]
    fn every_verdict_the_writer_records_has_its_own_label() {
        for verdict in [Verdict::Completed, Verdict::Killed] {
            let label = labelled(JobPhase::Settled, Some(verdict.label()));
            assert_eq!(
                label,
                verdict.label(),
                "a verdict with no label of its own falls into settled_unknown"
            );
        }
    }

    // ========================================================================
    // The delivery stamp and the boot handback
    // ========================================================================

    /// Write a terminal row directly, so a test can choose its age, its verdict
    /// and how many boots have already tried to hand it back — the three things
    /// the handback filter reads and none of which the normal write path lets
    /// you pick.
    fn seed_settled_attempted(
        dir: &std::path::Path,
        id: u64,
        outcome: &str,
        ended_ms: u64,
        announce_attempts: u8,
    ) {
        write_state(
            dir,
            &JobRecord {
                id,
                owner: OWNER.to_string(),
                command: "cargo build".to_string(),
                started_ms: ended_ms.saturating_sub(1_000),
                phase: JobPhase::Settled,
                ended_ms: Some(ended_ms),
                outcome: Some(outcome.to_string()),
                exit_code: Some(0),
                output_file: Some(OUTPUT_FILE.to_string()),
                partial_file: Some(PARTIAL_FILE.to_string()),
                announce_attempts,
                announced_boot: None,
                kind: JournalKind::Bash,
                pty_session_id: None,
                pid: None,
                process_created_at_ms: None,
                tombstone: None,
            },
        );
    }

    /// A terminal row nobody has tried to announce yet.
    fn seed_settled(dir: &std::path::Path, id: u64, outcome: &str, ended_ms: u64) {
        seed_settled_attempted(dir, id, outcome, ended_ms, 0);
    }

    /// The promise the spawn receipt makes is "you will hear when it finishes".
    /// A daemon that dies inside the announcer's 0/30/120s ladder leaves a
    /// `Settled` row nobody was told about, and before the handback that
    /// promise was withdrawn in silence with the answer sitting on disk.
    ///
    /// RED without the handback arm in `init_and_reconcile`: nothing is claimed.
    #[test]
    fn a_completion_that_was_never_announced_is_handed_back_once() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        seed_settled(tmp.path(), 21, "completed", now_ms());

        init_and_reconcile(tmp.path().to_path_buf());
        let handed = take_undelivered_settled();
        assert_eq!(handed.len(), 1, "the undelivered completion must come back");
        assert_eq!(handed[0].record.id, 21);

        // Draining is destructive within a boot...
        assert!(
            take_undelivered_settled().is_empty(),
            "one delivery per boot, however many callers ask"
        );
        // The reconcile that queued it recorded an ATTEMPT, not a delivery.
        assert_eq!(
            lookup(21, Some(OWNER))
                .expect("row")
                .record
                .announce_attempts,
            1,
            "queueing a handback is an attempt"
        );
        assert!(
            lookup(21, Some(OWNER))
                .expect("row")
                .record
                .announced_boot
                .is_none(),
            "and nothing may claim delivery before the broadcast returns"
        );
        // BT-D-R4-16: the stamp lives in `init_and_announce`, AFTER the
        // broadcast succeeds, so a crash mid-broadcast retries the handback
        // instead of silently losing it. Simulate that successful announce...
        record_announced(21);
        // ...and with the stamp landed on disk, the NEXT boot stays quiet. A
        // handback that repeated forever would be worse than the silence it
        // replaced.
        disable_for_test();
        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            take_undelivered_settled().is_empty(),
            "a restart must not re-announce a completion it already handed back"
        );
        assert!(lookup(21, Some(OWNER))
            .expect("row")
            .record
            .announced_boot
            .is_some());
        disable_for_test();
    }

    /// The bound on "the next boot retries it". A row the last three boots all
    /// failed to deliver stops asking for a proactive turn — it stays on disk
    /// and stays poll-able, which is the difference between giving up on the
    /// notice and giving up on the answer.
    ///
    /// RED without the [`MAX_ANNOUNCE_ATTEMPTS`] arm: a crash-looping daemon
    /// re-queues the same completion at every boot for the whole freshness
    /// window.
    #[test]
    fn a_completion_stops_being_offered_after_three_attempts() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        seed_settled_attempted(
            tmp.path(),
            26,
            "completed",
            now_ms(),
            MAX_ANNOUNCE_ATTEMPTS - 1,
        );

        // One attempt left: handed back, and the attempt is spent on disk.
        init_and_reconcile(tmp.path().to_path_buf());
        assert_eq!(take_undelivered_settled().len(), 1);
        assert_eq!(
            lookup(26, Some(OWNER))
                .expect("row")
                .record
                .announce_attempts,
            MAX_ANNOUNCE_ATTEMPTS
        );
        disable_for_test();

        // None left: the next boot leaves it alone...
        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            take_undelivered_settled().is_empty(),
            "an undeliverable completion may not drive a turn at every boot forever"
        );
        // ...without pretending it was ever delivered, and without becoming
        // unreadable: `poll` still answers for it.
        let row = lookup(26, Some(OWNER)).expect("the row is still readable");
        assert!(row.record.announced_boot.is_none());
        assert_eq!(row.record.announce_attempts, MAX_ANNOUNCE_ATTEMPTS);
        assert_eq!(row.record.phase, JobPhase::Settled);
        disable_for_test();
    }

    /// The stamp the announcer's success arm writes: from there on the session
    /// knows, and no later boot may say it again.
    #[test]
    fn record_announced_survives_a_restart() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(22, "make", Some(OWNER));
        record_settled(22, Verdict::Completed, Some(0), "done\n", "");
        assert!(lookup(22, Some(OWNER))
            .expect("row")
            .record
            .announced_boot
            .is_none());
        record_announced(22);
        assert!(lookup(22, Some(OWNER))
            .expect("row")
            .record
            .announced_boot
            .is_some());
        disable_for_test();

        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            lookup(22, Some(OWNER))
                .expect("row")
                .record
                .announced_boot
                .is_some(),
            "the stamp is durable or it is useless"
        );
        assert!(take_undelivered_settled().is_empty());
        disable_for_test();
    }

    /// A killed job is the owner's own action, so its outcome is not news.
    /// Nothing ever stamps those rows announced, so without the verdict test
    /// every `kill` would queue an announce for the next boot — forever.
    #[test]
    fn a_killed_job_is_never_handed_back() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        seed_settled(tmp.path(), 23, "killed", now_ms());

        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            take_undelivered_settled().is_empty(),
            "you asked for it to stop; a restart does not make that news"
        );
        let row = lookup(23, Some(OWNER)).expect("row");
        assert!(
            row.record.announced_boot.is_none(),
            "and it must not be stamped either — claiming it was announced \
             would be a lie about a notice nobody sent"
        );
        assert_eq!(
            row.record.announce_attempts, 0,
            "nor may an attempt be spent on a notice this boot never intended to send"
        );
        disable_for_test();
    }

    /// "Your build finished" is worth a model turn minutes later and is noise a
    /// day later, when the row is still readable through `poll` anyway. The age
    /// test also bounds the one-off cost of `announced` defaulting to false:
    /// without it, the first boot after this field shipped would announce every
    /// completed job inside the whole retention window.
    ///
    /// Stale rows are deliberately left UNSTAMPED — the age test only ever gets
    /// truer, so they cannot come round again, and stamping them would record a
    /// notice that was never sent.
    #[test]
    fn a_stale_completion_is_left_alone_rather_than_announced_or_falsely_stamped() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        let long_ago = now_ms() - ANNOUNCE_HANDBACK_MAX_AGE_MS - 60_000;
        seed_settled(tmp.path(), 24, "completed", long_ago);

        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            take_undelivered_settled().is_empty(),
            "yesterday is not news"
        );
        let row = lookup(24, Some(OWNER)).expect("the row is still readable");
        assert!(row.record.announced_boot.is_none());
        assert_eq!(row.record.announce_attempts, 0);
        assert_eq!(row.record.phase, JobPhase::Settled, "and still poll-able");
        disable_for_test();
    }

    /// An interrupted row is a statement about the previous daemon and, at
    /// most, about its OS process — never about the command. Announcing
    /// "interrupted" would spend a turn on a verdict nobody reached, so those
    /// rows stay poll-only, which is the recorded decision.
    #[test]
    fn an_interrupted_job_is_not_handed_back() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(25, "cargo build", Some(OWNER));
        disable_for_test();

        init_and_reconcile(tmp.path().to_path_buf());
        assert_eq!(
            lookup(25, Some(OWNER)).expect("row").record.phase,
            JobPhase::Interrupted
        );
        assert!(take_undelivered_settled().is_empty());
        disable_for_test();
    }

    /// A job that finished normally comes back with its recorded output and
    /// exit code — and is NOT rewritten to `Interrupted` by the next boot.
    #[test]
    fn a_settled_job_keeps_its_output_across_a_restart() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(7, "make", Some(OWNER));
        record_settled(7, Verdict::Completed, Some(0), "line one\nline two\n", "");
        disable_for_test();

        init_and_reconcile(tmp.path().to_path_buf());
        let found = lookup(7, Some(OWNER)).expect("terminal rows are retained");
        assert_eq!(found.record.phase, JobPhase::Settled);
        assert_eq!(found.record.outcome.as_deref(), Some("completed"));
        assert_eq!(found.record.exit_code, Some(0));
        // (b) newlines are preserved — a stdout trail is not a progress note.
        assert!(
            found.recorded_output.contains("line one\nline two"),
            "line structure must survive the trail: {:?}",
            found.recorded_output
        );
        disable_for_test();
    }

    /// §5.1 — this file is a new egress for command output, read by a LATER
    /// process. Asserted on the RAW BYTES on disk: masking only the value
    /// returned to the caller would pass any assertion on `recorded_output`.
    #[test]
    fn the_output_trail_is_redacted_before_it_reaches_the_disk() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345";
        record_spawn(11, &format!("curl -H 'x: {secret}'"), Some(OWNER));
        record_settled(
            11,
            Verdict::Completed,
            Some(0),
            &format!("token={secret}\nsecond line\n"),
            "",
        );

        let trail = std::fs::read_to_string(tmp.path().join("job-11").join(OUTPUT_FILE)).unwrap();
        assert!(
            !trail.contains("abcdefghijklmnopqrstuvwxyz"),
            "a credential must never land in the journal: {trail}"
        );
        assert!(trail.contains("REDACTED"), "got: {trail}");
        // ...and the command preview takes the same gate.
        let state = std::fs::read_to_string(tmp.path().join("job-11").join(STATE_FILE)).unwrap();
        assert!(
            !state.contains("abcdefghijklmnopqrstuvwxyz"),
            "the command line is credential-bearing too: {state}"
        );
        disable_for_test();
    }

    /// The journal is process-global, so its read face has to be scoped exactly
    /// like the live table — and an unowned row would be readable by everyone,
    /// so it is never written in the first place.
    #[test]
    fn owner_scoping_refuses_none_and_refuses_a_foreign_owner() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        record_spawn(21, "sleep 9", Some(OWNER));
        assert!(lookup(21, Some(OWNER)).is_some(), "the owner can read it");
        assert!(
            lookup(21, Some("someone-else")).is_none(),
            "another session must not read this job out of the journal"
        );
        assert!(
            lookup(21, None).is_none(),
            "an unscoped caller must not read every session's job history"
        );
        assert!(
            list_for_scope(None, &[]).is_empty(),
            "the enumeration face must be scoped like the by-id face"
        );

        // An unowned job is refused at the write side, so it cannot become a
        // row every later caller can read.
        record_spawn(22, "sleep 9", None);
        assert!(lookup(22, Some(OWNER)).is_none());
        assert!(
            !tmp.path().join("job-22").exists(),
            "an unowned job must not be journaled at all"
        );
        disable_for_test();
    }

    /// Persistence is opt-in. With no store dir configured nothing is written
    /// and every read answers empty — the pre-existing behaviour, byte for byte.
    #[test]
    fn every_entry_point_is_a_no_op_while_the_journal_is_off() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        // Enable then disable so the assertion below is about *these calls*
        // writing nothing, not about a directory nobody was ever pointed at.
        enable_for_test(tmp.path().to_path_buf());
        disable_for_test();

        reserve_id(99);
        record_spawn(99, "echo hi", Some(OWNER));
        record_partial(99, "[stdout]\nhi\n");
        record_settled(99, Verdict::Completed, Some(0), "hi\n", "");
        assert!(lookup(99, Some(OWNER)).is_none());
        assert!(list_for_scope(Some(OWNER), &[]).is_empty());
        assert_eq!(id_floor(), 1, "the allocator keeps its historical start");
        assert!(!is_enabled());
        // Nothing touched the filesystem.
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            0,
            "a disabled journal must not write anything"
        );
    }

    /// The wire the interrupted population depends on entirely: enabling the
    /// journal must also start the flusher.
    ///
    /// Without it every tombstone is guaranteed empty and nothing anywhere
    /// reports a problem — the rows are written, the reads succeed, the answer
    /// is just permanently "no output was recorded". That is the exact shape
    /// this subsystem already shipped once (`kill_all_running_background` with
    /// zero callers for three days), so it gets a test rather than a comment.
    #[tokio::test]
    async fn enabling_the_journal_starts_the_live_tail_flusher() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            FLUSHER_STARTED.load(Ordering::SeqCst),
            "the boot reconcile must start the flusher — a journal without one \
             writes tombstones that can never carry output"
        );
        disable_for_test();
    }

    /// The gap this round closes: a job that never reached a terminal state in
    /// the daemon that ran it used to come back as a tombstone with nothing in
    /// it. Now the periodic capture is what it left behind, and it survives the
    /// restart that killed the job.
    #[test]
    fn an_interrupted_job_comes_back_with_what_it_had_printed() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(41, "cargo build --release", Some(OWNER));
        record_partial(41, "[stdout]\n   Compiling alephcore v26.7.31\n");
        // SIGKILL: no terminal write, the process simply vanishes.
        disable_for_test();

        init_and_reconcile(tmp.path().to_path_buf());
        let found = lookup(41, Some(OWNER)).expect("the row survives");
        assert_eq!(found.record.phase, JobPhase::Interrupted);
        assert!(
            found.recorded_output.contains("Compiling alephcore"),
            "the tombstone must carry what the job had produced: {:?}",
            found.recorded_output
        );
        assert!(
            found.output_is_live_capture,
            "a mid-run window must be flagged as one — read as a result it says \
             the build got that far and stopped cleanly"
        );
        assert!(
            found.last_activity_ms >= found.record.started_ms,
            "the capture carries its own stamp, not the row's start time"
        );
        disable_for_test();
    }

    /// The doubling trap: two producers describing the same job.
    ///
    /// The finished-path trail and the live capture would both be "this job's
    /// output" in one append-only file. They are two files precisely so this
    /// question has an answer, and the answer is that a RESULT always beats a
    /// window taken partway through producing it.
    #[test]
    fn a_completed_job_prefers_its_result_over_the_stale_live_capture() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(42, "make", Some(OWNER));
        record_partial(42, "[stdout]\nhalfway through\n");
        record_settled(42, Verdict::Completed, Some(0), "the final answer\n", "");

        let found = lookup(42, Some(OWNER)).expect("row");
        assert!(found.recorded_output.contains("the final answer"));
        assert!(
            !found.recorded_output.contains("halfway through"),
            "the mid-run window must not be concatenated onto the result: {:?}",
            found.recorded_output
        );
        assert!(!found.output_is_live_capture);
        disable_for_test();
    }

    /// §5.1 again, for the second egress this round adds. The capture is
    /// gated before it gets here, but the reader is still a LATER process, so
    /// the unconditional masker applies to it exactly as to the trail.
    #[test]
    fn the_live_capture_is_redacted_before_it_reaches_the_disk() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345";
        record_spawn(43, "deploy", Some(OWNER));
        record_partial(43, &format!("[stdout]\nusing token={secret}\n"));

        let raw = std::fs::read_to_string(tmp.path().join("job-43").join(PARTIAL_FILE)).unwrap();
        assert!(
            !raw.contains("abcdefghijklmnopqrstuvwxyz"),
            "a credential must never land in the capture: {raw}"
        );
        assert!(raw.contains("REDACTED"), "got: {raw}");
        disable_for_test();
    }

    /// A row whose `state.json` will not parse is deliberately left on disk to
    /// be diagnosed — so its id is still CLAIMED, and a floor derived only from
    /// parsed rows walks straight over it. The next daemon would then reissue
    /// that id onto a directory that already exists.
    #[test]
    fn an_unparseable_row_still_raises_the_id_floor() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        // No watermark either: this is the store whose watermark write failed.
        std::fs::create_dir_all(tmp.path().join("job-57")).unwrap();
        std::fs::write(tmp.path().join("job-57").join(STATE_FILE), b"{ truncated").unwrap();

        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            lookup(57, Some(OWNER)).is_none(),
            "an unreadable row cannot be served"
        );
        assert!(
            id_floor() > 57,
            "...but its id was still handed out once, so it must not come round \
             again — floor was {}",
            id_floor()
        );
        disable_for_test();
    }

    /// The belt for an id that repeats anyway. `write_state` overwrites, but
    /// the trail APPENDS — so without this a new owner inherits the previous
    /// owner's output. That is a cross-session leak, not a numbering blemish.
    #[test]
    fn a_reissued_id_never_inherits_the_previous_owners_output() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        record_spawn(61, "print-the-secrets", Some(OWNER));
        record_settled(61, Verdict::Completed, Some(0), "FIRST-OWNERS-OUTPUT\n", "");
        record_partial(61, "[stdout]\nFIRST-OWNERS-CAPTURE\n");

        // The floor failed and id 61 comes round again, for someone else.
        const OTHER: &str = "{\"Ephemeral\":{\"agent_id\":\"b\",\"ephemeral_id\":\"k\"}}";
        record_spawn(61, "innocent command", Some(OTHER));

        let found = lookup(61, Some(OTHER)).expect("the new owner's row");
        assert!(
            !found.recorded_output.contains("FIRST-OWNERS"),
            "the new owner must not be handed another session's output: {:?}",
            found.recorded_output
        );
        assert!(found.recorded_output.is_empty());
        assert!(
            lookup(61, Some(OWNER)).is_none(),
            "and the row is no longer the first owner's either"
        );
        disable_for_test();
    }

    /// Retention is a boot-only sweep: a terminal row older than the window is
    /// removed, a fresh one is not.
    #[test]
    fn boot_prunes_terminal_rows_past_the_retention_window() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());
        record_spawn(31, "old", Some(OWNER));
        record_settled(31, Verdict::Completed, Some(0), "", "");
        record_spawn(32, "fresh", Some(OWNER));
        record_settled(32, Verdict::Completed, Some(0), "", "");
        disable_for_test();

        // Age row 31 past the window by rewriting its stamp on disk.
        let path = tmp.path().join("job-31").join(STATE_FILE);
        let mut aged: JobRecord = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        aged.ended_ms = Some(now_ms() - RECORD_RETENTION_MS - 1);
        std::fs::write(&path, serde_json::to_vec(&aged).unwrap()).unwrap();

        init_and_reconcile(tmp.path().to_path_buf());
        assert!(lookup(31, Some(OWNER)).is_none(), "aged row must be swept");
        assert!(!tmp.path().join("job-31").exists());
        assert!(lookup(32, Some(OWNER)).is_some(), "fresh row must survive");
        // The swept id must still not come round again.
        assert!(id_floor() > 31, "retention must not lower the id floor");
        disable_for_test();
    }
}
