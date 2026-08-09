//! Cross-process persistence for background sub-agents.
//!
//! [`BackgroundAgentTracker`](super::background_tracker::BackgroundAgentTracker)
//! is pure process memory: a `HashMap` behind a `OnceLock`, nothing on disk. A
//! daemon restart therefore erased every in-flight background sub-agent, and
//! the parent session's next `check_status` got
//! `"No background sub-agent found with request_id '…'"` with
//! `retryable: false` — a message that cannot distinguish *"you mistyped the
//! id"* from *"your child died with the previous daemon incarnation"*, and that
//! throws away whatever the child had produced before it vanished.
//!
//! This module is the sidecar that closes both gaps. Its shape is deliberately
//! copied from two things that already exist in this repo rather than invented:
//!
//! * [`crate::builtin_tools::scratchpad_registry`] — an in-memory process-global
//!   table mirrored write-through to disk via [`crate::utils::atomic_io`] and
//!   reloaded at boot. Persistence is **opt-in**: until [`init_and_reconcile`]
//!   runs, every entry point here is a zero-I/O no-op and the tracker behaves
//!   exactly as before.
//! * `swarm::tasks::store::runs::abandon_orphaned_runs` — the boot reconcile
//!   writes a **terminal state** for orphans instead of deleting the row. A
//!   mechanism that only records "it finished" cannot tell "it never ran" apart
//!   from "it ran and the write was lost"; a tombstone can.
//!
//! ## Layout
//!
//! ```text
//! <dir>/<slug>/state.json    # PersistedRun, atomically rewritten at start + terminal
//! <dir>/<slug>/result.txt    # append-only "<unix_ms>\t<text>" activity trail
//! ```
//!
//! `state.json` is written exactly twice per run (start, terminal), so the
//! atomic tempfile+fsync+rename cost is bounded. The activity trail is appended
//! instead, which is also where `last_activity` comes from: the timestamp of the
//! last line, single-sourced rather than duplicated into `state.json` (a field
//! that had to be rewritten on every progress event would cost one fsync per
//! tool call).
//!
//! ## Redaction (mandatory — this is a new egress)
//!
//! `result.txt` is the **first place a sub-agent's output crosses a process
//! boundary onto disk**, and it is re-injected into a fresh parent turn at the
//! next boot (which can fan out to a chat channel). Unattended redaction was
//! previously wired only into `TraceSink`, and the run's output escaped in the
//! clear down the `EventEmitter` leg — one masked copy plus one clear copy is
//! not redaction. So every byte written here goes through the same
//! [`SecretMasker`](crate::exec::masker::SecretMasker) the two existing legs
//! share, unconditionally: an artifact that outlives the process cannot be
//! gated on the run's attendedness, because the *reader* is a later process.
//!
//! R10 note: this is scaffolding, not cognition. It answers the mechanical
//! question "did this request_id exist in a previous process, and what had it
//! produced?". It makes no judgement about the work.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::exec::masker::SecretMasker;
use crate::sync_primitives::Mutex;

/// State file name inside a run's directory.
const STATE_FILE: &str = "state.json";
/// Append-only activity trail file name inside a run's directory.
const RESULT_FILE: &str = "result.txt";

/// How long a terminal record is kept on disk. Pruned at boot only — the
/// sweep walks the whole directory, so doing it per-write would turn every
/// spawn into an O(runs) stat storm.
const RECORD_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Bytes of activity trail retained per run when reading it back. A partial
/// result is evidence of what the child was doing, not a payload — the parent
/// re-delegates from it, it does not consume it. Bounded so a chatty child
/// cannot inflate the parent's next prompt without limit.
const PARTIAL_RESULT_TAIL_BYTES: usize = 8 * 1024;

/// Hard cap on one appended activity line. Progress previews are already
/// bounded at 200 chars upstream, but a terminal `final_text` is not.
const MAX_LINE_CHARS: usize = 4_000;

/// Shared masker. `SecretMasker` is a zero-sized handle — both the vendor floor
/// and the operator's `[[security.mask_patterns]]` live in process-wide statics
/// inside `exec::masker` — so this exists only to avoid re-constructing the
/// empty wrapper per line, and it inherits configured patterns without knowing
/// they exist.
static MASKER: LazyLock<SecretMasker> = LazyLock::new(SecretMasker::new);

/// Root directory for the sidecar. `None` = persistence disabled (every entry
/// point is a no-op), which is the state in CLI processes, tests, and any
/// embedding that never calls [`init_and_reconcile`].
static STORE_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

