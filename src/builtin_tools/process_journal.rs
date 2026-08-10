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
//! Only a job that reaches a **natural completion** contributes output: the
//! `CodeExecOutput` it produced has already been through the sandbox's
//! `scrub_and_gate_output` (secret redaction + block-class gate), so it is safe
//! to persist. A job's *live* tail is deliberately **not** flushed here: those
//! bytes are PRE-scrub, and writing them to a file that a later process reads
//! back would make "restart the daemon" a way to read what the finished path
//! refuses to return. Killed and interrupted rows therefore carry no output,
//! and the renderer says so explicitly rather than showing an empty string.
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
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::exec::masker::SecretMasker;
use crate::sync_primitives::{Mutex, MutexGuard};

/// State file name inside a job's directory.
const STATE_FILE: &str = "state.json";
/// Append-only output trail file name inside a job's directory.
const OUTPUT_FILE: &str = "output.txt";
/// Id high-water mark, at the store root (not inside a job directory).
const ID_WATERMARK_FILE: &str = "id_watermark.json";

/// How long a terminal row is kept on disk. Pruned at boot only — the sweep
/// walks the whole directory, so doing it per-write would turn every spawn
/// into an O(jobs) stat storm. Same window as the sub-agent sidecar.
const RECORD_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Bytes of output trail retained per job when reading it back. Matches the
/// live tail's budget so "poll a running job" and "poll a job the previous
/// daemon ran" hand back the same amount of text.
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

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
}

impl JobRecord {
    /// Absolute path of this job's output trail, given the journal root.
    #[must_use]
    pub fn output_path(&self, dir: &Path) -> Option<PathBuf> {
        let file = self.output_file.as_ref()?;
        Some(job_dir(dir, self.id).join(file))
    }
}

/// A journal row plus whatever output was recorded for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredJob {
    pub record: JobRecord,
    /// Tail of the (already-masked) output trail. Empty when nothing was
    /// recorded — which the renderer must state, not paper over.
    pub recorded_output: String,
    /// Unix ms of the last recorded trail line, or `started_ms` when there was
    /// none.
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
/// Unlike the sub-agent reconcile, this writes tombstones and **broadcasts
/// nothing** — there is no proactive turn to drive, the rows are simply there
/// the next time the model polls. It therefore has no ordering dependency on
/// any event subscriber.
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
    // Every id the journal has ever *shown*, retention-swept rows included:
    // their ids were handed out, so they must not come round again.
    let mut highest_row_id = 0u64;

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

    // Seeded here rather than at the boot call site so no future boot path can
    // enable the journal and forget the allocator — a resurrected row and a
    // live job sharing one id is the failure this pairs with.
    super::process_registry::process_registry().seed_id_floor(id_floor());

    if tombstoned > 0 {
        tracing::info!(
            interrupted = tombstoned,
            "process_journal: tombstoned background bash jobs left running by a previous process"
        );
    }
    tombstoned
}

/// Lowest id a fresh registry may hand out: one past everything ever reserved.
/// `1` while persistence is off, i.e. the allocator's historical start.
pub(crate) fn id_floor() -> u64 {
    reserved_lock().saturating_add(1)
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
/// after a restart — but so does the row write, so there is no row for them to
/// collide with.
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
    };
    write_state(&dir, &record);
    index_lock().insert(id, record);
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
    dir.join(format!("job-{id}"))
}

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

struct Trail {
    text: String,
    last_ms: Option<u64>,
}

/// Read back the tail of a job's output trail, restoring line structure.
///
/// The trail is located through [`JobRecord::output_path`], i.e. the name the
/// row itself carries — not by re-deriving the layout convention here.
fn read_trail(dir: &Path, record: &JobRecord) -> Trail {
    let empty = || Trail {
        text: String::new(),
        last_ms: None,
    };
    let Some(path) = record.output_path(dir) else {
        return empty();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return empty();
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
    // Keep the TAIL, not the head: what the job printed last is the actionable
    // part. UTF-8 safe by character boundary (P7) — walking backwards the
    // predicate holds for every index in the tail, so the boundary wanted is
    // the SMALLEST satisfying index (`take_while(..).last()`, never `find`).
    let text = if rendered.len() > OUTPUT_TAIL_BYTES {
        let start = rendered
            .char_indices()
            .rev()
            .map(|(i, _)| i)
            .take_while(|i| rendered.len() - i <= OUTPUT_TAIL_BYTES)
            .last()
            .unwrap_or(0);
        format!("…{}", &rendered[start..])
    } else {
        rendered
    };
    Trail { text, last_ms }
}

fn hydrate(dir: &Path, record: JobRecord) -> RecoveredJob {
    let trail = read_trail(dir, &record);
    RecoveredJob {
        last_activity_ms: trail.last_ms.unwrap_or(record.started_ms),
        recorded_output: trail.text,
        record,
    }
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

/// Test-only: point the journal at `dir` without running the boot reconcile,
/// and drop whatever a previous test left in the index.
#[cfg(test)]
pub(crate) fn enable_for_test(dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("test store dir");
    *store_lock() = Some(dir);
    index_lock().clear();
    *reserved_lock() = 0;
}

/// Test-only: turn persistence back off so unrelated tests keep their zero-I/O
/// behaviour once this test's tempdir is gone.
#[cfg(test)]
pub(crate) fn disable_for_test() {
    *store_lock() = None;
    index_lock().clear();
    *reserved_lock() = 0;
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
