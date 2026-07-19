//! Per-cron-job carry-over storage for `BudgetExhaustedPartialResult`.
//!
//! When a scheduled run trips a budget cap mid-task BUT has already
//! produced partial work, the harness emits
//! [`TerminateReason::BudgetExhaustedPartialResult`](crate::orchestrator::dispatch::TerminateReason)
//! carrying the partial text. The cron executor writes that text here,
//! keyed by `job_id`. The next time the same job fires, the executor
//! reads the stored partial back and prepends it to the user prompt
//! so the next run picks up where the previous one left off instead of
//! retrying from scratch.
//!
//! ## Storage shape
//!
//! One JSON file per job at
//! `$ALEPH_HOME/data/cron_carryover/{sanitised_job_id}.json`:
//!
//! ```json
//! { "partial_summary": "I gathered 3 sources but ran out of iterations…",
//!   "reason": "hit_max_iterations",
//!   "saved_at_ms": 1715212345678 }
//! ```
//!
//! No schema migration is needed — when the field set evolves, deserialise
//! with `#[serde(default)]` and fall back to a fresh run.
//!
//! ## Why a file, not the job snapshot?
//!
//! Cron job snapshots live in `SQLite` (`tasks::cron::store`). Adding a
//! `partial_summary` column would require a migration AND threading the
//! new field through the snapshot DTO + every test fixture. File-based
//! carry-over is additive — keeps store schema stable, survives daemon
//! restarts (unlike a process-memory cache), and is trivial to inspect
//! / clear from the shell when debugging.
//!
//! `$ALEPH_HOME` resolves identically to [`crate::canvas_io::canvas_dir`]
//! — env var → `~/.aleph` → `./` last resort.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// Carry-over files older than this are reclaimed by the sweeper.
///
/// Much longer than the 7-day tool-result retention because carry-overs
/// encode *user-meaningful in-flight work* — a sparsely-scheduled monthly
/// job that always hits its budget should still find its previous
/// partial waiting for it, not get garbage-collected silently. 30 days
/// gives a wide safety margin for typical cron cadences (hourly →
/// monthly) without letting truly stale state accumulate indefinitely.
pub const DEFAULT_CARRYOVER_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How often the periodic sweeper wakes up to scan for stale files.
///
/// Carry-overs are written at most a few per job per day; an hourly sweep
/// (the tool-result cadence) would be 6× more frequent than necessary.
/// 6 hours keeps the daemon overhead negligible while still bounding the
/// "stale file lingers past expiry" window to a fraction of the retention.
pub const DEFAULT_CARRYOVER_SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// On-disk shape of a single job's carry-over record.
///
/// `reason` carries the underlying cap label (`"hit_max_iterations"`,
/// `"context_budget_exhausted"`, `"max_output_tokens_exhausted"`) for
/// debugging. The prompt-injection path doesn't read it — it only
/// surfaces the prose in `partial_summary` to the next run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CarryOver {
    pub partial_summary: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub saved_at_ms: i64,
}