/// Records visible to [`lookup`]: everything loaded from disk at boot, plus
/// every run started in this process that has not reached a terminal state.
///
/// A run that settles in *this* process is dropped from the index because the
/// live tracker answers for it (non-destructively, until its own TTL); leaving
/// a duplicate here would give two different answers for one id depending on
/// which side was consulted first. Its tombstone stays on disk for the
/// retention window, so the *next* boot still loads it.
static INDEX: LazyLock<Mutex<HashMap<String, PersistedRun>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Lifecycle phase of a persisted background run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    /// Registered and (as far as the writing process knew) still executing.
    Running,
    /// Reached a terminal outcome in the process that started it.
    Settled,
    /// Found `Running` on disk with no live process behind it — the daemon
    /// that owned it is gone. A verdict about the *process*, never about the
    /// work: see [`init_and_reconcile`].
    Abandoned,
}

impl RunPhase {
    /// Wire label handed to the model. `Abandoned` deliberately does not read
    /// as a failure: nothing about the task was judged, the daemon simply
    /// stopped existing underneath it.
    #[must_use]
    pub const fn status_label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Settled => "completed",
            Self::Abandoned => "interrupted_by_restart",
        }
    }
}

/// One background sub-agent as recorded on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRun {
    pub request_id: String,
    /// Owning top-level session key, used to scope [`lookup`] exactly the way
    /// the live tracker's addressing face is scoped. Without it a request_id
    /// learned any other way (announce echo, log line, paste) would read
    /// another session's output back out of the sidecar — the very leak
    /// `BackgroundAgentTracker::addressable` exists to prevent.
    pub root_session: String,
    pub task: String,
    /// Resolved agent / role name, when known.
    pub agent: String,
    pub started_ms: u64,
    pub phase: RunPhase,
    /// Unix ms at which the run reached `Settled` / `Abandoned`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// Terminal outcome label from the tracker, when it settled normally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// File name (NOT a path) of the activity trail inside this run's
    /// directory. Stored as a name so the record stays valid when `ALEPH_HOME`
    /// moves; resolve it with [`Self::partial_result_path`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_result_file: Option<String>,
}

impl PersistedRun {
    /// Absolute path of this run's activity trail, given the sidecar root.
    #[must_use]
    pub fn partial_result_path(&self, dir: &Path) -> Option<PathBuf> {
        let file = self.partial_result_file.as_ref()?;
        Some(dir.join(slug(&self.request_id)).join(file))
    }
}

/// A run recovered by the boot reconcile, with whatever it managed to produce.
#[derive(Debug, Clone)]
pub struct RecoveredRun {
    pub record: PersistedRun,
    /// Tail of the (already-masked) activity trail. Empty when the child never
    /// reported anything before the process died.
    pub partial_result: String,
    /// Unix ms of the last recorded activity, or `started_ms` when there was
    /// none. Distinct from `ended_ms`, which is when *this boot* tombstoned it.
    pub last_activity_ms: u64,
}

// ============================================================================
// Boot
// ============================================================================

/// Enable persistence at `dir` and reconcile whatever a previous process left
/// behind. Returns the runs that were tombstoned by this call.
///
/// The live set is empty by construction: this runs at boot, before any
/// sub-agent is spawned, so *every* `Running` record on disk is an orphan — the
/// same reasoning `abandon_orphaned_runs` uses when its live task-id list is
/// empty. Each orphan gets a terminal `Abandoned` state written over it (never
/// deleted): a record that only ever says "finished" cannot distinguish "never
/// ran" from "ran and the write was lost".
///
/// Idempotent. Returns an empty vec when the directory cannot be created —
/// persistence stays off rather than failing boot (P7).
pub fn init_and_reconcile(dir: PathBuf) -> Vec<RecoveredRun> {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, dir = %dir.display(), "background_persistence: disabled (cannot create store dir)");
        return Vec::new();
    }

    let now = now_ms();
    let mut recovered = Vec::new();
    let mut index = HashMap::new();

    for record in read_all(&dir) {
        // Retention sweep. Terminal records outlive their run so a restart can
        // still answer for them; they do not outlive the retention window.
        if record.phase != RunPhase::Running
            && record
                .ended_ms
                .is_some_and(|t| now.saturating_sub(t) > RECORD_RETENTION_MS)
        {
            let _ = std::fs::remove_dir_all(dir.join(slug(&record.request_id)));
            continue;
        }

        if record.phase == RunPhase::Running {
            let trail = read_trail(&dir, &record);
            let tombstone = PersistedRun {
                phase: RunPhase::Abandoned,
                ended_ms: Some(now),
                ..record.clone()
            };
            write_state(&dir, &tombstone);
            recovered.push(RecoveredRun {
                last_activity_ms: trail.last_ms.unwrap_or(tombstone.started_ms),
                partial_result: trail.text,
                record: tombstone.clone(),
            });
            index.insert(tombstone.request_id.clone(), tombstone);
        } else {
            index.insert(record.request_id.clone(), record);
        }
    }

    *store_lock() = Some(dir);
    *index_lock() = index;

    if !recovered.is_empty() {
        tracing::info!(
            orphans = recovered.len(),
            "background_persistence: tombstoned background sub-agents orphaned by a previous process"
        );
    }
    recovered
}

