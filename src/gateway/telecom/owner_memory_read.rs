//! Production [`PrivateReadDriver`]: read-only keyword search of the
//! owner's notes. The single private source opened to public telecom turns
//! (`[telecom_private_read]`), reached only after approval A was consumed.
//!
//! Read-only by construction and by enforcement:
//! * one `SELECT` over the `notes_fts` table (body text indexed in the
//!   memory database) — no embedding call, no `NoteFactRetrieval` (which
//!   records recall signals), no `MemorySearchTool` (which logs the query
//!   and notifies tool-result sinks), no file reads;
//! * the statement runs with `PRAGMA query_only = ON` on the backend's
//!   single connection (held under its mutex for the whole closure), so any
//!   write on that path fails instead of landing; the prior setting is
//!   restored even if the query panics, and restoration failure aborts rather
//!   than letting the shared connection proceed in an unknown state;
//! * only the proven owner-specific `main__u-owner` partition is searched.
//!   Legacy `main` is cross-project/global and excluded pending provenance
//!   proof, so older notes there may be unavailable. Profile, SOUL, workspace
//!   files, hooks, skills and secrets are separate stores this driver never
//!   touches.
//!
//! The query and the result are never logged.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::private_read::{PrivateReadDriver, PrivateReadError, PrivateReadResult};
use super::public_approval::PublicApprovalScope;
use crate::error::AlephError;
use crate::memory::MemoryBackend;

/// Most notes returned per read.
pub const MAX_OWNER_MEMORY_RESULTS: usize = 8;

/// Byte bound of the whole result text.
pub const MAX_OWNER_MEMORY_RESULT_BYTES: usize = 8 * 1024;

/// Byte bound of one note body inside the result.
const MAX_NOTE_BYTES: usize = 2 * 1024;

/// Most query terms turned into FTS phrases.
const MAX_QUERY_TERMS: usize = 16;

/// Read-only owner-memory search driver.
pub struct OwnerMemoryReadDriver {
    db: MemoryBackend,
    partitions: Vec<String>,
}

impl OwnerMemoryReadDriver {
    /// Search only the owner's personal scope. Never fall back to the
    /// cross-project/global base partition without proving its provenance.
    pub fn new(db: MemoryBackend) -> Self {
        let base = crate::gateway::agent_env::DEFAULT_AGENT;
        let owner = crate::memory::project_scope::scoped_agent_id(
            base,
            crate::gateway::security::store::OWNER_USER_ID,
        );
        assert_eq!(
            owner, "main__u-owner",
            "owner memory scope must be the proven owner-specific partition"
        );
        Self {
            db,
            partitions: vec![owner],
        }
    }

    async fn search(&self, query: &str) -> Result<Vec<(String, String)>, AlephError> {
        let Some(expr) = fts_expression(query)? else {
            return Ok(Vec::new());
        };
        let partitions = self.partitions.clone();
        self.db
            .with_conn(move |conn| {
                let _read_only =
                    QueryOnlyGuard::new(conn).map_err(|e| AlephError::other(e.to_string()))?;
                select_notes(conn, &expr, &partitions).map_err(|e| AlephError::other(e.to_string()))
            })
            .await
    }
}

/// Scoped query-only setting on the backend's shared, mutex-held connection.
/// The backend recovers poisoned mutexes, so a panic cannot be the fallback
/// for failure to restore its previous state.
struct QueryOnlyGuard<'a> {
    conn: &'a rusqlite::Connection,
    restore_writable: bool,
}

impl<'a> QueryOnlyGuard<'a> {
    fn new(conn: &'a rusqlite::Connection) -> Result<Self, rusqlite::Error> {
        let was_read_only: bool = conn.query_row("PRAGMA query_only", [], |row| row.get(0))?;
        if !was_read_only {
            conn.execute_batch("PRAGMA query_only = ON")?;
        }
        Ok(Self {
            conn,
            restore_writable: !was_read_only,
        })
    }
}

