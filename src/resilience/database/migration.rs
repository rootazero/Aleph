/// Database migration logic for resilience storage.
///
/// This module provides idempotent migration functions for evolving the
/// resilience database schema over time. Migrations are safe to run multiple
/// times and prefer preserving existing data where feasible.
use crate::error::AlephError;
use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
use rusqlite::Connection;
use serde_json::Value;

/// Migrate `task_traces` from legacy flat role/content storage to structured
/// `AgentTraceEvent` storage.
///
/// The legacy schema stored a best-effort `role` plus arbitrary `content_json`.
/// The new schema stores a stable `event_kind` alongside the full serialized
/// `AgentTraceEvent`, making replay consume the same structured facts as live
/// panels/debug tools.
pub fn migrate_task_traces_to_agent_trace(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_task_traces_agent_trace")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to begin task_traces agent trace migration: {e}"
            ))
        })?;

    let result = migrate_task_traces_body(conn);

    match result {
        Ok(()) => conn
            .execute_batch("RELEASE migration_task_traces_agent_trace")
            .map_err(|e| AlephError::config(format!("Failed to commit migration: {e}"))),
        Err(e) => {
            if let Err(rollback_err) =
                conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace")
            {
                tracing::warn!(error = %rollback_err, "Rollback of migration_task_traces_agent_trace failed");
            }
            Err(e)
        }
    }
}

fn task_traces_table_exists(conn: &Connection) -> Result<bool, AlephError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_traces'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| AlephError::config(format!("Failed to check task_traces table: {e}")))?;
    Ok(count > 0)
}

fn task_traces_has_column(conn: &Connection, column: &str) -> Result<bool, AlephError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('task_traces') WHERE name = ?1",
            [column],
            |row| row.get(0),
        )
        .map_err(|e| AlephError::config(format!("Failed to inspect task_traces columns: {e}")))?;
    Ok(count > 0)
}

fn count_orphaned_legacy_traces(conn: &Connection) -> Result<i64, AlephError> {
    conn.query_row(
        "SELECT COUNT(*) FROM task_traces_legacy \
         WHERE task_id NOT IN (SELECT id FROM agent_tasks)",
        [],
        |row| row.get(0),
    )
    .map_err(|e| AlephError::config(format!("Failed to count orphaned legacy traces: {e}")))
}

struct LegacyTrace {
    id: i64,
    task_id: String,
    step_index: u32,
    role: String,
    content_json: String,
    timestamp: i64,
}

