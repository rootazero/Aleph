//! `SQLite` + sqlite-vec memory backend.
//!
//! Provides the SQLite-backed persistence layer for the notes system
//! (Knowledge Notes + embeddings), recall signals, and dream reports.
//! Raw memories use the Markdown-backed `RawMemoryStore` instead.

pub mod schema;
pub mod vec;

pub mod dream_kv;
pub mod dream_reports;
pub mod embedding_meta;
pub mod memory_write_decisions;
pub mod notes;
pub mod query_filed;
pub mod recall_signals;
mod sessions;

use crate::error::AlephError;
use crate::sync_primitives::{Arc, Mutex};
use rusqlite::Connection;
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
/// All SQL enters through the same checked connection slot. Closing the slot
/// never reopens it; a later runtime start constructs a fresh backend. The raw
/// Markdown store owns its independent connection to the same database file.
///
/// Synchronous store methods still hold a standard mutex during their queries.
/// `with_conn` moves longer work onto the blocking pool; converting every store
/// method to asynchronous blocking work remains a separate performance change.
pub struct SqliteMemoryBackend {
    conn: Arc<Mutex<Option<Connection>>>,
    tuning: RetrievalTuning,
}

/// Escape SQL `LIKE` metacharacters so a query containing `%`, `_` or `\\`
/// matches them literally. Without this, searching for "100%" matches every
/// row. The memory-write decision reader pairs this with `ESCAPE '\\'`.
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
    /// created as `metadata.db` inside that directory. Parent directories are
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
            db_path.join("metadata.db")
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
            conn: Arc::new(Mutex::new(Some(conn))),
            tuning: RetrievalTuning::default(),
        })
    }

    /// Close the actual connection, retaining it when SQLite refuses the close.
    /// Callers must drain admitted work before reaching this boundary.
    pub(crate) fn close(&self) -> Result<(), AlephError> {
        let mut slot = self.conn.lock().unwrap_or_else(|error| {
            tracing::warn!("SqliteMemoryBackend: recovering poisoned connection during close");
            error.into_inner()
        });
        let Some(connection) = slot.take() else {
            return Ok(());
        };
        match connection.close() {
            Ok(()) => Ok(()),
            Err((connection, error)) => {
                *slot = Some(connection);
                Err(AlephError::other(format!(
                    "memory database close failed: {error}"
                )))
            }
        }
    }

    /// Keep borrowed statements and transactions inside this checked boundary.
    fn with_connection<R>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<R, AlephError>,
    ) -> Result<R, AlephError> {
        Self::access_connection(&self.conn, work)
    }

    fn access_connection<R>(
        slot: &Mutex<Option<Connection>>,
        work: impl FnOnce(&mut Connection) -> Result<R, AlephError>,
    ) -> Result<R, AlephError> {
        let mut guard = slot.lock().unwrap_or_else(|error| {
            tracing::warn!(
                "SqliteMemoryBackend: SQLite mutex was poisoned by a prior panic; \
                 recovering (this should be rare)"
            );
            error.into_inner()
        });
        let connection = guard
            .as_mut()
            .ok_or_else(|| AlephError::other("memory database is closed"))?;
        work(connection)
    }

    /// Test-only accessor for the underlying connection mutex. Used by
    /// write-amplification regression tests that need to read SQLite's
    /// `total_changes()` counter directly.
    #[cfg(test)]
    #[allow(dead_code)] // test-only accessor
    pub(crate) fn conn(&self) -> &Arc<Mutex<Option<Connection>>> {
        &self.conn
    }

    /// Run checked SQLite work on the blocking pool. A closure queued before
    /// close but started afterwards fails without touching the closed database.
    /// Owned runtime cancellation is layered onto this boundary by MemoryRuntime.
    pub async fn with_conn<F, R>(&self, f: F) -> Result<R, AlephError>
    where
        F: FnOnce(&Connection) -> Result<R, AlephError> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            Self::access_connection(&conn, |connection| f(connection))
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
            conn: Arc::new(Mutex::new(Some(conn))),
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
}

// ---------------------------------------------------------------------------
// with_conn helper tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod lifecycle_tests;

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
                conn.query_row("SELECT v FROM t WHERE k = 1", [], |row| row.get(0))
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
                conn.execute("CREATE TABLE IF NOT EXISTS counter (n INTEGER)", [])
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
            h.await
                .expect("task must not panic")
                .expect("with_conn must succeed");
        }

        let final_count: i64 = backend
            .with_conn(|conn| {
                conn.query_row("SELECT n FROM counter", [], |row| row.get(0))
                    .map_err(|e| AlephError::config(format!("read: {e}")))
            })
            .await
            .unwrap();
        assert_eq!(
            final_count, N as i64,
            "every concurrent increment must persist"
        );
    }
}