impl CarryOver {
    pub fn new(partial_summary: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            partial_summary: partial_summary.into(),
            reason: reason.into(),
            saved_at_ms: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// `$ALEPH_HOME/data/cron_carryover/`. Created on first write.
#[must_use]
pub fn carryover_dir() -> PathBuf {
    aleph_home().join("data").join("cron_carryover")
}

fn aleph_home() -> PathBuf {
    std::env::var_os("ALEPH_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".aleph")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Sanitise `job_id` to `[A-Za-z0-9._-]` for safe filesystem use. Same
/// rules as [`crate::canvas_io::sanitise_name`] but inlined to keep the
/// dependency on canvas io off this module.
fn sanitise_job_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_replacement = false;
    for ch in raw.chars() {
        let keep = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-');
        if keep {
            out.push(ch);
            last_was_replacement = false;
        } else if !last_was_replacement {
            out.push('_');
            last_was_replacement = true;
        }
    }
    let trimmed = out.trim_matches(|c: char| c == '.' || c == '-' || c == '_');
    if trimmed.is_empty() {
        "anonymous".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Carryover filename for a `job_id`. Pure-character sanitisation is not
/// injective (`foo/bar_baz` and `foo_bar_baz` both produce `foo_bar_baz`),
/// so two distinct job ids would otherwise share one carryover file and
/// silently overwrite / read each other's progress. Suffix a short hash of
/// the raw id so collisions are distinguishable while the human-readable
/// prefix stays grep-friendly.
fn carryover_filename(job_id: &str) -> String {
    let safe = sanitise_job_id(job_id);
    let hash = short_hash(job_id.as_bytes());
    format!("{safe}-{hash}.json")
}

/// 32-bit FNV-1a — fast, non-cryptographic, no external dep. Eight hex chars
/// is enough to make accidental collisions negligible for carryover files.
fn short_hash(bytes: &[u8]) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{:08x}", h)
}

fn carryover_path_at(dir: &std::path::Path, job_id: &str) -> PathBuf {
    dir.join(carryover_filename(job_id))
}

/// Production helper — uses the default `carryover_dir()`.
#[must_use]
pub fn carryover_path(job_id: &str) -> PathBuf {
    carryover_path_at(&carryover_dir(), job_id)
}

/// Read the stored carry-over for `job_id` from `dir`. Tests pass a
/// tempdir; production goes through [`read`].
///
/// Returns `Ok(None)` for the common case (no prior partial, file absent).
/// Surfaces other IO errors as `Err` so the caller can decide whether
/// to log + skip vs. fail the run.
pub fn read_at(dir: &std::path::Path, job_id: &str) -> std::io::Result<Option<CarryOver>> {
    let path = carryover_path_at(dir, job_id);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<CarryOver>(&text)
            .map(Some)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Production wrapper around [`read_at`].
pub fn read(job_id: &str) -> std::io::Result<Option<CarryOver>> {
    read_at(&carryover_dir(), job_id)
}

/// Write `record` for `job_id` under `dir`. Creates the directory on
/// demand. Atomic-rename via tempfile to avoid partial reads on crash —
/// matches the [`crate::canvas_io::save_canvas`] pattern.
pub fn write_at(dir: &std::path::Path, job_id: &str, record: &CarryOver) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let dest = carryover_path_at(dir, job_id);
    let tmp = dest.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&tmp, body)?;
    fs::rename(&tmp, &dest)
}

/// Production wrapper around [`write_at`].
pub fn write(job_id: &str, record: &CarryOver) -> std::io::Result<()> {
    write_at(&carryover_dir(), job_id, record)
}

/// Clear (delete) a job's carry-over. Called after the carry-over has
/// been successfully injected into the next run's prompt so we don't
/// re-inject the same stale partial on every subsequent firing.
///
/// `Ok(())` for both "deleted" and "didn't exist" — the caller's intent
/// is "make sure there's no carry-over for this job", which is satisfied
/// either way.
pub fn clear_at(dir: &std::path::Path, job_id: &str) -> std::io::Result<()> {
    let path = carryover_path_at(dir, job_id);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Production wrapper around [`clear_at`].
pub fn clear(job_id: &str) -> std::io::Result<()> {
    clear_at(&carryover_dir(), job_id)
}

/// Render the prompt-injection prefix for a given carry-over. Wrapped
/// in `<carryover>` tags so the LLM can spot it as harness-supplied
/// context rather than fresh user instructions.
///
/// Empty `partial_summary` collapses to an empty string so callers can
/// safely concatenate without a sentinel check.
#[must_use]
pub fn render_prefix(record: &CarryOver) -> String {
    let text = record.partial_summary.trim();
    if text.is_empty() {
        return String::new();
    }
    format!(
        "<carryover reason=\"{}\">\n\
         A previous scheduled run of this task ran out of budget before \
         finishing. Below is the partial progress it managed to capture. \
         Resume from where it left off — do NOT re-do completed work.\n\n\
         {}\n\
         </carryover>\n\n",
        record.reason, text,
    )
}

// =============================================================================
// Periodic sweeper — reclaim carry-over files older than retention.
//
// Modeled on [`crate::tools::result_store::sweep_stale_tool_result_dirs`]
// but operating on FILES (one per job) rather than directories (one per
// session). Decision is based on file mtime, which the atomic-rename in
// [`write_at`] resets on every fresh write — so a long-running job that
// keeps re-saving partials always stays fresh.
// =============================================================================

/// Sweep stale carry-over files under `dir`. Returns the number of files
/// removed. Errors on individual entries are logged at WARN and skipped
/// so a single permission denial cannot stall the sweep.
///
/// Empty / missing directory is a no-op (returns 0) — first-boot and
/// freshly-cleaned production both hit this path normally.
///
/// Decision is by **file mtime** (last write), not `saved_at_ms` inside
/// the JSON. The two should agree because [`write_at`] performs an
/// atomic rename, which gives the destination file a fresh mtime at
/// each write. Reading mtime is cheaper than deserialising every file
/// just to compare timestamps, and the sweep prefers false negatives
/// (skip removal) over false positives (kill live state).
pub fn sweep_stale_carryovers(dir: &Path, cutoff: Duration) -> usize {
    let entries = match fs::read_dir(dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "carryover sweep: read_dir failed",
            );
            return 0;
        }
    };
    let now = SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        // Layout: one .json file per job. Skip directories so a stray
        // user-placed folder never gets purged, and skip leftover .tmp
        // files (a write that crashed mid-rename) — leave those for an
        // operator to inspect.
        let is_file = entry.file_type().is_ok_and(|ft| ft.is_file());
        if !is_file {
            continue;
        }
        let is_json = path.extension().is_some_and(|ext| ext == "json");
        if !is_json {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|mtime| now.duration_since(mtime).unwrap_or(Duration::ZERO) >= cutoff);
        if !stale {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(
                file = %path.display(),
                error = %e,
                "carryover sweep: remove_file failed",
            ),
        }
    }
    removed
}

/// Spawn a background Tokio task that periodically calls
/// [`sweep_stale_carryovers`] on `dir`.
///
/// Fire-and-forget: the task is detached. No-op when no Tokio runtime
/// is current — keeps non-async test harnesses from panicking.
///
/// Callers that want one-shot deterministic GC (admin CLI, test
/// fixtures) should invoke [`sweep_stale_carryovers`] directly.
pub fn spawn_periodic_carryover_sweeper(dir: PathBuf, retention: Duration, interval: Duration) {
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::debug!(
            dir = %dir.display(),
            "carryover sweeper not spawned: no Tokio runtime",
        );
        return;
    }
    tokio::spawn(async move {
        // Match the tool-result sweeper: skip the first immediate tick
        // so service boot doesn't block on a synchronous fs walk.
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let dir = dir.clone();
            let removed =
                tokio::task::spawn_blocking(move || sweep_stale_carryovers(&dir, retention))
                    .await
                    .unwrap_or(0);
            if removed > 0 {
                tracing::info!(
                    removed,
                    "carryover sweeper reclaimed stale cron carry-over files",
                );
            }
        }
    });
}