fn load_legacy_traces(conn: &Connection) -> Result<Vec<LegacyTrace>, AlephError> {
    let mut select = conn
        .prepare(
            r#"
            SELECT id, task_id, step_index, role, content_json, timestamp
            FROM task_traces_legacy
            WHERE task_id IN (SELECT id FROM agent_tasks)
            ORDER BY id ASC
            "#,
        )
        .map_err(|e| AlephError::config(format!("Failed to prepare legacy trace query: {e}")))?;

    let rows = select
        .query_map([], |row| {
            Ok(LegacyTrace {
                id: row.get(0)?,
                task_id: row.get(1)?,
                step_index: row.get(2)?,
                role: row.get(3)?,
                content_json: row.get(4)?,
                timestamp: row.get(5)?,
            })
        })
        .map_err(|e| AlephError::config(format!("Failed to load legacy traces: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AlephError::config(format!("Failed to collect legacy traces: {e}")))?;

    Ok(rows)
}

fn insert_migrated_traces(conn: &Connection, traces: &[LegacyTrace]) -> Result<(), AlephError> {
    let mut insert = conn
        .prepare(
            r#"
            INSERT INTO task_traces (id, task_id, step_index, event_kind, event_json, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
        )
        .map_err(|e| AlephError::config(format!("Failed to prepare migrated trace insert: {e}")))?;

    for trace in traces {
        let event = legacy_trace_to_agent_trace(trace.step_index, &trace.role, &trace.content_json);
        let event_json = serde_json::to_string(&event)
            .map_err(|e| AlephError::config(format!("Failed to serialize migrated trace: {e}")))?;

        insert
            .execute(rusqlite::params![
                trace.id,
                trace.task_id,
                trace.step_index,
                event.kind(),
                event_json,
                trace.timestamp
            ])
            .map_err(|e| AlephError::config(format!("Failed to insert migrated trace: {e}")))?;
    }

    Ok(())
}

fn migrate_task_traces_body(conn: &Connection) -> Result<(), AlephError> {
    if !task_traces_table_exists(conn)? {
        return Ok(());
    }

    if task_traces_has_column(conn, "event_json")? && task_traces_has_column(conn, "event_kind")? {
        return Ok(());
    }

    conn.execute_batch(
        r#"
        ALTER TABLE task_traces RENAME TO task_traces_legacy;
        CREATE TABLE task_traces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            step_index INTEGER NOT NULL CHECK (step_index >= 0),
            event_kind TEXT NOT NULL,
            event_json TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            FOREIGN KEY(task_id) REFERENCES agent_tasks(id) ON DELETE RESTRICT,
            UNIQUE(task_id, step_index)
        );
        "#,
    )
    .map_err(|e| AlephError::config(format!("Failed to recreate task_traces table: {e}")))?;

    // Legacy DBs predate strict FK enforcement; a trace whose parent
    // `agent_tasks` row was later removed would violate the new table's
    // FOREIGN KEY on insert and abort the whole migration (and thus startup,
    // since `new()` propagates the error). Such orphans are useless for replay
    // anyway. Count them for visibility, then skip them via the SELECT filter
    // below so the migration is robust on real-world databases.
    let orphan_count = count_orphaned_legacy_traces(conn)?;
    if orphan_count > 0 {
        tracing::warn!(
            orphan_count,
            "Skipping orphaned task_traces rows with no surviving agent_tasks parent during migration"
        );
    }

    let legacy_rows = load_legacy_traces(conn)?;
    insert_migrated_traces(conn, &legacy_rows)?;

    conn.execute_batch(
        r#"
        DROP TABLE task_traces_legacy;
        CREATE INDEX IF NOT EXISTS idx_task_traces_task ON task_traces(task_id, step_index);
        "#,
    )
    .map_err(|e| AlephError::config(format!("Failed to finalize task_traces migration: {e}")))?;

    Ok(())
}

/// Migrate to add `UNIQUE(task_id, step_index)` and `CHECK (step_index >= 0)`
/// to `task_traces`.
///
/// The new-column DDL in `schema.rs` already includes both constraints for
/// fresh databases. This migration back-fills them on databases that were
/// created before the constraints existed. A DB that already has the
/// constraint (idempotent re-run) is detected via the `sqlite_master`
/// index list and skipped.
///
/// `task_traces` may carry duplicate `(task_id, step_index)` rows from
/// earlier code that did not enforce the invariant — Shadow Replay would
/// re-apply the same event in such cases. We deduplicate by keeping the
/// lowest `id` (the earliest inserted row) before adding the constraint,
/// so the migration is safe to run on real-world databases.
///
/// # Safety
/// - Uses savepoint for atomic migration
/// - Idempotent: skips if the constraint already exists
pub fn migrate_task_traces_unique_step_index(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_task_traces_unique_step")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to begin task_traces unique-step migration: {e}"
            ))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_traces'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_unique_step");
            AlephError::config(format!("Failed to check task_traces table: {e}"))
        })?;

    if table_exists == 0 {
        // Schema bootstrap path; nothing to migrate.
        conn.execute_batch("RELEASE migration_task_traces_unique_step")
            .map_err(|e| {
                AlephError::config(format!(
                    "Failed to commit task_traces unique-step migration: {e}"
                ))
            })?;
        return Ok(());
    }

    let constraint_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND tbl_name='task_traces' \
               AND sql LIKE '%UNIQUE%task_id%step_index%'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_unique_step");
            AlephError::config(format!("Failed to check UNIQUE constraint: {e}"))
        })?;

    if constraint_exists > 0 {
        conn.execute_batch("RELEASE migration_task_traces_unique_step")
            .map_err(|e| {
                AlephError::config(format!(
                    "Failed to commit task_traces unique-step migration: {e}"
                ))
            })?;
        return Ok(());
    }

    // Deduplicate any pre-existing duplicate (task_id, step_index) pairs by
    // keeping the smallest `id` per pair. SQLite ships no row_number(); the
    // MIN(id) per group + self-join gives us a delete set.
    let deleted = conn.execute(
        r#"
        DELETE FROM task_traces
         WHERE id NOT IN (
             SELECT MIN(id) FROM task_traces GROUP BY task_id, step_index
         )
        "#,
        [],
    )?;
    if deleted > 0 {
        tracing::warn!(
            deleted,
            "Removed duplicate (task_id, step_index) rows from task_traces before adding UNIQUE constraint"
        );
    }

    // SQLite cannot ALTER TABLE … ADD CONSTRAINT; rebuild the table with the
    // UNIQUE / CHECK constraints in place, then re-apply the index.
    conn.execute_batch(
        r#"
        CREATE TABLE task_traces_new (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            step_index INTEGER NOT NULL CHECK (step_index >= 0),
            event_kind TEXT NOT NULL,
            event_json TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            FOREIGN KEY(task_id) REFERENCES agent_tasks(id) ON DELETE RESTRICT,
            UNIQUE(task_id, step_index)
        );
        INSERT INTO task_traces_new
            (id, task_id, step_index, event_kind, event_json, timestamp)
        SELECT id, task_id, step_index, event_kind, event_json, timestamp
          FROM task_traces;
        DROP TABLE task_traces;
        ALTER TABLE task_traces_new RENAME TO task_traces;
        CREATE INDEX IF NOT EXISTS idx_task_traces_task ON task_traces(task_id, step_index);
        "#,
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_unique_step");
        AlephError::config(format!(
            "Failed to add UNIQUE(task_id, step_index) to task_traces: {e}"
        ))
    })?;

    tracing::info!("Added UNIQUE(task_id, step_index) and CHECK(step_index >= 0) to task_traces");

    conn.execute_batch("RELEASE migration_task_traces_unique_step")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to commit task_traces unique-step migration: {e}"
            ))
        })?;

    Ok(())
}