/// Boot entry point: reconcile, then tell each affected parent session **once**.
///
/// One notification per parent session, not one per orphan: a `SubAgentCompleted`
/// event drives a whole proactive parent turn (`gateway::subagent_announce`), so
/// N events for one session would be N runs of the same agent all saying "your
/// children died". The event is addressed to a session, so grouping any coarser
/// than per-session would have nowhere to go.
///
/// Returns the number of orphans reconciled.
pub async fn init_and_announce_orphans(dir: PathBuf) -> usize {
    let recovered = init_and_reconcile(dir);
    if recovered.is_empty() {
        return 0;
    }
    let total = recovered.len();

    let mut by_session: HashMap<String, Vec<RecoveredRun>> = HashMap::new();
    for run in recovered {
        if run.record.root_session.is_empty() {
            // No parent session to announce into; the record is still on disk
            // and `check_status` can still explain it.
            continue;
        }
        by_session
            .entry(run.record.root_session.clone())
            .or_default()
            .push(run);
    }

    for (session, runs) in by_session {
        let agent_id = crate::routing::session_key::SessionKey::from_key_string(&session)
            .map_or_else(|| "primary".to_string(), |k| k.agent_id().to_string());
        let summary = summarize_orphans(&runs);
        let event = crate::event::SubAgentCompletionEvent {
            agent_id: agent_id.clone(),
            child_session_id: runs
                .first()
                .map(|r| r.record.request_id.clone())
                .unwrap_or_default(),
            summary,
            success: false,
            error: Some(format!(
                "{} background sub-agent(s) were interrupted by a daemon restart",
                runs.len()
            )),
            // Deliberately `None`: the announce path's dedup asks the live
            // tracker whether this request_id was already consumed, and the
            // tracker has never heard of a pre-restart id. Supplying one would
            // pin the whole grouped notice to a single arbitrary child.
            request_id: None,
        };
        crate::event::GlobalBus::global()
            .broadcast(
                &agent_id,
                &session,
                crate::event::AlephEvent::SubAgentCompleted(event),
            )
            .await;
    }
    total
}

