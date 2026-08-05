//! `core/sqlite-integrity` — the operational `SQLite` stores must be readable.
//!
//! Aleph keeps most of its durable state in `~/.aleph/data/*.db`: sessions,
//! security, pairing, devices, the delivery queue, the loop graph, the hub
//! catalog. A corrupted page in any of them surfaces at runtime as an opaque
//! write failure or a query that silently returns nothing — the kind of
//! symptom that gets blamed on the feature above it rather than on the file
//! underneath. codex checks the same thing (`state_check` →
//! `sqlite_integrity_detail`); Aleph runs it over *every* store in the data
//! dir instead of one named file, and concurrently.
//!
//! **Not repairable.** A failed integrity check means "restore from a backup
//! or accept data loss" — a human decision with irreversible consequences,
//! and `.recover` is not something a diagnostic should run behind the
//! operator's back. The finding routes to the decision instead.
//!
//! `PRAGMA quick_check` (not `integrity_check`) is deliberate: it skips the
//! index-vs-table cross-validation, which is O(size) on multi-hundred-MB
//! session stores, and still catches the page-level corruption this check
//! exists to find. The deadline in the engine is the backstop, not the plan.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};

const ID: &str = "core/sqlite-integrity";

/// Tag on the per-database success finding.
const TAG_DB_OK: &str = "sqlite-ok";
/// Tag on a per-database failure finding (unreadable or corrupt).
const TAG_DB_CORRUPT: &str = "sqlite-corrupt";

/// Rows of `quick_check` output kept in the detail. The pragma emits one row
/// per problem and can produce thousands on a badly damaged file; the first
/// few identify the failure class, the rest are noise in a tool result.
const MAX_REPORTED_ROWS: usize = 5;

/// Cap on how many stores are probed in one run. The data dir is Aleph's own
/// and holds well under a dozen databases; a directory that somehow holds
/// hundreds is itself the finding, and probing all of them would trade a
/// bounded check for an unbounded one.
const MAX_DATABASES: usize = 32;

pub struct SqliteIntegrityCheck {
    data_dir: PathBuf,
}

impl SqliteIntegrityCheck {
    #[must_use]
    pub const fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }
}

/// Outcome of probing one database file.
enum DbVerdict {
    Ok,
    /// The file could not be opened at all (permissions, not-a-database).
    Unreadable(String),
    /// `quick_check` returned something other than `ok`.
    Corrupt(Vec<String>),
}

/// Run `PRAGMA quick_check` against one file. Blocking (rusqlite is sync) —
/// callers must keep it off the async executor.
fn probe_database(path: &Path) -> DbVerdict {
    let conn = match crate::utils::sqlite_open::open_sqlite_readonly(path) {
        Ok(c) => c,
        Err(e) => return DbVerdict::Unreadable(e.to_string()),
    };
    let rows = conn.prepare("PRAGMA quick_check").and_then(|mut stmt| {
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()
    });
    match rows {
        // The pragma reports health as a single literal `ok` row.
        Ok(rows) if rows.len() == 1 && rows[0].eq_ignore_ascii_case("ok") => DbVerdict::Ok,
        Ok(rows) => DbVerdict::Corrupt(rows.into_iter().take(MAX_REPORTED_ROWS).collect()),
        Err(e) => DbVerdict::Unreadable(e.to_string()),
    }
}

/// Collect `*.db` files directly under `dir`, sorted for a deterministic
/// report. Journal / WAL sidecars are excluded: they are not standalone
/// databases and opening them reports a spurious corruption.
fn list_databases(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dbs: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "db"))
        .collect();
    dbs.sort();
    dbs.truncate(MAX_DATABASES);
    dbs
}

fn corrupt_hint(name: &str) -> String {
    format!(
        "Stop aleph-server, back up {name} before touching it, then restore the newest good \
         copy (or move the file aside to let Aleph recreate an empty store — that discards its \
         contents). Doctor never repairs this: recovering a damaged store is a data-loss \
         decision, not a mechanical fix."
    )
}