/// Migrate to add `channel_offsets` table for persistent polling offset tracking.
///
/// Stores the last processed `update_id` per channel so that restarts resume
/// from where they left off instead of dropping or re-processing updates.
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Uses savepoint for atomic migration
pub fn migrate_add_channel_offsets(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_channel_offsets")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin channel_offsets migration: {e}"))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='channel_offsets'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_channel_offsets") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_channel_offsets failed");
            }
            AlephError::config(format!("Failed to check channel_offsets table: {e}"))
        })?;

    if table_exists == 0 {
        conn.execute_batch(
            r#"
            CREATE TABLE channel_offsets (
                channel_id TEXT PRIMARY KEY,
                bot_id TEXT NOT NULL,
                last_update_id INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL
            )
            "#,
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_channel_offsets") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_channel_offsets failed");
            }
            AlephError::config(format!("Failed to create channel_offsets table: {e}"))
        })?;

        tracing::info!("Created channel_offsets table");
    } else {
        tracing::debug!("channel_offsets table already exists, skipping creation");
    }

    conn.execute_batch("RELEASE migration_channel_offsets")
        .map_err(|e| {
            AlephError::config(format!("Failed to commit channel_offsets migration: {e}"))
        })?;

    Ok(())
}

/// Migrate to add `paired_users` table for pairing persistence.
///
/// Stores which Telegram users are paired (allowed to interact) per channel,
/// enabling pairing state to survive restarts.
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Uses savepoint for atomic migration
pub fn migrate_add_paired_users(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_paired_users")
        .map_err(|e| AlephError::config(format!("Failed to begin paired_users migration: {e}")))?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='paired_users'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_paired_users") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_paired_users failed");
            }
            AlephError::config(format!("Failed to check paired_users table: {e}"))
        })?;

    if table_exists == 0 {
        conn.execute_batch(
            r#"
            CREATE TABLE paired_users (
                channel_id TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                paired_at TEXT NOT NULL,
                PRIMARY KEY(channel_id, user_id)
            )
            "#,
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_paired_users") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_paired_users failed");
            }
            AlephError::config(format!("Failed to create paired_users table: {e}"))
        })?;

        tracing::info!("Created paired_users table");
    } else {
        tracing::debug!("paired_users table already exists, skipping creation");
    }

    conn.execute_batch("RELEASE migration_paired_users")
        .map_err(|e| AlephError::config(format!("Failed to commit paired_users migration: {e}")))?;

    Ok(())
}

/// Migrate to add `sticker_descriptions` table for Telegram sticker cache.
///
/// Stores LLM-generated descriptions of stickers so they can be reused
/// without re-running vision inference.
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Uses savepoint for atomic migration
pub fn migrate_add_sticker_descriptions(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_sticker_descriptions")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to begin sticker_descriptions migration: {e}"
            ))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sticker_descriptions'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_sticker_descriptions") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_sticker_descriptions failed");
            }
            AlephError::config(format!("Failed to check sticker_descriptions table: {e}"))
        })?;

    if table_exists == 0 {
        conn.execute_batch(
            r#"
            CREATE TABLE sticker_descriptions (
                file_unique_id TEXT PRIMARY KEY,
                description TEXT NOT NULL,
                cached_at TEXT NOT NULL
            )
            "#,
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_sticker_descriptions") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_sticker_descriptions failed");
            }
            AlephError::config(format!(
                "Failed to create sticker_descriptions table: {e}"
            ))
        })?;

        tracing::info!("Created sticker_descriptions table");
    } else {
        tracing::debug!("sticker_descriptions table already exists, skipping creation");
    }

    conn.execute_batch("RELEASE migration_sticker_descriptions")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to commit sticker_descriptions migration: {e}"
            ))
        })?;

    Ok(())
}

