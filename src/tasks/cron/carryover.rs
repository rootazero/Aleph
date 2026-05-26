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
//! Cron job snapshots live in SQLite (`tasks::cron::store`). Adding a
//! `partial_summary` column would require a migration AND threading the
//! new field through the snapshot DTO + every test fixture. File-based
//! carry-over is additive — keeps store schema stable, survives daemon
//! restarts (unlike a process-memory cache), and is trivial to inspect
//! / clear from the shell when debugging.
//!
//! `$ALEPH_HOME` resolves identically to [`crate::canvas_io::canvas_dir`]
//! — env var → `~/.aleph` → `./` last resort.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

fn carryover_path_at(dir: &std::path::Path, job_id: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitise_job_id(job_id)))
}

/// Production helper — uses the default `carryover_dir()`.
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
pub fn write_at(
    dir: &std::path::Path,
    job_id: &str,
    record: &CarryOver,
) -> std::io::Result<()> {
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
        let got: CarryOver = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(got.partial_summary, "hello");
        assert_eq!(got.reason, "");
        assert_eq!(got.saved_at_ms, 0);
    }

    #[test]
    fn write_uses_atomic_rename() {
        // Verify the .json.tmp pattern: after write, the .tmp file is gone
        // (renamed to final). If a crash happened mid-write, a reader
        // would not see a half-written .json file.
        let dir = TempDir::new().unwrap();
        let rec = CarryOver::new("x", "y");
        write_at(dir.path(), "atomic_job", &rec).unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(entries.iter().any(|n| n == "atomic_job.json"));
        assert!(
            !entries.iter().any(|n| n.ends_with(".tmp")),
            "tmp file must be renamed away, entries={entries:?}",
        );
    }
}
