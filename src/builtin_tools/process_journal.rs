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
//!   [`crate::utils::atomic_io::write_atomic`], exactly twice per job — spawn
//!   and terminal) plus an append-only `output.txt` trail;
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
//! **1. There is no pid, so there is no liveness probe.**
//! `background_persistence` may assert "every `Running` record at boot is an
//! orphan" because its runs are in-process `tokio` tasks: if the process is
//! gone, the run is gone. A background `bash` job is a **real OS process**. It
//! is spawned with `kill_on_drop(true)`, so an orderly teardown reaps it — but
//! a `SIGKILL`ed daemon never drops anything, and the child can outlive it.
//! Nothing in [`ProcEntry`](super::process_registry) records the child's pid
//! (it holds only an `AbortHandle`), and plumbing one through was **decided
//! against for this round**. The recovered row therefore has to say something
//! stronger and more honest than the sub-agent one:
//! [`JobPhase::Interrupted`] renders as `interrupted_by_restart_liveness_unknown`
//! and carries an advisory stating that Aleph no longer holds a handle and
//! **did not check** whether the OS process is still alive. It is never
//! reported as a failure — nothing about the command was judged.
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
/// the one-off cost of the `announced` flag being `#[serde(default)]` — without
/// it, the first boot after this field shipped would announce every completed
/// job inside the whole 7-day retention window. Rows older than this are left
/// **unstamped**: claiming they were announced would be a lie, and the age test
/// only ever gets truer, so they cannot come round again.
const ANNOUNCE_HANDBACK_MAX_AGE_MS: u64 = 60 * 60 * 1000;

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
    Interrupted,
}

impl JobPhase {
    /// Wire label handed to the model.
    ///
    /// `Interrupted` says more than the sub-agent sidecar's
    /// `interrupted_by_restart` on purpose: a `bash` child is a real OS process
    /// that can outlive a `SIGKILL`ed daemon, and this module records no pid,
    /// so it cannot and does not probe whether the process is still alive.
    /// Neither label reads as a failure.
    #[must_use]
    pub const fn status_label(self) -> &'static str {
        match self {
            Self::Running => "running_unconfirmed",
            Self::Settled => "recorded",
            Self::Interrupted => "interrupted_by_restart_liveness_unknown",
        }
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
    /// Whether the owning session was ever *told* this job finished.
    ///
    /// `phase` answers "did it finish"; this answers "does anyone know". The
    /// gap between them is a real window: `bash_exec` broadcasts the completion
    /// after the row is settled, and `gateway::process_announce` retries at
    /// 0/30/120s while the session is busy. A daemon that dies inside those two
    /// and a half minutes leaves a `Settled` row nobody was told about, and the
    /// promise the spawn receipt makes is withdrawn in silence with the result
    /// sitting on disk. [`take_undelivered_settled`] is the reader.
    ///
    /// `#[serde(default)]` makes every pre-existing row read as *not*
    /// announced, which is the fail-safe direction (duplicate-visible beats
    /// loss-silent); the boot handback's freshness window keeps that from
    /// turning an upgrade into a week of stale notices.
    #[serde(default)]
    pub announced: bool,
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
/// cannot distinguish "never ran" from "ran and the write was lost".
///
/// This function itself **broadcasts nothing** — an interrupted row drives no
/// proactive turn, it is simply there the next time the model polls, so this
/// half has no ordering dependency on any event subscriber. It does claim the
/// second recovered population on the way past: a row that reached `Settled`
/// with `announced` still false is a completion whose notice died with the
/// previous daemon, and [`take_undelivered_settled`] hands those to
/// [`init_and_announce`], which is the half that does have an ordering
/// dependency.
///
/// Idempotent. Returns 0 when the directory cannot be created — persistence
/// stays off rather than failing boot (P7).
pub fn init_and_reconcile(dir: PathBuf) -> usize {
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
                ..record
            };
            write_state(&dir, &tombstone);
            tombstoned += 1;
            index.insert(tombstone.id, tombstone);
        } else if is_undelivered_completion(&record, now) {
            // BT-D-R4-16: stamp the row announced=true only AFTER the
            // boot-time broadcast succeeds, not in this reconcile pass.
            // Previously we stamped here "so a second restart before the
            // parent turn lands repeats it forever", but the stamp ran
            // BEFORE the actual broadcast in init_and_announce. If this
            // daemon crashed between the stamp and the broadcast, the
            // completion was marked delivered on disk but never delivered
            // — silent loss. Now we leave announced=false on disk, push
            // the record to the handback queue, and let init_and_announce
            // call record_announced() after broadcast returns Ok. A
            // crash mid-broadcast leaves the row stamped=false, and the
            // next boot retries the handback (no silent loss).
            undelivered.push(record.clone());
            index.insert(record.id, record);
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
        // BT-D-R4-16: stamp announced=true AFTER broadcast returns. A
        // crash mid-broadcast leaves the row stamped=false on disk so
        // the next boot retries the handback. Previously the stamp ran
        // in init_and_reconcile (before this broadcast was reached),
        // which silently lost completions on a crash in the gap.
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
///   daemon*, and this module deliberately makes no claim about whether that
///   job's OS process is still alive; announcing "it was interrupted, liveness
///   unknown" would spend a turn on a verdict nobody reached. Those rows stay
///   poll-able, which is the recorded decision.
/// * `outcome == completed` — a killed job is the owner's own action, so its
///   outcome is not news (the same stance `subagent_tool::spawn` takes for a
///   cancelled child). Without this test every `kill` would queue an announce
///   for the next boot, since nothing ever stamps those rows announced.
/// * fresh — see [`ANNOUNCE_HANDBACK_MAX_AGE_MS`].
fn is_undelivered_completion(record: &JobRecord, now: u64) -> bool {
    record.phase == JobPhase::Settled
        && !record.announced
        && record.outcome.as_deref() == Some(Verdict::Completed.label())
        && record
            .ended_ms
            .is_some_and(|t| now.saturating_sub(t) <= ANNOUNCE_HANDBACK_MAX_AGE_MS)
}

/// Drain the completions [`init_and_reconcile`] found undelivered, hydrated
/// with whatever output was recorded for them.
///
/// Destructive: within a boot this is the one chance to deliver them. The
/// rows are stamped `announced` on disk only by `init_and_announce` AFTER the
/// broadcast succeeds (BT-D-R4-16), so a boot that drains without announcing
/// hands the completion back again next boot. Empty for every boot that had
/// nothing to hand back, which is the overwhelming majority.
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
        announced: false,
    };
    write_state(&dir, &record);
    index_lock().insert(id, record);
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

