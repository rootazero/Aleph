//! `core/sqlite-integrity` — the operational `SQLite` stores must be readable.
//!
//! Aleph keeps most of its durable state in `~/.aleph/data/*.db`: sessions,
//! security, pairing, devices, the delivery queue, the loop graph, the hub
//! catalog. A corrupted page in any of them surfaces at runtime as an opaque
//! write failure or a query that silently returns nothing — the kind of
//! symptom that gets blamed on the feature above it rather than on the file
//! underneath. codex checks the same thing (`state_check` →
//! `sqlite_integrity_detail`); Aleph runs it over *every* store in the data
//! dir instead of one named file — sequentially inside a single
//! `spawn_blocking`, since rusqlite is synchronous and "concurrent" probing
//! would just move the serialization onto the async executor.
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

use crate::diagnostics::check::{unknown_finding, DirListing, HealthCheck, Posture};
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
/// report, plus how many entries the walk could not read. Journal / WAL
/// sidecars are excluded: they are not standalone databases and opening them
/// reports a spurious corruption.
///
/// # Three ways this used to turn "I could not look" into "there is nothing"
///
/// All three produced the same false sentence — the caller's *ok* finding,
/// "holds no *.db files — nothing to verify (normal before first run)" — on a
/// data dir that may be full of stores nobody could see:
///
/// 1. `read_dir`'s `Err` became an empty `Vec`. Now [`DirListing`] separates
///    "the directory is not there" (which really is nothing to verify, and
///    `core/data-dir` owns that finding) from "the directory would not open",
///    which is returned as the `Err` finding.
/// 2. `.filter_map(Result::ok)` dropped entries the OS refused **part-way
///    through** the walk. Now they are counted and returned, so the caller can
///    refuse to call an incomplete listing empty.
/// 3. `Path::is_file()` is false on any stat error too, so a `.db` whose
///    metadata could not be read vanished from the report. Now a path that
///    cannot be stat'd is KEPT: `probe_database` opens it and says something
///    true about it (`DbVerdict::Unreadable`, naming the file), which beats
///    silence. Only a successful stat saying "not a regular file" excludes it.
///
/// # Errors
///
/// The directory exists but could not be opened.
// The `Err` IS the finding this check will report; see `check::Presence::of`.
#[allow(clippy::result_large_err)]
fn list_databases(dir: &Path) -> Result<(Vec<PathBuf>, usize), Finding> {
    let (entries, unreadable_entries) = match DirListing::of(ID, "SQLite integrity", dir)? {
        DirListing::Absent => return Ok((Vec::new(), 0)),
        DirListing::Listed {
            entries,
            unreadable_entries,
        } => (entries, unreadable_entries),
    };
    let mut dbs: Vec<PathBuf> = entries
        .into_iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "db"))
        .filter(|p| p.metadata().map_or(true, |m| m.is_file()))
        .collect();
    dbs.sort();
    dbs.truncate(MAX_DATABASES);
    Ok((dbs, unreadable_entries))
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

    // The blocking closure below propagates a `Finding` as its `Err`; see
    // `list_databases`.
    #[allow(clippy::result_large_err)]
    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let dir = self.data_dir.clone();

        // rusqlite is synchronous and `quick_check` reads the whole file —
        // keep both off the async executor.
        let probed = tokio::task::spawn_blocking(move || {
            let (dbs, unreadable_entries) = list_databases(&dir)?;
            let verdicts = dbs
                .into_iter()
                .map(|path| {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into(),
                    );
                    let verdict = probe_database(&path);
                    (name, verdict)
                })
                .collect::<Vec<_>>();
            Ok::<_, Finding>((verdicts, unreadable_entries))
        })
        .await;

        let (probed, unreadable_entries) = match probed {
            Ok(Ok(p)) => p,
            // The directory is there and would not open. Not "no stores".
            Ok(Err(f)) => return vec![f],
            Err(e) => {
                return vec![unknown_finding(
                    ID,
                    "SQLite integrity",
                    format!("the integrity probe task failed to run: {e}"),
                )]
            }
        };

        if probed.is_empty() {
            // An incomplete listing that yielded no `*.db` cannot claim there
            // are none: "I found nothing" is not "there is nothing".
            if unreadable_entries > 0 {
                return vec![unknown_finding(
                    ID,
                    "SQLite integrity",
                    format!(
                        "{dir} opened, but {unreadable_entries} {entries} could not be \
                         read and no *.db file was among the ones that could — so this \
                         run cannot say whether any store is there, let alone whether \
                         it is intact.",
                        dir = self.data_dir.display(),
                        entries = plural_entries(unreadable_entries),
                    ),
                )];
            }
            return vec![Finding::ok(
                ID,
                "No SQLite stores yet",
                format!(
                    "{} holds no *.db files — nothing to verify (normal before first run).",
                    self.data_dir.display()
                ),
            )];
        }

        let mut findings: Vec<Finding> = probed
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
            .collect();

        // Verdicts above are real; the LIST they came from was not complete.
        // Said separately rather than folded into the per-store lines, because
        // what is unknown is which stores exist, not whether these ones pass.
        if unreadable_entries > 0 {
            findings.push(unknown_finding(
                ID,
                "SQLite integrity",
                format!(
                    "{unreadable_entries} {entries} in {dir} could not be read, so the \
                     list of stores above is incomplete — a database that is present \
                     but unlistable was never probed.",
                    dir = self.data_dir.display(),
                    entries = plural_entries(unreadable_entries),
                ),
            ));
        }
        findings
    }
}

/// `entry` / `entries`, so the two "incomplete listing" sentences above do not
/// each grow their own copy of the same conditional.
fn plural_entries(n: usize) -> &'static str {
    if n == 1 {
        "entry"
    } else {
        "entries"
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

    /// A data dir that cannot be opened is not a data dir with no stores in
    /// it. `read_dir`\'s dropped `Err` used to make the two identical, and the
    /// resulting sentence — "nothing to verify (normal before first run)" —
    /// is the reassuring one.
    ///
    /// Its twin, `ok_when_data_dir_is_missing_entirely`, pins the other
    /// direction: a genuinely absent directory must still read as ok, because
    /// `core/data-dir` owns that finding.
    #[tokio::test]
    async fn an_unopenable_data_dir_is_not_reported_as_holding_no_stores() {
        let check = SqliteIntegrityCheck::new(PathBuf::from("aleph\u{0}data"));
        let findings = check.run(Posture::Inspect).await;
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_problem(), "{:?}", findings[0]);
        assert_eq!(findings[0].title, "SQLite integrity unknown");
    }
}
