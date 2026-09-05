use super::migration;
/// Core `StateDatabase` struct and initialization
///
/// Contains the database connection, schema setup, and migration logic.
/// Schema DDL is in `schema.rs`, tests are in `tests.rs`.
use crate::error::AlephError;
use crate::sync_primitives::{Arc, Mutex};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

/// Direct extern declaration of the sqlite-vec auto-extension entrypoint.
///
/// The upstream `sqlite-vec` crate exposes `sqlite3_vec_init` with an
/// argument-less stub (`pub fn sqlite3_vec_init();`), but the real C symbol
/// has the canonical 3-arg SQLite extension signature declared in
/// `sqlite-vec.h`:
///
/// ```c
/// int sqlite3_vec_init(sqlite3 *db,
///                      char **pzErrMsg,
///                      const sqlite3_api_routines *pApi);
/// ```
///
/// Declaring the symbol ourselves with the correct signature makes the
/// **linker** enforce ABI compatibility: a future sqlite-vec release that
/// adds attributes, changes arity, or changes the calling convention will
/// fail to link here instead of silently corrupting the stack on every
/// extension load. The previous implementation used
/// `std::mem::transmute(sqlite3_vec_init as *const ())` to bridge the
/// 0-arg stub and the 3-arg real signature — that erased the contract.
unsafe extern "C" {
    fn sqlite3_vec_init(
        db: *mut rusqlite::ffi::sqlite3,
        pz_err_msg: *mut *mut std::ffi::c_char,
        p_api: *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::ffi::c_int;
}

mod schema;
#[cfg(test)]
mod tests;

/// Default embedding dimension for vector search
pub const DEFAULT_EMBEDDING_DIM: u32 = 1024;

/// State database for resilience state management (agent events, tasks, traces)
pub struct StateDatabase {
    pub(crate) conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)] // Stored for diagnostics and test assertions (e.g. in_memory check)
    pub(crate) db_path: PathBuf,
}

impl StateDatabase {
    /// Register sqlite-vec extension for all connections
    fn register_sqlite_vec_extension() -> Result<(), AlephError> {
        // Register sqlite-vec extension before opening any connection.
        //
        // SAFETY: `sqlite3_auto_extension` stores the entrypoint for later
        // invocation by SQLite when a new connection is opened. We pass the
        // FFI symbol with its real signature (declared above), so SQLite's
        // 3-arg call lands on a function with the matching arity and
        // calling convention. `sqlite3_auto_extension` is idempotent for a
        // given pointer, so re-registering from every `new()` /
        // `in_memory()` is safe — SQLite checks the entrypoint slot before
        // appending.
        // rust-doctor-disable-next-line unsafe-block-audit
        unsafe {
            let rc = rusqlite::ffi::sqlite3_auto_extension(Some(sqlite3_vec_init));
            if rc != rusqlite::ffi::SQLITE_OK {
                return Err(AlephError::config(format!(
                    "Failed to register sqlite-vec extension: error code {rc}"
                )));
            }
        }
        Ok(())
    }

    /// Create the database schema
    fn create_schema(conn: &Connection, embedding_dim: u32) -> Result<(), AlephError> {
        conn.execute_batch(Self::schema_sql())
            .map_err(|e| AlephError::config(format!("Failed to create schema: {e}")))?;
        conn.execute_batch(&Self::vec_schema_sql(embedding_dim))
            .map_err(|e| AlephError::config(format!("Failed to create vec0 tables: {e}")))?;
        Ok(())
    }