/// Stamp "the owning session was told about this job".
///
/// Written by the announcer's success arm (and by the boot handback, in the
/// same pass that hands a row over). Without it, a restart inside the retry
/// ladder re-delivers a notice the session already received — forever, since
/// nothing else ever clears the condition.
///
/// No-op for an id this process never journaled.
pub(crate) fn record_announced(id: u64) {
    let Some(dir) = store_dir() else { return };
    let record = {
        let mut index = index_lock();
        let Some(record) = index.get_mut(&id) else {
            return;
        };
        if record.announced {
            return;
        }
        record.announced = true;
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
        let label = found.record.phase.status_label();
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
    // The `announced` stamp and the boot handback
    // ========================================================================

    /// Write a terminal row directly, so a test can choose its age and verdict
    /// — the two things the handback filter reads and neither of which the
    /// normal write path lets you pick.
    fn seed_settled(dir: &std::path::Path, id: u64, outcome: &str, ended_ms: u64) {
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
                announced: false,
            },
        );
    }

    /// The promise the spawn receipt makes is "you will hear when it finishes".
    /// A daemon that dies inside the announcer's 0/30/120s ladder leaves a
    /// `Settled` row nobody was told about, and before the `announced` field
    /// that promise was withdrawn in silence with the answer sitting on disk.
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
        // BT-D-R4-16: reconcile no longer stamps `announced` on disk — the
        // stamp moved to `init_and_announce`, AFTER the broadcast succeeds,
        // so a crash mid-broadcast retries the handback instead of silently
        // losing it. Simulate that successful announce here...
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
        assert!(lookup(21, Some(OWNER)).expect("row").record.announced);
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
        assert!(!lookup(22, Some(OWNER)).expect("row").record.announced);
        record_announced(22);
        assert!(lookup(22, Some(OWNER)).expect("row").record.announced);
        disable_for_test();

        init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            lookup(22, Some(OWNER)).expect("row").record.announced,
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
        assert!(
            !lookup(23, Some(OWNER)).expect("row").record.announced,
            "and it must not be stamped either — claiming it was announced \
             would be a lie about a notice nobody sent"
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
        assert!(!row.record.announced);
        assert_eq!(row.record.phase, JobPhase::Settled, "and still poll-able");
        disable_for_test();
    }

    /// An interrupted row makes no claim about whether its OS process is still
    /// alive — this module records no pid and does not probe. Announcing
    /// "interrupted, liveness unknown" would spend a turn on a verdict nobody
    /// reached, so those rows stay poll-only, which is the recorded decision.
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
