//! `SQLite` + sqlite-vec memory backend.
//!
//! Provides the SQLite-backed persistence layer for the notes system
//! (Knowledge Notes + embeddings), raw memories, recall signals, and dream
//! reports. The legacy facts table has been removed.

pub mod schema;
pub mod vec;

pub mod dream_kv;
pub mod dream_reports;
pub mod embedding_meta;
pub mod memory_write_decisions;
pub mod notes;
pub mod query_filed;
pub mod raw_memories;
pub mod recall_signals;
pub mod routing_experience;
mod sessions;

use crate::error::AlephError;
use crate::sync_primitives::{Arc, Mutex};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};

/// Tunable knobs for hybrid (vector + FTS) retrieval fusion.
///
/// Defaults reproduce the historical hardcoded behaviour (RRF k=60,
/// BM25 lift 0.15) so every construction site that does not override
/// these is byte-for-byte unchanged.
#[derive(Debug, Clone, Copy)]
pub struct RetrievalTuning {
    /// Reciprocal Rank Fusion constant.
    pub rrf_k: u32,
    /// Extra multiplicative lift applied to FTS (lexical) matches in fusion.
    pub bm25_bonus_weight: f32,
}

impl Default for RetrievalTuning {
    fn default() -> Self {
        Self {
            rrf_k: 60,
            bm25_bonus_weight: 0.15,
        }
    }
}

/// SQLite-backed memory store using sqlite-vec for vector operations.
///
/// # Deferred risk: sync `Mutex<Connection>` blocking the tokio executor
///
/// `conn` is `Arc<std::sync::Mutex<Connection>>` (the `Arc` wrapper is
/// what makes `with_conn` below safe to drive from `spawn_blocking`).
/// `lock_conn!` in `notes/store_impl.rs` still holds the guard across a
/// full SQLite query — which is fine for single-shot ops but, under high
/// concurrency (many simultaneous note-store / dream-report / recall-signal
/// calls), causes the holding tokio worker thread to block until the query
/// returns. With several long queries queued, the executor's parallelism
/// collapses to one-at-a-time.
///
/// **Infra landed this commit, call-site migration deferred to next
/// sprint:** the field is now `Arc<Mutex<…>>` and [`SqliteMemoryBackend::with_conn`]
/// wraps a closure in `tokio::task::spawn_blocking`. Migrating the 48+
/// `lock_conn!(self)` sites in `notes/store_impl.rs` plus the ~18 sites
/// in the helper files (`dream_reports.rs`, `memory_write_decisions.rs`,
/// `query_filed.rs`, `raw_memories.rs`, `recall_signals.rs`,
/// `routing_experience.rs`) to the async helper is mechanical (each call
/// becomes `with_conn(...).await?`) but spans enough surface that doing
/// it in one review-round commit risks regressions across the 130+ store
/// tests. Call it out as a separate sprint item with its own testing
/// budget. Until then, treat high-concurrency note-store usage as a known
/// performance ceiling rather than a correctness bug.
///
/// See `review-results/` for the original risk write-up.
pub struct SqliteMemoryBackend {
    conn: Arc<Mutex<Connection>>,
    tuning: RetrievalTuning,
}

/// Build the `WHERE` clause and its positional bind values for the
/// user-facing `raw_memories` projection.
///
/// `count_raw_memories` and `get_raw_memories_dashboard` both go through
/// here, so a filtered count can never disagree with the filtered list. That
/// disagreement was the phantom-page bug: the console paired a *global* count
/// with an *agent-scoped* list, so "next page" stayed enabled forever and
/// eventually landed on an empty page.
///
/// `tool_invocation` rows are per-call telemetry consumed by the Dream cycle
/// by source, never user-facing memories, so they are excluded unconditionally.
fn raw_where(agent_id: Option<&str>, query: Option<&str>) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;

    let mut sql = String::from(" WHERE source != 'tool_invocation'");
    let mut binds: Vec<Value> = Vec::new();

    if let Some(agent) = agent_id {
        sql.push_str(" AND agent_id = ?");
        binds.push(Value::Text(agent.to_string()));
    }
    // A blank search box browses; it does not match every row containing a space.
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        sql.push_str(r" AND content LIKE ? ESCAPE '\'");
        binds.push(Value::Text(format!("%{}%", escape_like(q))));
    }

    (sql, binds)
}

