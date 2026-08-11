//! Schema migration for the swarm task store.
//!
//! Sync function over `&rusqlite::Connection` so the caller owns the
//! `tokio::sync::Mutex` lock acquisition (see [`super::SqliteCoordTaskStore::migrate`]).
//!
//! Also handles legacy schema cleanup: earlier versions stored team data
//! in `coord_teams` / `coord_team_members` tables inside this database and
//! added a `FOREIGN KEY (team_id) REFERENCES coord_teams(id)` on
//! `coord_tasks`.  Team management has since moved to a separate `teams.db`
//! via `TeamStore`, so the FK now causes inserts to fail.  When the legacy
//! tables are detected we rebuild `coord_tasks` without the stale FK.

use rusqlite::Connection;

use super::helpers::db_err;

pub(super) fn migrate(conn: &Connection) -> crate::error::Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(db_err)?;

    // --- Legacy schema migration -------------------------------------------
    // Detect old `coord_teams` table whose FK on `coord_tasks.team_id`
    // blocks inserts (teams now live in teams.db).
    let has_legacy: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='coord_teams'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0i64)
        > 0;

    if has_legacy {
        // Must disable FK checks while we recreate the table, otherwise
        // the data copy itself could trip the stale constraint.
        conn.execute_batch("PRAGMA foreign_keys = OFF;")
            .map_err(db_err)?;

        let migration_result = conn.execute_batch(
            r#"
            BEGIN;

            -- Rebuild coord_tasks without the stale FK to coord_teams
            CREATE TABLE coord_tasks_new (
                id TEXT PRIMARY KEY,
                team_id TEXT,
                subject TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                owner TEXT,
                priority TEXT NOT NULL DEFAULT 'normal',
                result TEXT,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL,
                started_at INTEGER,
                completed_at INTEGER
            );
            -- Explicit column list: a positional `SELECT *` breaks if the source
            -- table picked up extra columns (e.g. the locked_by/locked_at locking
            -- ALTERs) before this rebuild ran, supplying more values than the
            -- 12-column target accepts.
            INSERT INTO coord_tasks_new
                (id, team_id, subject, description, status, owner, priority,
                 result, metadata, created_at, started_at, completed_at)
            SELECT
                id, team_id, subject, description, status, owner, priority,
                result, metadata, created_at, started_at, completed_at
            FROM coord_tasks;
            DROP TABLE coord_tasks;
            ALTER TABLE coord_tasks_new RENAME TO coord_tasks;

            -- Drop legacy team tables that are no longer used
            DROP TABLE IF EXISTS coord_team_members;
            DROP TABLE IF EXISTS coord_teams;

            COMMIT;
            "#,
        );

        if let Err(e) = migration_result {
            // Best-effort rollback so the connection is not left with FK
            // enforcement disabled and an open transaction.
            let _ = conn.execute_batch("ROLLBACK; PRAGMA foreign_keys = ON;");
            return Err(db_err(e));
        }

        // Re-enable FK enforcement
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(db_err)?;

        tracing::info!("coord_tasks: migrated away from legacy coord_teams FK");
    }

    // --- Standard schema ---------------------------------------------------
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS coord_tasks (
            id TEXT PRIMARY KEY,
            team_id TEXT,
            subject TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'pending',
            owner TEXT,
            priority TEXT NOT NULL DEFAULT 'normal',
            result TEXT,
            metadata TEXT NOT NULL DEFAULT '{}',
            created_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS coord_task_dependencies (
            task_id TEXT NOT NULL,
            depends_on TEXT NOT NULL,
            PRIMARY KEY (task_id, depends_on),
            FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE,
            FOREIGN KEY (depends_on) REFERENCES coord_tasks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_coord_tasks_team_status ON coord_tasks(team_id, status);
        CREATE INDEX IF NOT EXISTS idx_coord_tasks_owner ON coord_tasks(owner);
        -- B6-03: index on coord_task_dependencies(depends_on). The edge
        -- table's only prior index is the implicit PK (task_id, depends_on),
        -- which by the leftmost-prefix rule cannot serve any lookup keyed on
        -- depends_on. Three production paths are keyed exactly that way:
        --   * deps::get_dependents  WHERE depends_on = ?1
        --   * deps::get_newly_unblocked  JOIN ... WHERE d.depends_on = ?1
        --     (runs on every task completion)
        --   * FK child scan for delete_team_tasks (O(deleted_tasks x total_edges))
        -- NOTE: `--`, not `//`. This is a SQL string, not Rust: `//` made
        -- `execute_batch` fail with `near "/": syntax error` at the FIRST
        -- statement, so the whole coord schema never ran and every
        -- `CoordTaskStore::new` returned Err. Boot treats that as the
        -- supported warn-and-continue degradation, so the coordination-task
        -- subsystem went dark with no error a user could see.
        CREATE INDEX IF NOT EXISTS idx_coord_task_deps_depends_on
            ON coord_task_dependencies(depends_on);
        "#,
    )
    .map_err(db_err)?;

    // --- Add task locking columns (idempotent) ---
    let has_locked_by: bool = conn
        .prepare("SELECT locked_by FROM coord_tasks LIMIT 0")
        .is_ok();
    if !has_locked_by {
        conn.execute_batch(
            "ALTER TABLE coord_tasks ADD COLUMN locked_by TEXT;\
             ALTER TABLE coord_tasks ADD COLUMN locked_at INTEGER;",
        )
        .map_err(db_err)?;
    }

    // --- Per-attempt run history (additive) ---
    // Captures each dispatcher claim so the panel drawer can show what
    // each attempt did, even after the task itself reaches a terminal
    // state. Older databases get the table at first migration.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS coord_task_runs (
            id         TEXT PRIMARY KEY,
            task_id    TEXT NOT NULL,
            agent_id   TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            ended_at   INTEGER,
            status     TEXT NOT NULL,
            summary    TEXT,
            error      TEXT,
            FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_coord_task_runs_task
            ON coord_task_runs(task_id, started_at);
        "#,
    )
    .map_err(db_err)?;

    // Phase C — step-level review fields on the per-attempt history.
    // Additive: existing dbs get NULL on these columns, matching the
    // "no review recorded" semantic. Idempotent via PRAGMA introspection.
    add_column_if_missing(conn, "coord_task_runs", "review_verdict", "TEXT")?;
    add_column_if_missing(conn, "coord_task_runs", "reviewer_kind", "TEXT")?;
    add_column_if_missing(conn, "coord_task_runs", "reviewer_id", "TEXT")?;

    // --- Per-task comments (additive) ---
    // Free-text handoff notes; permanent (no TTL) so they survive across
    // retries. MessageStore is intentionally not reused — it carries
    // inbox/TTL semantics that don't fit a comment thread.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS coord_task_comments (
            id         TEXT PRIMARY KEY,
            task_id    TEXT NOT NULL,
            author     TEXT NOT NULL,
            body       TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_coord_task_comments_task
            ON coord_task_comments(task_id, created_at);
        "#,
    )
    .map_err(db_err)?;

    // --- Team snapshots (additive) ---
    // Point-in-time JSON bundles of a team's config + members + tasks +
    // recent events. Stored here (not teams.db) so the snapshot read path
    // shares one DB lock with the bulk content (tasks). Restore is
    // dry-run by default in the tool layer.
    //
    // Inspired by ClawTeam's SnapshotManager. No FK to coord_tasks —
    // snapshots are deliberately decoupled from current task lifecycle
    // (deleting a task must not delete historical snapshots referencing it).
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS coord_team_snapshots (
            id         TEXT PRIMARY KEY,
            team_id    TEXT NOT NULL,
            tag        TEXT NOT NULL DEFAULT '',
            created_at INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            payload    TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_coord_team_snapshots_team
            ON coord_team_snapshots(team_id, created_at DESC);
        "#,
    )
    .map_err(db_err)?;

    // --- Exit journals (R3 — ClawTeam parity) ---
    // One journal per task (PRIMARY KEY on task_id). UPSERT lets an agent
    // rewrite its journal during retries — only the latest snapshot is
    // canonical. Older runs are preserved in `coord_task_runs`.
    //
    // List-fields (decisions / artifacts_ref / next_steps) are stored as
    // JSON arrays of strings. SQLite has no native array; we serialise +
    // parse at the store boundary so the public type stays Rust-native.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS coord_task_journals (
            task_id        TEXT PRIMARY KEY,
            agent_id       TEXT NOT NULL,
            summary        TEXT NOT NULL,
            decisions      TEXT NOT NULL DEFAULT '[]',
            artifacts_ref  TEXT NOT NULL DEFAULT '[]',
            next_steps     TEXT NOT NULL DEFAULT '[]',
            confidence     INTEGER,
            created_at     INTEGER NOT NULL,
            FOREIGN KEY (task_id) REFERENCES coord_tasks(id) ON DELETE CASCADE
        );
        "#,
    )
    .map_err(db_err)?;

    Ok(())
}