impl Drop for QueryOnlyGuard<'_> {
    fn drop(&mut self) {
        if self.restore_writable && self.conn.execute_batch("PRAGMA query_only = OFF").is_err() {
            // A recovered mutex could otherwise expose an uncertain state to
            // writers. There is no connection-poisoning API at this boundary.
            std::process::abort();
        }
    }
}

impl std::fmt::Debug for OwnerMemoryReadDriver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerMemoryReadDriver")
            .field("partitions", &self.partitions)
            .finish_non_exhaustive()
    }
}

fn select_notes(
    conn: &rusqlite::Connection,
    expr: &str,
    partitions: &[String],
) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT substr(path, 1, ?3), substr(content, 1, ?4) FROM notes_fts \
         WHERE content MATCH ?1 AND agent_id = ?2 ORDER BY rank LIMIT ?5",
    )?;
    let mut out = Vec::new();
    for agent in partitions {
        let rows = stmt.query_map(
            rusqlite::params![
                expr,
                agent,
                256_i64,
                MAX_NOTE_BYTES as i64,
                MAX_OWNER_MEMORY_RESULTS as i64
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        for row in rows {
            out.push(row?);
            if out.len() >= MAX_OWNER_MEMORY_RESULTS {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

/// AND of quoted phrases, one per whitespace-separated term. Restrict the
/// MATCH to content: a filename alone must not select an unrelated body.
/// Quoting makes FTS5 operators inert. Reject excess terms instead of
/// silently omitting them.
fn fts_expression(query: &str) -> Result<Option<String>, AlephError> {
    let terms: Vec<&str> = query.split_whitespace().take(MAX_QUERY_TERMS + 1).collect();
    if terms.len() > MAX_QUERY_TERMS {
        return Err(AlephError::other("owner memory query exceeds 16 terms"));
    }
    Ok((!terms.is_empty()).then(|| {
        terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ")
    }))
}

/// Prefix on a UTF-8 boundary, with a byte cap.
fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Bounded, UTF-8 safe result text.
fn render(rows: &[(String, String)]) -> String {
    let mut out = String::new();
    for (i, (path, content)) in rows.iter().take(MAX_OWNER_MEMORY_RESULTS).enumerate() {
        let entry = format!(
            "[{}] {}\n{}\n",
            i + 1,
            truncate_utf8(path, 256),
            truncate_utf8(content.trim(), MAX_NOTE_BYTES)
        );
        let room = MAX_OWNER_MEMORY_RESULT_BYTES - out.len();
        if entry.len() > room {
            out.push_str(truncate_utf8(&entry, room));
            break;
        }
        out.push_str(&entry);
    }
    out
}

#[async_trait]
impl PrivateReadDriver for OwnerMemoryReadDriver {
    async fn read(
        &self,
        _scope: &PublicApprovalScope,
        query: &str,
        cancel: &CancellationToken,
    ) -> Result<PrivateReadResult, PrivateReadError> {
        if cancel.is_cancelled() {
            return Err(PrivateReadError::Cancelled);
        }
        let rows = tokio::select! {
            () = cancel.cancelled() => return Err(PrivateReadError::Cancelled),
            rows = self.search(query) => rows,
        };
        match rows {
            Ok(rows) if rows.is_empty() => Ok(PrivateReadResult::Unavailable),
            Ok(rows) => Ok(PrivateReadResult::Transient(Arc::from(render(&rows)))),
            Err(_) => {
                tracing::warn!("[public] owner memory read failed");
                Err(PrivateReadError::Failed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SqliteMemoryBackend;

    async fn backend() -> MemoryBackend {
        MemoryBackend::new(SqliteMemoryBackend::in_memory().expect("in-memory backend"))
    }

    async fn insert(db: &MemoryBackend, agent: &str, path: &str, content: &str) {
        let (agent, path, content) = (agent.to_string(), path.to_string(), content.to_string());
        db.with_conn(move |c| {
            c.execute(
                "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES (?1, ?1, ?2, ?3)",
                rusqlite::params![path, content, agent],
            )
            .map_err(|e| AlephError::other(e.to_string()))?;
            Ok(())
        })
        .await
        .unwrap();
    }

    async fn total_changes(db: &MemoryBackend) -> i64 {
        db.with_conn(|c| {
            c.query_row("SELECT total_changes()", [], |r| r.get::<_, i64>(0))
                .map_err(|e| AlephError::other(e.to_string()))
        })
        .await
        .unwrap()
    }

    fn scope() -> PublicApprovalScope {
        let ctx =
            crate::gateway::telecom::public_context::test_support::sample_public_context("bind-m");
        PublicApprovalScope::new(&ctx, "c", "a", "t", 1)
    }

    async fn read(
        d: &OwnerMemoryReadDriver,
        q: &str,
    ) -> Result<PrivateReadResult, PrivateReadError> {
        d.read(&scope(), q, &CancellationToken::new()).await
    }

    #[tokio::test]
    async fn finds_owner_notes_only_and_writes_nothing() {
        let db = backend().await;
        insert(&db, "main", "a.md", "dentist appointment Tuesday 10:00").await;
        insert(&db, "main__u-owner", "b.md", "dentist is Dr. Lee").await;
        insert(&db, "main__u-guest", "c.md", "dentist GUEST-CANARY").await;
        insert(&db, "other", "d.md", "dentist OTHER-CANARY").await;
        let driver = OwnerMemoryReadDriver::new(db.clone());
        let before = total_changes(&db).await;
        let PrivateReadResult::Transient(body) = read(&driver, "dentist").await.unwrap() else {
            panic!("expected a result");
        };
        assert_eq!(total_changes(&db).await, before, "read must not write");
        assert!(
            !body.contains("Tuesday 10:00"),
            "base is not owner-exclusive"
        );
        assert!(body.contains("Dr. Lee"));
        assert!(!body.contains("GUEST-CANARY"));
        assert!(!body.contains("OTHER-CANARY"));
        // A previously writable connection remains writable for its writers.
        insert(&db, "main__u-owner", "e.md", "after").await;
        assert!(total_changes(&db).await > before, "writers remain enabled");
    }

    #[tokio::test]
    async fn base_only_note_is_unavailable() {
        let db = backend().await;
        insert(&db, "main", "legacy.md", "base-only-canary").await;
        let driver = OwnerMemoryReadDriver::new(db);
        assert_eq!(
            read(&driver, "base-only-canary").await,
            Ok(PrivateReadResult::Unavailable)
        );
    }

    #[tokio::test]
    async fn path_only_match_never_selects_private_content() {
        let db = backend().await;
        insert(
            &db,
            "main__u-owner",
            "dentist.md",
            "unrelated PRIVATE-CANARY",
        )
        .await;
        let driver = OwnerMemoryReadDriver::new(db);
        assert_eq!(
            read(&driver, "dentist").await,
            Ok(PrivateReadResult::Unavailable)
        );
    }

    #[tokio::test]
    async fn preserves_preexisting_query_only_setting() {
        let db = backend().await;
        insert(&db, "main__u-owner", "owner.md", "dentist owner").await;
        db.with_conn(|conn| {
            conn.execute_batch("PRAGMA query_only = ON")
                .map_err(|e| AlephError::other(e.to_string()))
        })
        .await
        .unwrap();
        let driver = OwnerMemoryReadDriver::new(db.clone());
        let before = total_changes(&db).await;
        assert!(matches!(
            read(&driver, "dentist").await,
            Ok(PrivateReadResult::Transient(_))
        ));
        db.with_conn(|conn| {
            let setting: bool = conn
                .query_row("PRAGMA query_only", [], |r| r.get(0))
                .map_err(|e| AlephError::other(e.to_string()))?;
            assert!(setting, "the driver must preserve a preexisting ON");
            assert!(conn
                .execute(
                    "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES ('x', 'x', 'x', 'main__u-owner')",
                    [],
                )
                .is_err());
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(total_changes(&db).await, before);
    }

    #[tokio::test]
    async fn query_only_blocks_writes_during_read_and_restores_after_panic() {
        let db = backend().await;
        let before = total_changes(&db).await;
        db.with_conn(|conn| {
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _read_only = QueryOnlyGuard::new(conn).unwrap();
                assert!(conn
                    .execute(
                        "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES ('x', 'x', 'x', 'main__u-owner')",
                        [],
                    )
                    .is_err());
                panic!("simulate an interrupted read");
            }));
            assert!(caught.is_err());
            let setting: bool = conn
                .query_row("PRAGMA query_only", [], |r| r.get(0))
                .map_err(|e| AlephError::other(e.to_string()))?;
            assert!(!setting, "panic must restore the original OFF setting");
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(total_changes(&db).await, before);
    }

    #[tokio::test]
    async fn overlimit_terms_fail_instead_of_ignoring_tail() {
        let db = backend().await;
        insert(&db, "main__u-owner", "owner.md", "first").await;
        let driver = OwnerMemoryReadDriver::new(db.clone());
        let before = total_changes(&db).await;
        let limit = (0..MAX_QUERY_TERMS)
            .map(|i| if i == 0 { "first" } else { "other" })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(fts_expression(&limit).unwrap().is_some());
        assert_eq!(
            read(&driver, &limit).await,
            Ok(PrivateReadResult::Unavailable)
        );
        let excess = format!("{limit} forbidden");
        assert!(fts_expression(&excess).is_err());
        assert_eq!(read(&driver, &excess).await, Err(PrivateReadError::Failed));
        assert_eq!(total_changes(&db).await, before);
    }

    #[tokio::test]
    async fn sql_limits_decoded_rows_before_rendering() {
        let db = backend().await;
        insert(
            &db,
            "main__u-owner",
            &"p".repeat(10_000),
            &format!("dentist {}", "x".repeat(100_000)),
        )
        .await;
        let expr = fts_expression("dentist").unwrap().unwrap();
        let rows = db
            .with_conn(move |conn| {
                select_notes(conn, &expr, &["main__u-owner".to_string()])
                    .map_err(|e| AlephError::other(e.to_string()))
            })
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.chars().count() <= 256);
        assert!(rows[0].1.chars().count() <= MAX_NOTE_BYTES);
    }

    #[tokio::test]
    async fn result_is_bounded_in_count_and_bytes_utf8_safe() {
        let db = backend().await;
        for i in 0..20 {
            insert(
                &db,
                "main__u-owner",
                &format!("n{i}.md"),
                &format!("needle {}", "é".repeat(3000)),
            )
            .await;
        }
        let driver = OwnerMemoryReadDriver::new(db);
        let PrivateReadResult::Transient(body) = read(&driver, "needle").await.unwrap() else {
            panic!("expected a result");
        };
        assert!(body.len() <= MAX_OWNER_MEMORY_RESULT_BYTES);
        assert!(body.matches("needle").count() <= MAX_OWNER_MEMORY_RESULTS);
        // `Arc<str>` construction already proves valid UTF-8.
    }

    #[tokio::test]
    async fn no_match_empty_query_and_fts_operators_are_inert() {
        let db = backend().await;
        insert(&db, "main__u-owner", "a.md", "something").await;
        let driver = OwnerMemoryReadDriver::new(db);
        assert_eq!(
            read(&driver, "absent").await,
            Ok(PrivateReadResult::Unavailable)
        );
        assert_eq!(
            read(&driver, "   ").await,
            Ok(PrivateReadResult::Unavailable)
        );
        // Operator syntax is quoted, so it cannot widen or break the match.
        assert_eq!(
            read(&driver, "NEAR(* OR \" agent_id:other").await,
            Ok(PrivateReadResult::Unavailable)
        );
    }

    #[tokio::test]
    async fn cancelled_read_is_cancelled() {
        let db = backend().await;
        let driver = OwnerMemoryReadDriver::new(db);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            driver.read(&scope(), "x", &cancel).await,
            Err(PrivateReadError::Cancelled)
        );
    }
}