/// Escape SQL `LIKE` metacharacters so a query containing `%`, `_` or `\`
/// matches them literally. Without this, searching for "100%" matches every
/// row. Pairs with the `ESCAPE '\'` clause emitted by [`raw_where`].
fn escape_like(q: &str) -> String {
    let mut out = String::with_capacity(q.len());
    for ch in q.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

impl SqliteMemoryBackend {
    /// Create a new `SqliteMemoryBackend` backed by the given database file.
    ///
    /// If `db_path` points to an existing directory, the database file will be
    /// created as `memory.db` inside that directory. Parent directories are
    /// created automatically when they do not exist.
    ///
    /// This will:
    /// 1. Register the sqlite-vec extension globally
    /// 2. Open (or create) the database at `db_path`
    /// 3. Set WAL journal mode for concurrent reads
    /// 4. Initialize the relational and vector schemas
    pub fn new(db_path: &Path) -> Result<Self, AlephError> {
        // If the caller passed a directory, place the DB file inside it
        let resolved: PathBuf = if db_path.is_dir() {
            db_path.join("memory.db")
        } else {
            db_path.to_path_buf()
        };

        // Ensure the parent directory exists so SQLite can create the DB file
        if let Some(parent) = resolved.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AlephError::config(format!(
                        "Failed to create directory for memory DB at {}: {e}",
                        parent.display()
                    ))
                })?;
            }
        }

        // Register sqlite-vec before opening any connection
        vec::register_sqlite_vec();

        let conn = crate::utils::sqlite_open::open_sqlite_safe(&resolved).map_err(|e| {
            AlephError::config(format!(
                "Failed to open memory database at {}: {e}",
                resolved.display()
            ))
        })?;

        // Initialize schemas
        schema::init_schema(&conn)?;
        schema::init_vec_tables(&conn)?;

        tracing::info!(
            path = %resolved.display(),
            "SqliteMemoryBackend initialized"
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            tuning: RetrievalTuning::default(),
        })
    }

    /// Test-only accessor for the underlying connection mutex. Used by
    /// write-amplification regression tests that need to read SQLite's
    /// `total_changes()` counter directly.
    #[cfg(test)]
    #[allow(dead_code)] // test-only accessor
    pub(crate) fn conn(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }

    /// Run a synchronous SQLite closure off the tokio executor thread.
    ///
    /// Acquires `self.conn` (a `std::sync::Mutex<Connection>` wrapped in
    /// `Arc` so the lock guard can be moved into a `spawn_blocking`
    /// closure) and invokes `f` on the blocking pool. Use this in
    /// preference to `lock_conn!` + inline work whenever the body holds
    /// the guard across any I/O — otherwise the calling tokio worker
    /// blocks until the SQLite query returns, and a queue of slow
    /// queries collapses executor parallelism (Risk 4 of the review
    /// backlog).
    ///
    /// Constraints (from `spawn_blocking`):
    /// - `f` must be `Send + 'static` and own everything it touches.
    /// - `R` must be `Send + 'static`.
    /// - `f` runs synchronously on a blocking-pool thread; do not `.await`
    ///   inside it.
    ///
    /// If a closure panics, `spawn_blocking` propagates a JoinError
    /// surfaced as `AlephError::other` — same shape as the
    /// `into_inner()` poison-recovery path used by `lock_conn!`.
    ///
    /// Migration is mechanical but spans 66+ call sites across
    /// `notes/store_impl.rs` and the 6 helper files; the call-site sweep
    /// is intentionally out of this commit (see the field doc-comment).
    pub async fn with_conn<F, R>(&self, f: F) -> Result<R, AlephError>
    where
        F: FnOnce(&Connection) -> Result<R, AlephError> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap_or_else(|e| {
                tracing::warn!(
                    "SqliteMemoryBackend: SQLite mutex was poisoned by a prior panic; \
                     recovering (this should be rare)"
                );
                e.into_inner()
            });
            f(&guard)
        })
        .await
        .map_err(|e| AlephError::other(format!("with_conn spawn_blocking join: {e}")))?
    }

    /// Create an in-memory `SqliteMemoryBackend` for testing.
    #[cfg(test)]
    pub fn in_memory() -> Result<Self, AlephError> {
        vec::register_sqlite_vec();

        let conn = Connection::open_in_memory()
            .map_err(|e| AlephError::config(format!("Failed to open in-memory DB: {e}")))?;

        schema::init_schema(&conn)?;
        schema::init_vec_tables(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            tuning: RetrievalTuning::default(),
        })
    }

    /// Override retrieval fusion tuning. Consumes and returns `self` so it
    /// can be chained right after `new()` at the live construction site,
    /// before the backend is wrapped in `Arc` and shared.
    pub const fn with_retrieval_tuning(mut self, rrf_k: u32, bm25_bonus_weight: f32) -> Self {
        self.tuning = RetrievalTuning {
            rrf_k,
            bm25_bonus_weight,
        };
        self
    }

    // ---- Dashboard helpers (raw_memories-based) -----------------------------

    /// Get raw memory entries (session summaries / conversation records).
    ///
    /// `query` is an optional substring filter over `content` (`LIKE '%q%'`).
    /// SQLite's default `LIKE` is case-insensitive for ASCII only — it does
    /// not case-fold non-ASCII text, so a search over this store's Chinese
    /// content is effectively case-sensitive there. `raw_memories` has no
    /// fts5 shadow table — this is a browse-UI filter, and building real FTS
    /// here would need a DDL migration plus sync triggers (deliberately out
    /// of scope).
    ///
    /// `offset` skips the first N rows of the `created_at DESC` ordering,
    /// enabling stable server-side pagination for the dashboard.
    ///
    /// The `WHERE` clause comes from [`raw_where`], shared with
    /// [`Self::count_raw_memories`] so list and count cannot drift.
    pub fn get_raw_memories_dashboard(
        &self,
        agent_id: Option<&str>,
        query: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::memory::store::raw_memory::RawMemory>, AlephError> {
        use crate::memory::store::raw_memory::{RawMemory, RawMemorySource};
        use rusqlite::types::Value;

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (where_sql, mut binds) = raw_where(agent_id, query);
        let sql = format!(
            "SELECT id, content, source, source_detail, agent_id, session_id, path, \
             attachment_text, is_processed, created_at \
             FROM raw_memories{where_sql} \
             ORDER BY created_at DESC LIMIT ? OFFSET ?"
        );
        binds.push(Value::Integer(limit as i64));
        binds.push(Value::Integer(offset as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_raw_memories_dashboard prepare: {e}")))?;

        let row_mapper = |row: &rusqlite::Row| -> rusqlite::Result<RawMemory> {
            let source_str: String = row.get("source")?;
            let source_detail: Option<String> = row.get("source_detail")?;
            let is_processed_int: i64 = row.get("is_processed")?;
            Ok(RawMemory {
                id: row.get("id")?,
                content: row.get("content")?,
                source: RawMemorySource::from_persisted(&source_str, source_detail.as_deref()),
                agent_id: row.get("agent_id")?,
                session_id: row.get("session_id")?,
                path: row.get("path")?,
                attachment_text: row.get("attachment_text")?,
                is_processed: is_processed_int != 0,
                created_at: row.get("created_at")?,
            })
        };

        let rows = stmt
            .query_map(rusqlite::params_from_iter(binds), row_mapper)
            .map_err(|e| AlephError::config(format!("get_raw_memories_dashboard failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| {
                AlephError::config(format!("get_raw_memories_dashboard row failed: {e}"))
            })?);
        }
        Ok(results)
    }

    /// Count user-facing raw memory entries under the same filter the
    /// dashboard list uses.
    ///
    /// Shares [`raw_where`] with [`Self::get_raw_memories_dashboard`]: pass the
    /// same `(agent_id, query)` and the count is guaranteed to describe exactly
    /// that list. Excludes `tool_invocation` telemetry.
    pub fn count_raw_memories(
        &self,
        agent_id: Option<&str>,
        query: Option<&str>,
    ) -> Result<i64, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (where_sql, binds) = raw_where(agent_id, query);
        let sql = format!("SELECT COUNT(*) FROM raw_memories{where_sql}");

        conn.query_row(&sql, rusqlite::params_from_iter(binds), |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_raw_memories failed: {e}")))
    }

    /// The `agent_id`/partition that owns a raw memory row, or `None` if no
    /// row has this id.
    ///
    /// The narrowest possible lookup for the P1 ownership check
    /// `memory.delete` needs (spec §11-1c): `delete_raw_memory` below
    /// addresses a row by bare `id` alone with no `agent_id`, so the caller
    /// must be checked against `visibility::partition_visible` BEFORE
    /// deleting — this answers "who owns this id" without widening the
    /// delete query itself.
    pub fn raw_memory_agent_id(&self, id: &str) -> Result<Option<String>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.query_row(
            "SELECT agent_id FROM raw_memories WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| AlephError::config(format!("raw_memory_agent_id failed: {e}")))
    }

    /// Delete a single raw memory entry by id.
    ///
    /// Returns `true` when a row was removed, `false` when no entry matched.
    /// Raw memories live in a self-contained table with no vector/foreign-key
    /// linkage, so this is a clean single-row delete.
    pub fn delete_raw_memory(&self, id: &str) -> Result<bool, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let affected = conn
            .execute(
                "DELETE FROM raw_memories WHERE id = ?1",
                rusqlite::params![id],
            )
            .map_err(|e| AlephError::config(format!("delete_raw_memory failed: {e}")))?;
        Ok(affected > 0)
    }
}