/// Idempotently add a column to an existing table.
///
/// `SQLite` raises on duplicate columns, so we inspect `PRAGMA table_info`
/// first instead of relying on error-string matching. Used by Phase C
/// (step-review fields) and any future additive migration.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    type_decl: &str,
) -> crate::error::Result<()> {
    fn is_safe_identifier(s: &str) -> bool {
        !s.is_empty()
            && s.starts_with(|c: char| c.is_ascii_alphabetic())
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    fn is_safe_type_decl(s: &str) -> bool {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_() ".contains(c))
    }
    if !is_safe_identifier(table) || !is_safe_identifier(column) || !is_safe_type_decl(type_decl) {
        return Err(crate::error::AlephError::invalid_input(format!(
            "Unsafe identifier for migration: table={table}, column={column}, type={type_decl}"
        )));
    }

    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_err)?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_err)?
        .filter_map(Result::ok)
        .any(|name| name == column);
    if !exists {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {type_decl}");
        conn.execute(&sql, []).map_err(db_err)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole schema has to apply to an empty database.
    ///
    /// `execute_batch` parses the entire literal, so ONE bad character in a
    /// comment near the end kills the FIRST statement and nothing is created.
    /// That is what a stray Rust-style `//` did (`near "/": syntax error`):
    /// every `CoordTaskStore::new` returned `Err`, boot took its supported
    /// warn-and-continue path, and the coordination-task subsystem went dark
    /// with nothing a user could see — while the compiler was perfectly happy,
    /// because the SQL is just a string to it.
    ///
    /// Cheap enough to keep forever: an in-memory connection and one call.
    #[test]
    fn the_schema_applies_to_an_empty_database() {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        migrate(&conn).expect("the coord schema must apply cleanly");
    }

    /// Re-running it must be a no-op, not a second failure: `migrate` runs on
    /// every open, and the ALTER-based column additions below the batch are
    /// guarded by probes rather than by `IF NOT EXISTS`.
    #[test]
    fn the_schema_is_idempotent() {
        let conn = Connection::open_in_memory().expect("in-memory sqlite");
        migrate(&conn).expect("first apply");
        migrate(&conn).expect("second apply must be a no-op");
    }
}