/// Process-wide guard so repeated [`CronService::new`] calls (the
/// sweeper bootstrap point — see [`mod@crate::tasks::cron`]) cannot
/// spawn duplicate sweeper tasks. Mirrors the
/// [`crate::tools::result_store::set_global_tool_result_store`] pattern.
static CARRYOVER_SWEEPER_INSTALLED: OnceLock<()> = OnceLock::new();

/// Bootstrap a single global carry-over sweeper rooted at the
/// production [`carryover_dir`]. Safe to call from every
/// `CronService::new` — second and later invocations are no-ops thanks
/// to the [`OnceLock`] guard.
///
/// Returns `true` when the sweeper was actually installed by this call,
/// `false` when it had already been installed. Useful for test
/// assertions; production code can ignore the return.
pub fn bootstrap_global_carryover_sweeper() -> bool {
    let mut installed = false;
    CARRYOVER_SWEEPER_INSTALLED.get_or_init(|| {
        spawn_periodic_carryover_sweeper(
            carryover_dir(),
            DEFAULT_CARRYOVER_RETENTION,
            DEFAULT_CARRYOVER_SWEEP_INTERVAL,
        );
        installed = true;
    });
    installed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_write_then_read() {
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("partial work here", "hit_max_iterations");
        write_at(dir.path(), "job_42", &rec).unwrap();
        let got = read_at(dir.path(), "job_42").unwrap().unwrap();
        assert_eq!(got.partial_summary, "partial work here");
        assert_eq!(got.reason, "hit_max_iterations");
    }

    #[test]
    fn read_missing_returns_none_not_error() {
        let dir = TempDir::new().unwrap();
        let got = read_at(dir.path(), "absent_job").unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn clear_deletes_record_idempotently() {
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("x", "y");
        write_at(dir.path(), "j", &rec).unwrap();
        assert!(read_at(dir.path(), "j").unwrap().is_some());
        clear_at(dir.path(), "j").unwrap();
        assert!(read_at(dir.path(), "j").unwrap().is_none());
        // idempotent second clear
        clear_at(dir.path(), "j").unwrap();
    }

    #[test]
    fn sanitise_job_id_strips_path_traversal() {
        assert_eq!(sanitise_job_id("../etc/passwd"), "etc_passwd");
        assert_eq!(sanitise_job_id("normal-job_42"), "normal-job_42");
        assert_eq!(sanitise_job_id("foo/bar baz"), "foo_bar_baz");
        assert_eq!(sanitise_job_id(""), "anonymous");
        assert_eq!(sanitise_job_id("..."), "anonymous");
    }

    #[test]
    fn render_prefix_includes_reason_and_partial() {
        let rec = CarryOver::new("did A, B; C remaining", "context_budget_exhausted");
        let s = render_prefix(&rec);
        assert!(s.contains("<carryover reason=\"context_budget_exhausted\">"));
        assert!(s.contains("did A, B; C remaining"));
        assert!(s.contains("Resume from where it left off"));
        assert!(s.ends_with("\n\n"));
    }

    #[test]
    fn render_prefix_collapses_to_empty_for_blank_partial() {
        let rec = CarryOver::new("   \n  \t", "hit_max_iterations");
        assert!(render_prefix(&rec).is_empty());
    }

    #[test]
    fn legacy_record_without_reason_or_saved_at_deserialises() {
        // Forward-compat probe — if someone hand-edits a carry-over file
        // and drops the optional fields, we still round-trip.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("legacy.json");
        std::fs::write(&path, r#"{"partial_summary":"hello"}"#).unwrap();
        let got: CarryOver =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(got.partial_summary, "hello");
        assert_eq!(got.reason, "");
        assert_eq!(got.saved_at_ms, 0);
    }

    #[test]
    fn write_uses_atomic_rename() {
        // Verify the .json.tmp pattern: after write, the .tmp file is gone
        // (renamed to final). If a crash happened mid-write, a reader
        // would not see a half-written .json file.
        //
        // The final filename is `{sanitised_id}-{hash}.json` (the hash
        // suffix disambiguates ids that would otherwise collide on the
        // pure-character sanitisation — see `carryover_filename`).
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("x", "y");
        write_at(dir.path(), "atomic_job", &rec).unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.iter().any(|n| n.starts_with("atomic_job-") && n.ends_with(".json")),
            "expected atomic_job-<hash>.json in {entries:?}"
        );
        assert!(
            !entries.iter().any(|n| n.ends_with(".tmp")),
            "tmp file must be renamed away, entries={entries:?}",
        );
    }

    // ── Sweeper tests ─────────────────────────────────────────────────

    /// Move `path`'s mtime backwards by `delta` so the sweeper sees it
    /// as stale without actually waiting `delta` real seconds. Uses
    /// `filetime`, the same crate the production tool-result sweeper
    /// uses for its own retention tests.
    fn backdate(path: &Path, delta: Duration) {
        let target = SystemTime::now()
            .checked_sub(delta)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let target_ft = filetime::FileTime::from_system_time(target);
        // atime == mtime keeps behaviour predictable on filesystems
        // that don't auto-update atime.
        filetime::set_file_times(path, target_ft, target_ft).unwrap();
    }

    #[test]
    fn sweep_removes_files_older_than_cutoff() {
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("fresh", "y");
        write_at(dir.path(), "fresh_job", &rec).unwrap();
        write_at(dir.path(), "stale_job", &rec).unwrap();
        // Backdate one file to 60 days old; cutoff is 30 days.
        backdate(
            &carryover_path_at(dir.path(), "stale_job"),
            Duration::from_secs(60 * 24 * 60 * 60),
        );
        let removed = sweep_stale_carryovers(dir.path(), DEFAULT_CARRYOVER_RETENTION);
        assert_eq!(removed, 1);
        assert!(read_at(dir.path(), "fresh_job").unwrap().is_some());
        assert!(read_at(dir.path(), "stale_job").unwrap().is_none());
    }

    #[test]
    fn sweep_on_empty_dir_is_zero_no_error() {
        let dir = TempDir::new().unwrap();
        let removed = sweep_stale_carryovers(dir.path(), DEFAULT_CARRYOVER_RETENTION);
        assert_eq!(removed, 0);
    }

    #[test]
    fn sweep_on_missing_dir_is_zero_no_error() {
        let missing = std::path::PathBuf::from("/tmp/aleph-carryover-does-not-exist-zzz");
        let removed = sweep_stale_carryovers(&missing, DEFAULT_CARRYOVER_RETENTION);
        assert_eq!(removed, 0);
    }

    #[test]
    fn sweep_ignores_directories_and_tmp_files() {
        let dir = TempDir::new().unwrap();
        // Make a subdirectory (looks like a stale file by mtime).
        let subdir = dir.path().join("intruder");
        std::fs::create_dir(&subdir).unwrap();
        // Leftover .tmp from a crashed write — should be left alone.
        let tmp = dir.path().join("crashed.json.tmp");
        std::fs::write(&tmp, b"partial").unwrap();
        // A real stale .json.
        let rec = CarryOver::new("x", "y");
        write_at(dir.path(), "real_job", &rec).unwrap();
        backdate(
            &carryover_path_at(dir.path(), "real_job"),
            Duration::from_secs(40 * 24 * 60 * 60),
        );
        backdate(&subdir, Duration::from_secs(40 * 24 * 60 * 60));
        backdate(&tmp, Duration::from_secs(40 * 24 * 60 * 60));

        let removed = sweep_stale_carryovers(dir.path(), DEFAULT_CARRYOVER_RETENTION);
        assert_eq!(removed, 1, "only the .json file should be removed");
        assert!(subdir.is_dir(), "intruder dir must survive");
        assert!(tmp.exists(), "crash-leftover .tmp must survive");
    }

    #[test]
    fn sweep_with_zero_cutoff_removes_every_file() {
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("x", "y");
        write_at(dir.path(), "a", &rec).unwrap();
        write_at(dir.path(), "b", &rec).unwrap();
        write_at(dir.path(), "c", &rec).unwrap();
        let removed = sweep_stale_carryovers(dir.path(), Duration::ZERO);
        assert_eq!(removed, 3);
    }

    #[test]
    fn write_resets_mtime_so_resaved_file_survives_sweep() {
        // The whole point of mtime-based retention: a job that keeps
        // re-saving its partial every firing should stay fresh forever.
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("x", "y");
        write_at(dir.path(), "active_job", &rec).unwrap();
        // Pretend the original write was 60 days ago.
        backdate(
            &carryover_path_at(dir.path(), "active_job"),
            Duration::from_secs(60 * 24 * 60 * 60),
        );
        // A subsequent firing writes a fresh partial — mtime resets.
        write_at(dir.path(), "active_job", &rec).unwrap();
        let removed = sweep_stale_carryovers(dir.path(), DEFAULT_CARRYOVER_RETENTION);
        assert_eq!(removed, 0);
        assert!(read_at(dir.path(), "active_job").unwrap().is_some());
    }

    #[tokio::test]
    async fn bootstrap_global_sweeper_is_idempotent() {
        // First call inside a tokio runtime should install (return
        // true). The OnceLock claim is process-wide and persists across
        // tests; the contract we care about is: never spawn two
        // sweepers. We only assert the second call is a no-op, since
        // an earlier test may have already claimed the lock.
        let first = bootstrap_global_carryover_sweeper();
        let second = bootstrap_global_carryover_sweeper();
        assert!(!second, "second bootstrap must be a no-op");
        // If `first` was true, the spawned task is fire-and-forget; the
        // tokio runtime will tear it down when the test exits. If
        // `first` was false, an earlier test installed it.
        let _ = first;
    }
}