#[cfg(test)]
mod raw_where_tests {
    use super::{escape_like, raw_where};
    use rusqlite::types::Value;

    #[test]
    fn baseline_always_excludes_telemetry() {
        let (sql, binds) = raw_where(None, None);
        assert_eq!(sql, " WHERE source != 'tool_invocation'");
        assert!(binds.is_empty());
    }

    #[test]
    fn agent_scope_adds_one_bind() {
        let (sql, binds) = raw_where(Some("main"), None);
        assert_eq!(sql, " WHERE source != 'tool_invocation' AND agent_id = ?");
        assert_eq!(binds, vec![Value::Text("main".to_string())]);
    }

    #[test]
    fn query_adds_escaped_like_bind() {
        let (sql, binds) = raw_where(None, Some("deploy"));
        assert_eq!(
            sql,
            " WHERE source != 'tool_invocation' AND content LIKE ? ESCAPE '\\'"
        );
        assert_eq!(binds, vec![Value::Text("%deploy%".to_string())]);
    }

    #[test]
    fn agent_and_query_bind_in_positional_order() {
        let (sql, binds) = raw_where(Some("main"), Some("smoke"));
        assert_eq!(
            sql,
            " WHERE source != 'tool_invocation' AND agent_id = ? AND content LIKE ? ESCAPE '\\'"
        );
        assert_eq!(
            binds,
            vec![
                Value::Text("main".to_string()),
                Value::Text("%smoke%".to_string()),
            ]
        );
    }