/// One human/model-readable block describing every orphan of one session.
fn summarize_orphans(runs: &[RecoveredRun]) -> String {
    let mut out = format!(
        "{} background sub-agent(s) were still running when the daemon last stopped. \
         They did not fail — their process disappeared. Partial progress below; \
         re-delegate anything still needed.\n",
        runs.len()
    );
    for run in runs {
        out.push_str(&format!(
            "\n- request_id: {}\n  task: {}\n  agent: {}\n",
            run.record.request_id, run.record.task, run.record.agent
        ));
        if !run.partial_result.is_empty() {
            out.push_str("  partial_result:\n");
            for line in run.partial_result.lines() {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

// ============================================================================
// Write path (called by BackgroundAgentTracker)
// ============================================================================

/// Record the start of a background sub-agent. No-op when persistence is off.
///
/// `started_ms` is stamped here rather than accepted from the caller: the
/// tracker stamps its own with the same clock, and a second parameter would be
/// a second answer to "when did this run begin".
pub fn record_start(request_id: &str, root_session: &str, task: &str, agent: &str) {
    let Some(dir) = store_dir() else { return };
    let started_ms = now_ms();
    if request_id.is_empty() {
        return;
    }
    let record = PersistedRun {
        request_id: request_id.to_string(),
        root_session: root_session.to_string(),
        // The task text is model-authored and can quote a credential it was
        // handed; it lands in the same file the trail does, so it takes the
        // same gate.
        task: mask_line(task),
        agent: agent.to_string(),
        started_ms,
        phase: RunPhase::Running,
        ended_ms: None,
        outcome: None,
        partial_result_file: Some(RESULT_FILE.to_string()),
    };
    write_state(&dir, &record);
    index_lock().insert(record.request_id.clone(), record);
}

/// Append one activity line to a run's trail. No-op when persistence is off or
/// the run was never registered here.
pub fn record_activity(request_id: &str, text: &str) {
    let Some(dir) = store_dir() else { return };
    if text.trim().is_empty() {
        return;
    }
    if !index_lock().contains_key(request_id) {
        return;
    }
    append_trail(&dir, request_id, text);
}

/// Record a terminal outcome. The tombstone stays on disk (retention window)
/// but leaves the in-memory index, because the live tracker now answers for
/// this id.
pub fn record_settled(request_id: &str, outcome: &str, final_text: &str) {
    let Some(dir) = store_dir() else { return };
    let Some(mut record) = index_lock().remove(request_id) else {
        return;
    };
    if !final_text.trim().is_empty() {
        append_trail(&dir, request_id, final_text);
    }
    record.phase = RunPhase::Settled;
    record.ended_ms = Some(now_ms());
    record.outcome = Some(outcome.to_string());
    write_state(&dir, &record);
}

// ============================================================================
// Read path (called by the subagent tool's not-found branches)
// ============================================================================

/// Look up a run this process does not know about, scoped to `scope` the same
/// way the live tracker's addressing face is.
///
/// `scope = None` (a caller with no session identity, e.g. a CLI) sees
/// everything, matching `BackgroundAgentTracker::addressable`.
#[must_use]
pub fn lookup(request_id: &str, scope: Option<&str>) -> Option<RecoveredRun> {
    let dir = store_dir()?;
    let record = index_lock().get(request_id).cloned()?;
    if let Some(scope) = scope {
        if !record.root_session.is_empty() && record.root_session != scope {
            return None;
        }
    }
    let trail = read_trail(&dir, &record);
    Some(RecoveredRun {
        last_activity_ms: trail.last_ms.unwrap_or(record.started_ms),
        partial_result: trail.text,
        record,
    })
}

// ============================================================================
// Internals
// ============================================================================

fn store_lock() -> crate::sync_primitives::MutexGuard<'static, Option<PathBuf>> {
    STORE_DIR.lock().unwrap_or_else(|e| e.into_inner())
}

fn index_lock() -> crate::sync_primitives::MutexGuard<'static, HashMap<String, PersistedRun>> {
    INDEX.lock().unwrap_or_else(|e| e.into_inner())
}

fn store_dir() -> Option<PathBuf> {
    store_lock().clone()
}

fn now_ms() -> u64 {
    super::subagent_tree_events::now_ms()
}

/// Filesystem-safe directory name for a request id. Request ids are uuid-ish,
/// but nothing in the type system says so, and a `../` in one would otherwise
/// escape the sidecar root.
fn slug(request_id: &str) -> String {
    let cleaned: String = request_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

/// Mask + bound one line of model/tool text before it can reach the disk.
fn mask_line(text: &str) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    let bounded: String = flat.chars().take(MAX_LINE_CHARS).collect();
    MASKER.mask(&bounded)
}

fn write_state(dir: &Path, record: &PersistedRun) {
    let run_dir = dir.join(slug(&record.request_id));
    if let Err(e) = std::fs::create_dir_all(&run_dir) {
        tracing::debug!(error = %e, "background_persistence: cannot create run dir");
        return;
    }
    let bytes = match serde_json::to_vec_pretty(record) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(error = %e, "background_persistence: cannot serialize record");
            return;
        }
    };
    if let Err(e) = crate::utils::atomic_io::write_atomic(&run_dir.join(STATE_FILE), &bytes) {
        tracing::debug!(error = %e, "background_persistence: state write failed");
    }
}

fn append_trail(dir: &Path, request_id: &str, text: &str) {
    let run_dir = dir.join(slug(request_id));
    if let Err(e) = std::fs::create_dir_all(&run_dir) {
        tracing::debug!(error = %e, "background_persistence: cannot create run dir");
        return;
    }
    let line = format!("{}\t{}\n", now_ms(), mask_line(text));
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join(RESULT_FILE))
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(line.as_bytes()) {
                tracing::debug!(error = %e, "background_persistence: trail append failed");
            }
        }
        Err(e) => tracing::debug!(error = %e, "background_persistence: trail open failed"),
    }
}