fn legacy_trace_to_agent_trace(step_index: u32, role: &str, content_json: &str) -> AgentTraceEvent {
    let iteration = step_index as usize;
    let text = extract_legacy_trace_text(content_json);

    match role {
        "tool" => AgentTraceEvent::ToolSummary {
            iteration,
            summary: text,
        },
        _ => AgentTraceEvent::TextEmitted {
            iteration,
            stream: AgentTraceTextKind::Final,
            text,
        },
    }
}

fn extract_legacy_trace_text(content_json: &str) -> String {
    match serde_json::from_str::<Value>(content_json) {
        Ok(Value::String(text)) => text,
        Ok(Value::Object(map)) => {
            let text = ["content", "text", "output", "message", "result"]
                .iter()
                .find_map(|key| map.get(*key).and_then(Value::as_str))
                .map(std::borrow::ToOwned::to_owned);
            text.unwrap_or_else(|| Value::Object(map).to_string())
        }
        Ok(value) => value.to_string(),
        Err(_) => content_json.to_string(),
    }
}

/// Migrate to add `owner_user_id` column to `group_chat_sessions`.
///
/// Group chat sessions were originally created without persisting the P1
/// ownership stamp that `GroupChatSession::new` reads from
/// `crate::scope::current_scope()`. The stamp was held in memory only and
/// silently lost on daemon restart, breaking
/// `stamped_owner_visible`-style visibility queries that fall through to the
/// operator-default branch when `owner_user_id IS NULL`.
///
/// This migration adds the column if absent. Existing rows are backfilled with
/// `NULL`, which keeps the operator-default visibility behavior for legacy
/// sessions — the same behavior they had before, so this is non-breaking.
///
/// # Safety
/// - Uses savepoint for atomic migration
/// - Idempotent: skips if column already exists
pub fn migrate_add_group_chat_owner(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_group_chat_owner")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin group_chat_owner migration: {e}"))
        })?;

    let column_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('group_chat_sessions') WHERE name='owner_user_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_group_chat_owner") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_group_chat_owner failed");
            }
            AlephError::config(format!(
                "Failed to check group_chat_sessions.owner_user_id column: {e}"
            ))
        })?;

    if column_exists == 0 {
        conn.execute_batch(
            "ALTER TABLE group_chat_sessions ADD COLUMN owner_user_id TEXT",
        )
        .map_err(|e| {
            if let Err(rollback_err) = conn.execute_batch("ROLLBACK TO migration_group_chat_owner") {
                tracing::warn!(error = %rollback_err, "Rollback of migration_group_chat_owner failed");
            }
            AlephError::config(format!(
                "Failed to add owner_user_id column to group_chat_sessions: {e}"
            ))
        })?;

        tracing::info!("Added owner_user_id column to group_chat_sessions");
    } else {
        tracing::debug!("group_chat_sessions.owner_user_id already exists, skipping");
    }

    conn.execute_batch("RELEASE migration_group_chat_owner")
        .map_err(|e| {
            AlephError::config(format!("Failed to commit group_chat_owner migration: {e}"))
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
    use rusqlite::Connection;

    #[test]
    fn test_migrate_task_traces_to_agent_trace() {
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            r#"
            CREATE TABLE agent_tasks (
                id TEXT PRIMARY KEY
            );
            CREATE TABLE task_traces (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                role TEXT NOT NULL,
                content_json TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            );
            INSERT INTO agent_tasks (id) VALUES ('task-1');
            INSERT INTO task_traces (task_id, step_index, role, content_json, timestamp)
            VALUES
                ('task-1', 0, 'assistant', '{"content":"hello"}', 123),
                ('task-1', 1, 'tool', '{"output":"search complete"}', 124);
            "#,
        )
        .unwrap();

        migrate_task_traces_to_agent_trace(&conn).unwrap();

        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info('task_traces') ORDER BY cid ASC")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            columns,
            vec![
                "id",
                "task_id",
                "step_index",
                "event_kind",
                "event_json",
                "timestamp"
            ]
        );

        let rows: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT event_kind, event_json FROM task_traces ORDER BY id ASC")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        assert_eq!(rows[0].0, "text_emitted");
        assert_eq!(
            serde_json::from_str::<AgentTraceEvent>(&rows[0].1).unwrap(),
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "hello".to_string(),
            }
        );

        assert_eq!(rows[1].0, "tool_summary");
        assert_eq!(
            serde_json::from_str::<AgentTraceEvent>(&rows[1].1).unwrap(),
            AgentTraceEvent::ToolSummary {
                iteration: 1,
                summary: "search complete".to_string(),
            }
        );

        migrate_task_traces_to_agent_trace(&conn).unwrap();
    }
}