#[async_trait]
impl HealthCheck for SqliteIntegrityCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "SQLite integrity"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let dir = self.data_dir.clone();

        // rusqlite is synchronous and `quick_check` reads the whole file —
        // keep both off the async executor.
        let probed = tokio::task::spawn_blocking(move || {
            list_databases(&dir)
                .into_iter()
                .map(|path| {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into(),
                    );
                    let verdict = probe_database(&path);
                    (name, verdict)
                })
                .collect::<Vec<_>>()
        })
        .await;

        let probed = match probed {
            Ok(p) => p,
            Err(e) => {
                return vec![Finding::problem(
                    ID,
                    Severity::Warning,
                    "SQLite integrity unknown",
                    format!("the integrity probe task failed to run: {e}"),
                )]
            }
        };

        if probed.is_empty() {
            return vec![Finding::ok(
                ID,
                "No SQLite stores yet",
                format!(
                    "{} holds no *.db files — nothing to verify (normal before first run).",
                    self.data_dir.display()
                ),
            )];
        }

        probed
            .into_iter()
            .map(|(name, verdict)| match verdict {
                DbVerdict::Ok => Finding::ok(ID, format!("{name}: ok"), "quick_check passed.")
                    .with_tag(TAG_DB_OK),
                DbVerdict::Unreadable(err) => Finding::problem(
                    ID,
                    Severity::Error,
                    format!("{name}: unreadable"),
                    format!("could not open or query the store: {err}"),
                )
                .with_fix_hint(corrupt_hint(&name))
                .with_tag(TAG_DB_CORRUPT),
                DbVerdict::Corrupt(rows) => Finding::problem(
                    ID,
                    Severity::Error,
                    format!("{name}: corrupt"),
                    format!("quick_check reported: {}", rows.join("; ")),
                )
                .with_fix_hint(corrupt_hint(&name))
                .with_tag(TAG_DB_CORRUPT),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn seed_db(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let conn = crate::utils::sqlite_open::open_sqlite_safe(&path).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (1);")
            .unwrap();
        drop(conn);
        path
    }

    #[tokio::test]
    async fn ok_when_data_dir_has_no_databases() {
        let tmp = tempdir().unwrap();
        let check = SqliteIntegrityCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    async fn ok_when_data_dir_is_missing_entirely() {
        // First run: the data dir may not exist yet — `core/data-dir` owns
        // that finding, this check must not duplicate it as a scary error.
        let tmp = tempdir().unwrap();
        let check = SqliteIntegrityCheck::new(tmp.path().join("absent"));
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    async fn healthy_store_passes_and_is_tagged() {
        let tmp = tempdir().unwrap();
        seed_db(tmp.path(), "sessions.db");
        let check = SqliteIntegrityCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].is_problem());
        assert!(findings[0].has_tag(TAG_DB_OK));
        assert!(findings[0].title.starts_with("sessions.db"));
    }

    #[tokio::test]
    async fn garbage_file_with_db_extension_is_an_error() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("broken.db"), b"this is not a database").unwrap();
        let check = SqliteIntegrityCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].has_tag(TAG_DB_CORRUPT));
        assert!(findings[0].fix_hint.as_deref().unwrap().contains("back up"));
    }

    #[tokio::test]
    async fn never_repairs_even_in_fix_posture() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("broken.db"), b"not a database").unwrap();
        let check = SqliteIntegrityCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Fix).await;
        assert!(findings.iter().all(|f| !f.repairable));
        assert!(findings.iter().all(|f| f.repair_outcome.is_none()));
        // The file is still there — a "repair" that deleted it would be data loss.
        assert!(tmp.path().join("broken.db").exists());
    }

    #[tokio::test]
    async fn wal_and_journal_sidecars_are_not_probed() {
        let tmp = tempdir().unwrap();
        seed_db(tmp.path(), "sessions.db");
        // A WAL sidecar is not a standalone database; probing it would report
        // a phantom corruption next to a perfectly healthy store.
        std::fs::write(tmp.path().join("sessions.db-wal"), b"garbage").unwrap();
        std::fs::write(tmp.path().join("sessions.db-shm"), b"garbage").unwrap();
        let check = SqliteIntegrityCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1, "only the .db file is a database");
        assert!(!findings[0].is_problem());
    }

    #[tokio::test]
    async fn every_store_gets_its_own_finding() {
        let tmp = tempdir().unwrap();
        seed_db(tmp.path(), "a.db");
        seed_db(tmp.path(), "b.db");
        std::fs::write(tmp.path().join("c.db"), b"broken").unwrap();
        let check = SqliteIntegrityCheck::new(tmp.path().to_path_buf());
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 3);
        assert_eq!(findings.iter().filter(|f| f.is_problem()).count(), 1);
    }
}