    fn ensure_parent_dir(path: &std::path::Path) -> Result<(), AlephError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AlephError::config(format!("Failed to create database directory: {e}"))
            })?;
        }
        Ok(())
    }

    fn open_db(path: &std::path::Path) -> Result<Connection, AlephError> {
        crate::utils::sqlite_open::open_sqlite_safe(path)
            .map_err(|e| AlephError::config(format!("Failed to open database: {e}")))
    }

    fn validate_embedding_dim(embedding_dim: u32) -> Result<(), AlephError> {
        if embedding_dim == 0 || embedding_dim > 16_384 {
            return Err(AlephError::config(format!(
                "embedding dimension must be in 1..=16384, got {embedding_dim}"
            )));
        }
        Ok(())
    }

    fn drop_memories_for_dim_change(
        conn: &Connection,
        changed: bool,
        new_dim: u32,
        message: &str,
    ) -> Result<(), AlephError> {
        if !changed {
            return Ok(());
        }
        conn.execute_batch(
            "DROP TABLE IF EXISTS memories; DROP TABLE IF EXISTS memories_vec; DROP TABLE IF EXISTS memories_fts;",
        )
        .map_err(|e| AlephError::config(format!("Failed to drop memories tables: {e}")))?;
        tracing::info!(new_dim = new_dim, "{}", message);
        Ok(())
    }

    fn run_optional_migrations(conn: &Connection) -> Result<(), AlephError> {
        // NOTE: `migrate_add_experience_replays` was removed 2026-08-16 — the
        // experience_replays / experiences_vec tables it created had zero
        // readers or writers anywhere in the tree (Cortex consumer never
        // landed). Existing databases keep their orphaned tables (harmless);
        // new databases simply don't create them.
        migration::migrate_task_traces_to_agent_trace(conn)?;
        migration::migrate_task_traces_unique_step_index(conn)?;
        migration::migrate_add_channel_offsets(conn)?;
        migration::migrate_add_paired_users(conn)?;
        migration::migrate_add_sticker_descriptions(conn)?;
        migration::migrate_add_group_chat_owner(conn)?;
        Ok(())
    }

    fn record_embedding_dim(conn: &Connection, dim: u32) -> Result<(), AlephError> {
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![dim.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update schema_info: {e}")))?;
        Ok(())
    }

    /// Create an in-memory database for testing
    ///
    /// This creates a SQLite database in memory (:memory:) without persisting to disk.
    /// Useful for unit tests that need isolated database instances.
    /// Also reachable under `test-helpers`: `memory::events::testing` is
    /// gated on that feature and calls this, so a `cfg(test)`-only gate made
    /// the integration-test build (`--features test-helpers --test '*'`,
    /// where `cfg(test)` is false for the lib) fail to compile.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn in_memory() -> Result<Self, AlephError> {
        Self::register_sqlite_vec_extension()?;

        let conn = Connection::open_in_memory()
            .map_err(|e| AlephError::config(format!("Failed to open in-memory database: {}", e)))?;

        // Mirror the production pragmas so FK violations surface in tests
        // exactly where they would in production — otherwise `foreign_keys=ON`
        // regressions can land undetected. Without this, an in-memory test
        // happily inserts an orphan `task_traces` row referencing a
        // non-existent `agent_tasks.id` and never exercises the constraint.
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=MEMORY;
             PRAGMA synchronous=OFF;",
        )
        .map_err(|e| AlephError::config(format!("Failed to set in-memory pragmas: {e}")))?;

        // Initialize the database schema
        Self::create_schema(&conn, DEFAULT_EMBEDDING_DIM)?;

        // Update embedding dimension in schema_info
        conn.execute(
            "INSERT OR REPLACE INTO schema_info (key, value) VALUES ('embedding_dimension', ?1)",
            params![DEFAULT_EMBEDDING_DIM.to_string()],
        )
        .map_err(|e| AlephError::config(format!("Failed to update schema_info: {}", e)))?;

        // Drop obsolete tables (matches `new()` / `new_with_dim()`).
        // Without this, a future migration that drops a real table leaves
        // in-memory tests with a stale schema that disagrees with production.
        Self::drop_obsolete_tables(&conn).map_err(|e| {
            AlephError::config(format!("Failed to drop obsolete facts tables: {e}"))
        })?;

        // Run migrations (same as new()) so in-memory DBs have full schema
        migration::migrate_task_traces_to_agent_trace(&conn)?;
        migration::migrate_task_traces_unique_step_index(&conn)?;
        migration::migrate_add_channel_offsets(&conn)?;
        migration::migrate_add_paired_users(&conn)?;
        migration::migrate_add_sticker_descriptions(&conn)?;
        migration::migrate_add_group_chat_owner(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: PathBuf::from(":memory:"),
        })
    }

    /// Run a synchronous SQLite closure off the tokio executor thread.
    ///
    /// Mirrors [`crate::memory::store::sqlite::SqliteMemoryBackend::with_conn`]
    /// for the resilience layer. Acquires `self.conn` (an
    /// `Arc<std::sync::Mutex<Connection>>`) and invokes `f` on the
    /// blocking pool via `tokio::task::spawn_blocking`. Use this in
    /// preference to `self.conn.lock()` inside any `async fn` whose
    /// body holds the guard across I/O — otherwise the calling tokio
    /// worker blocks until the SQLite query returns, and a queue of
    /// slow queries collapses executor parallelism (Risk 4 of the
    /// review backlog).
    ///
    /// Constraints (from `spawn_blocking`):
    /// - `f` must be `Send + 'static` and own everything it touches
    ///   (every borrowed parameter of the enclosing `async fn` must
    ///   be cloned to an owned value at the function head before the
    ///   closure starts).
    /// - `R` must be `Send + 'static`.
    /// - `f` runs synchronously on a blocking-pool thread; do not
    ///   `.await` inside it.
    ///
    /// The 27 async-fnned lock sites across `memory_events.rs`,
    /// `traces.rs`, `tasks.rs` are migrated in a follow-up commit;
    /// sync methods (`sticker_descriptions`, `channel_offsets`,
    /// `group_chat`) cannot await and remain on the direct `lock()`
    /// path.
    pub async fn with_conn<F, R>(&self, f: F) -> Result<R, AlephError>
    where
        F: FnOnce(&mut Connection) -> Result<R, AlephError> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            // On a poisoned mutex, surface the failure to the caller instead
            // of silently reusing the connection. A panic inside an earlier
            // `f` leaves the SQLite handle in an unknown state — possibly
            // mid-statement, mid-transaction, or with FK constraints
            // half-checked — so continuing to use it can corrupt subsequent
            // writes. Returning `AlephError` lets the caller decide whether
            // to reopen the database, surface a doctor alert, or treat the
            // database as unhealthy.
            //
            // The previous `unwrap_or_else(|e| e.into_inner())` recovery was
            // louder than helpful: every long-lived daemon re-logged the
            // warning on every operation after the first panic, flooding the
            // log without actually preventing the inconsistency.
            let mut guard = conn.lock().map_err(|e| {
                AlephError::other(format!(
                    "StateDatabase SQLite mutex poisoned by a prior panic; \
                     refusing to reuse the half-broken connection ({e})"
                ))
            })?;
            f(&mut guard)
        })
        .await
        .map_err(|e| AlephError::other(format!("StateDatabase with_conn join: {e}")))?
    }

    /// Initialize vector database with schema
    ///
    /// Includes migration logic for embedding dimension changes.
    /// When embedding dimension changes (e.g., 384 -> 1024), old data is cleared.
    pub fn new(db_path: PathBuf) -> Result<Self, AlephError> {
        Self::ensure_parent_dir(&db_path)?;
        Self::register_sqlite_vec_extension()?;

        let conn = Self::open_db(&db_path)?;

        let needs_migration = Self::check_needs_migration(&conn)?;
        // Drop the old memories table AND its derived vec0 index for the
        // dimension migration. `memories_vec` is a vector index over
        // `memories.embedding`; if only `memories` is dropped, the
        // subsequent `CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec`
        // keeps the stale-dimension table (IF NOT EXISTS is a no-op),
        // leaving schema_info and the vec0 table disagreeing on the
        // dimension and breaking every later embedding insert. Dropping
        // both lets create_schema rebuild memories_vec at the new
        // dimension, mirroring new_with_dim(). `memories_fts` is an
        // external-content FTS5 index over `memories`; dropping `memories`
        // does not drop it, so without an explicit drop it would retain
        // stale rows pointing at rowids that no longer exist (phantom
        // search hits). create_schema recreates it empty alongside the
        // memories table and its sync triggers.
        Self::drop_memories_for_dim_change(
            &conn,
            needs_migration,
            DEFAULT_EMBEDDING_DIM,
            "Cleared memories + memories_vec + memories_fts tables for embedding dimension migration",
        )?;

        Self::create_schema(&conn, DEFAULT_EMBEDDING_DIM)?;

        Self::drop_obsolete_tables(&conn).map_err(|e| {
            AlephError::config(format!("Failed to drop obsolete facts tables: {e}"))
        })?;

        Self::run_optional_migrations(&conn)?;
        Self::migrate_to_vec0(&conn)?;
        Self::record_embedding_dim(&conn, DEFAULT_EMBEDDING_DIM)?;

        tracing::info!(
            subsystem = "resilience",
            event = "database_initialized",
            db_path = %db_path.display(),
            embedding_dim = DEFAULT_EMBEDDING_DIM,
            "StateDatabase initialized"
        );

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Initialize vector database with a specific embedding dimension
    ///
    /// Use this when the embedding dimension is known from configuration.
    pub fn new_with_dim(db_path: PathBuf, embedding_dim: u32) -> Result<Self, AlephError> {
        Self::validate_embedding_dim(embedding_dim)?;
        Self::ensure_parent_dir(&db_path)?;
        Self::register_sqlite_vec_extension()?;

        let conn = Self::open_db(&db_path)?;

        let dim_changed = Self::check_dimension_change(&conn, embedding_dim)?;
        // Drop `memories` alongside `memories_vec` (and the derived FTS
        // index), mirroring `new()`. Dropping only `memories_vec` would
        // leave stale, old-dimension embedding BLOBs in `memories`; nothing
        // re-indexes them here (migrate_to_vec0 is skipped below on a
        // dimension change), and on the *next* restart — where the stored
        // dimension matches and `dim_changed` is false — migrate_to_vec0
        // would copy those old-dimension blobs into the new-dimension
        // `memories_vec`, hitting a vec0 dimension-mismatch and failing
        // startup. Clearing the rows keeps the dimension migration safe
        // across restarts.
        Self::drop_memories_for_dim_change(
            &conn,
            dim_changed,
            embedding_dim,
            "Cleared memories + memories_vec + memories_fts for dimension change. Embeddings will be re-indexed.",
        )?;

        Self::create_schema(&conn, embedding_dim)?;

        Self::drop_obsolete_tables(&conn).map_err(|e| {
            AlephError::config(format!("Failed to drop obsolete facts tables: {e}"))
        })?;

        Self::run_optional_migrations(&conn)?;

        if !dim_changed {
            Self::migrate_to_vec0(&conn)?;
        }

        Self::record_embedding_dim(&conn, embedding_dim)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Check if the configured dimension differs from the stored dimension
    fn check_dimension_change(conn: &Connection, target_dim: u32) -> Result<bool, AlephError> {
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_info'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to check schema_info table: {e}")))?;

        if !table_exists {
            return Ok(false);
        }

        let stored: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to read embedding dimension: {e}")))?;

        match stored {
            Some(dim_str) => match dim_str.parse::<u32>() {
                Ok(stored_dim) if stored_dim == target_dim => Ok(false),
                Ok(stored_dim) => {
                    tracing::info!(
                        stored_dim = stored_dim,
                        target_dim = target_dim,
                        "Embedding dimension change detected"
                    );
                    Ok(true)
                }
                Err(_) => {
                    tracing::warn!(
                        stored_dim = %dim_str,
                        target_dim = target_dim,
                        "Invalid stored embedding dimension, treating as changed"
                    );
                    Ok(true)
                }
            },
            None => Ok(false),
        }
    }

    /// Migrate existing memories and facts to vec0 tables
    fn migrate_to_vec0(conn: &Connection) -> Result<(), AlephError> {
        // Check if migration needed (vec tables exist but empty, memories table has data)
        let memories_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("Failed to count memories: {e}")))?;

        let vec_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM memories_vec", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("Failed to count memories_vec: {e}")))?;

        if memories_count > 0 && vec_count == 0 {
            tracing::info!(
                memories_count = memories_count,
                "Migrating existing memories to vec0 table"
            );

            // Migrate memories
            conn.execute(
                r#"
                INSERT INTO memories_vec (rowid, embedding)
                SELECT rowid, embedding FROM memories WHERE embedding IS NOT NULL
                "#,
                [],
            )
            .map_err(|e| AlephError::config(format!("Failed to migrate memories to vec0: {e}")))?;

            tracing::info!("Memories migration complete");
        }

        Ok(())
    }

    /// Check if database needs migration due to dimension change
    fn check_needs_migration(conn: &Connection) -> Result<bool, AlephError> {
        // Check if schema_info table exists
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='schema_info'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("Failed to check schema_info table: {e}")))?;

        if !table_exists {
            // Check if memories table exists (old database without schema_info)
            let memories_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='memories'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::config(format!("Failed to check memories table: {e}")))?;

            // If old memories table exists but no schema_info, needs migration
            return Ok(memories_exists);
        }

        // Check current dimension in schema_info
        let current_dimension: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_info WHERE key = 'embedding_dimension'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to read embedding dimension: {e}")))?;

        match current_dimension {
            Some(dim_str) => match dim_str.parse::<u32>() {
                Ok(dim) if dim == DEFAULT_EMBEDDING_DIM => Ok(false), // Already at current dimension
                Ok(dim) => {
                    tracing::info!(
                        stored_dim = dim,
                        current_dim = DEFAULT_EMBEDDING_DIM,
                        "Embedding dimension mismatch detected"
                    );
                    Ok(true) // Needs migration (different dimension)
                }
                Err(_) => {
                    tracing::warn!(
                        stored_dim = %dim_str,
                        current_dim = DEFAULT_EMBEDDING_DIM,
                        "Invalid stored embedding dimension, treating as mismatch"
                    );
                    Ok(true)
                }
            },
            None => Ok(true), // No dimension stored, needs migration
        }
    }

    /// Serialize embedding vector to bytes (f32 array -> bytes)
    #[must_use]
    pub fn serialize_embedding(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Store or update a sticker description in the cache.
    pub async fn store_sticker_description(
        &self,
        file_unique_id: &str,
        description: &str,
    ) -> Result<(), AlephError> {
        let file_unique_id = file_unique_id.to_string();
        let description = description.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO sticker_descriptions (file_unique_id, description, cached_at) VALUES (?1, ?2, datetime('now'))",
            params![file_unique_id, description],
            )
            .map_err(|e| AlephError::config(format!("Failed to store sticker description: {e}")))?;
            Ok(())
        })
        .await
    }

    /// Load a cached sticker description by its unique file id.
    pub async fn load_sticker_description(
        &self,
        file_unique_id: &str,
    ) -> Result<Option<String>, AlephError> {
        let file_unique_id = file_unique_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT description FROM sticker_descriptions WHERE file_unique_id = ?1 LIMIT 1",
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare sticker query: {e}")))?;
            let mut rows = stmt
                .query(params![file_unique_id])
                .map_err(|e| AlephError::config(format!("Failed to query sticker description: {e}")))?;
            if let Some(row) = rows
                .next()
                .map_err(|e| AlephError::config(format!("Failed to read sticker row: {e}")))?
            {
                row.get(0)
                    .map_err(|e| {
                        AlephError::config(format!("Failed to deserialize sticker description: {e}"))
                    })
                    .map(Some)
            } else {
                Ok(None)
            }
        })
        .await
    }
}