struct Trail {
    text: String,
    last_ms: Option<u64>,
}

/// Read back the tail of a run's activity trail. `last_activity` comes from the
/// last line's own timestamp rather than a field in `state.json`, so there is
/// exactly one place that knows when the child last did something.
///
/// The trail is located through [`PersistedRun::partial_result_path`], i.e. the
/// name the record itself carries — not by re-deriving the layout convention
/// here. A record written by an older (or future) build that named its trail
/// something else is still readable, and a record with no trail simply has
/// nothing to report.
fn read_trail(dir: &Path, record: &PersistedRun) -> Trail {
    let empty = || Trail {
        text: String::new(),
        last_ms: None,
    };
    let Some(path) = record.partial_result_path(dir) else {
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

    // Keep the TAIL, not the head: what the child was doing when it died is the
    // actionable part. UTF-8 safe by character boundary (P7).
    let rendered: String = lines
        .iter()
        .map(|l| l.split_once('\t').map_or(*l, |(_, body)| body))
        .collect::<Vec<_>>()
        .join("\n");
    let text = if rendered.len() > PARTIAL_RESULT_TAIL_BYTES {
        let start = rendered
            .char_indices()
            .rev()
            .map(|(i, _)| i)
            .find(|i| rendered.len() - i <= PARTIAL_RESULT_TAIL_BYTES)
            .unwrap_or(0);
        format!("…{}", &rendered[start..])
    } else {
        rendered
    };
    Trail { text, last_ms }
}

fn read_all(dir: &Path) -> Vec<PersistedRun> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let state = entry.path().join(STATE_FILE);
        let Ok(bytes) = std::fs::read(&state) else {
            continue;
        };
        match serde_json::from_slice::<PersistedRun>(&bytes) {
            Ok(record) => out.push(record),
            Err(e) => {
                // Fail-open on a corrupt record: a bad file must never block
                // boot. It is logged rather than deleted so it is still there
                // to diagnose.
                tracing::warn!(error = %e, path = %state.display(), "background_persistence: unreadable record");
            }
        }
    }
    out
}

/// Serializes every test that points the sidecar somewhere: the store root is
/// process-global by design (one daemon, one store), so two tests aiming it at
/// different tempdirs at once would read each other's records. Exposed
/// crate-wide because the tool-level integration test in
/// `agents::subagent_tool::tests` drives the same global.
#[cfg(test)]
static TEST_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Acquire [`TEST_GATE`]. Poison-tolerant (P7): a panicking test must not wedge
/// every later one.
#[cfg(test)]
pub(crate) fn test_gate() -> std::sync::MutexGuard<'static, ()> {
    TEST_GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Test-only: point the sidecar at `dir` without running the boot reconcile,
/// and drop whatever a previous test left in the index.
#[cfg(test)]
pub(crate) fn enable_for_test(dir: PathBuf) {
    std::fs::create_dir_all(&dir).expect("test store dir");
    *store_lock() = Some(dir);
    index_lock().clear();
}