    #[test]
    fn blank_query_is_not_a_filter() {
        // A whitespace-only search box must browse, not match every row that
        // happens to contain a space.
        assert_eq!(raw_where(None, Some("   ")).1.len(), 0);
        assert_eq!(raw_where(None, Some("")).1.len(), 0);
    }

    #[test]
    fn like_metacharacters_match_literally() {
        // Without escaping, a user searching for "100%" would match everything.
        assert_eq!(escape_like("100%"), r"100\%");
        assert_eq!(escape_like("a_b"), r"a\_b");
        assert_eq!(escape_like(r"c\d"), r"c\\d");
        assert_eq!(escape_like("plain"), "plain");
    }
}

// ---------------------------------------------------------------------------
// with_conn helper tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod with_conn_tests {
    use super::*;
    use rusqlite::params;

    /// Smoke test: `with_conn` runs the closure against the live connection
    /// and returns its result. We write + read through it to confirm the
    /// `Arc<Mutex<Connection>>` plumbing is sound end-to-end.
    #[tokio::test]
    async fn with_conn_round_trip_through_spawn_blocking() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();

        let write_count: i64 = backend
            .with_conn(|conn| {
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS t (k INTEGER PRIMARY KEY, v TEXT NOT NULL)",
                    [],
                )
                .map_err(|e| AlephError::config(format!("create: {e}")))?;
                conn.execute(
                    "INSERT INTO t (k, v) VALUES (?1, ?2)",
                    params![1i64, "alpha"],
                )
                .map_err(|e| AlephError::config(format!("insert: {e}")))?;
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM t", [], |row| row.get(0))
                    .map_err(|e| AlephError::config(format!("count: {e}")))?;
                Ok(count)
            })
            .await
            .expect("with_conn write path must succeed");

        assert_eq!(write_count, 1, "the inserted row must be visible");

        let read_back: String = backend
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT v FROM t WHERE k = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::config(format!("select: {e}")))
            })
            .await
            .expect("with_conn read path must succeed");

        assert_eq!(read_back, "alpha");
    }

    /// Errors raised inside the closure propagate as `AlephError` to the
    /// caller. The point of this test is to ensure `spawn_blocking`'s
    /// join boundary does not swallow them.
    #[tokio::test]
    async fn with_conn_propagates_closure_errors() {
        let backend = SqliteMemoryBackend::in_memory().unwrap();
        let result = backend
            .with_conn(|_conn| -> Result<(), AlephError> {
                Err(AlephError::other("intentional closure failure"))
            })
            .await;
        let err = result.expect_err("closure error must propagate");
        assert!(
            err.to_string().contains("intentional closure failure"),
            "the original message must survive spawn_blocking; got: {err}"
        );
    }

    /// `with_conn` runs the closure concurrently with other tasks — the
    /// whole point of wrapping it in `spawn_blocking` is to keep the
    /// tokio executor free. Spawn N parallel `with_conn` calls and assert
    /// they all complete and observe a coherent connection state.
    ///
    /// (This is a structural assertion: even if SQLite serializes the
    /// writes, the helper must let the runtime schedule them
    /// concurrently, not block the worker thread.)
    #[tokio::test]
    async fn with_conn_supports_concurrent_callers() {
        let backend = Arc::new(SqliteMemoryBackend::in_memory().unwrap());

        backend
            .with_conn(|conn| {
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS counter (n INTEGER)",
                    [],
                )
                .map_err(|e| AlephError::config(format!("create: {e}")))?;
                conn.execute("INSERT INTO counter (n) VALUES (0)", [])
                    .map_err(|e| AlephError::config(format!("seed: {e}")))?;
                Ok(())
            })
            .await
            .unwrap();

        const N: usize = 32;
        let mut handles = Vec::with_capacity(N);
        for _ in 0..N {
            let backend = Arc::clone(&backend);
            handles.push(tokio::spawn(async move {
                backend
                    .with_conn(|conn| {
                        conn.execute("UPDATE counter SET n = n + 1", [])
                            .map_err(|e| AlephError::config(format!("inc: {e}")))?;
                        Ok(())
                    })
                    .await
            }));
        }

        for h in handles {
            h.await.expect("task must not panic").expect("with_conn must succeed");
        }

        let final_count: i64 = backend
            .with_conn(|conn| {
                conn.query_row("SELECT n FROM counter", [], |row| row.get(0))
                    .map_err(|e| AlephError::config(format!("read: {e}")))
            })
            .await
            .unwrap();
        assert_eq!(final_count, N as i64, "every concurrent increment must persist");
    }
}