/// Test-only: turn persistence back off so unrelated tests keep their zero-I/O
/// behaviour once this test's tempdir is gone.
#[cfg(test)]
pub(crate) fn disable_for_test() {
    *store_lock() = None;
    index_lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_gate as gate;

    /// The core of W24: a run left `Running` on disk by a dead process must
    /// come back as a tombstone that still carries what the child produced.
    ///
    /// Asserts on the re-read of `state.json`, not on the return value alone —
    /// discarding the `write_state` call inside the reconcile would still
    /// produce a populated `Vec`, and the next boot would re-report the same
    /// orphan forever.
    #[test]
    fn boot_reconcile_tombstones_orphans_and_keeps_their_partial_result() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        record_start("req-orphan", "agent:a:peer:user", "research X", "default");
        record_activity("req-orphan", "called web_search");
        record_activity("req-orphan", "found three candidate papers");
        disable_for_test();

        // A fresh daemon boots against the same directory.
        let recovered = init_and_reconcile(tmp.path().to_path_buf());
        // Keyed by id, not by count: the store root is process-global, so a
        // concurrently-running test that spawns a real background sub-agent
        // legitimately lands its own record in this same directory.
        let run = recovered
            .iter()
            .find(|r| r.record.request_id == "req-orphan")
            .expect("the orphan must be recovered");
        assert_eq!(run.record.phase, RunPhase::Abandoned);
        assert!(
            run.partial_result.contains("found three candidate papers"),
            "the child's work must survive the restart: {:?}",
            run.partial_result
        );
        assert!(
            run.last_activity_ms >= run.record.started_ms,
            "the trail timestamp must not predate the run"
        );

        // The tombstone is on disk, so a SECOND boot reports nothing new.
        let second = init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            !second.iter().any(|r| r.record.request_id == "req-orphan"),
            "a tombstoned orphan must not be re-reported on the next boot: {second:?}"
        );
        // ...and it is still answerable.
        let looked_up = lookup("req-orphan", Some("agent:a:peer:user")).expect("still on disk");
        assert_eq!(looked_up.record.phase, RunPhase::Abandoned);
        assert_eq!(
            looked_up.record.phase.status_label(),
            "interrupted_by_restart"
        );
        disable_for_test();
    }

    /// §5.1 — this file is a new egress for sub-agent output. Asserted on the
    /// RAW BYTES on disk: masking the value returned to the caller while
    /// writing the clear text would pass any assertion made on `partial_result`.
    #[test]
    fn the_activity_trail_is_redacted_before_it_reaches_the_disk() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345";
        record_start(
            "req-secret",
            "agent:a:peer:user",
            "read the vault",
            "default",
        );
        record_activity("req-secret", &format!("tool returned: {secret}"));

        let on_disk =
            std::fs::read_to_string(tmp.path().join("req-secret").join(RESULT_FILE)).unwrap();
        assert!(
            !on_disk.contains("abcdefghijklmnopqrstuvwxyz"),
            "a credential must never land in the sidecar: {on_disk}"
        );
        assert!(on_disk.contains("REDACTED"), "got: {on_disk}");
        disable_for_test();
    }

    /// The sidecar is process-global, so its read face has to be scoped exactly
    /// like the tracker's — otherwise a request_id learned from a log line
    /// reads another session's output back out of it.
    #[test]
    fn lookup_is_scoped_to_the_owning_session() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        record_start("req-scope", "agent:owner:peer:user", "t", "default");
        assert!(lookup("req-scope", Some("agent:owner:peer:user")).is_some());
        assert!(
            lookup("req-scope", Some("agent:stranger:peer:user")).is_none(),
            "another session must not read this run out of the sidecar"
        );
        assert!(
            lookup("req-scope", None).is_some(),
            "an unscoped caller sees it"
        );
        disable_for_test();
    }

    /// A run that settles normally leaves a terminal record, not a `Running`
    /// one — otherwise the next boot would tombstone something that finished.
    #[test]
    fn a_settled_run_is_not_tombstoned_by_the_next_boot() {
        let _g = gate();
        let tmp = tempfile::tempdir().unwrap();
        enable_for_test(tmp.path().to_path_buf());

        record_start("req-done", "agent:a:peer:user", "t", "default");
        record_settled("req-done", "completed", "the answer is 42");
        disable_for_test();

        let recovered = init_and_reconcile(tmp.path().to_path_buf());
        assert!(
            !recovered.iter().any(|r| r.record.request_id == "req-done"),
            "a settled run is not an orphan: {recovered:?}"
        );
        let looked_up = lookup("req-done", None).expect("terminal record is retained");
        assert_eq!(looked_up.record.phase, RunPhase::Settled);
        assert_eq!(looked_up.record.outcome.as_deref(), Some("completed"));
        assert!(looked_up.partial_result.contains("the answer is 42"));
        disable_for_test();
    }

    /// Persistence is opt-in. With no store dir configured nothing is written
    /// and `lookup` answers `None` — the pre-existing behaviour, byte for byte.
    #[test]
    fn every_entry_point_is_a_no_op_while_persistence_is_off() {
        let _g = gate();
        disable_for_test();
        record_start("req-off", "s", "t", "a");
        record_activity("req-off", "x");
        record_settled("req-off", "completed", "y");
        assert!(lookup("req-off", None).is_none());
    }

    /// A request id is attacker-adjacent input as far as the filesystem is
    /// concerned: it names a directory.
    #[test]
    fn slug_cannot_escape_the_store_root() {
        assert_eq!(slug("../../etc/passwd"), "______etc_passwd");
        assert_eq!(slug(""), "_");
        assert!(!slug("a/b").contains('/'));
    }
}
