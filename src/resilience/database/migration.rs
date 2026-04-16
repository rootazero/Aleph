/// Database migration logic for resilience storage.
///
/// This module provides idempotent migration functions for evolving the
/// resilience database schema over time. Migrations are safe to run multiple
/// times and prefer preserving existing data where feasible.
use crate::error::AlephError;
use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
use rusqlite::Connection;
use serde_json::Value;

/// Migrate to add experience_replays table for Cortex evolution system
///
/// This migration creates the experience_replays table for storing distilled
/// task execution experiences that can be replayed for faster execution.
///
/// # Migration Steps
/// 1. Check if experience_replays table exists
/// 2. If not, create the table with all required columns
/// 3. Create indexes for efficient querying
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Creates indexes with IF NOT EXISTS
pub fn migrate_add_experience_replays(conn: &Connection) -> Result<(), AlephError> {
    // Use savepoint for atomic migration
    conn.execute_batch("SAVEPOINT migration_experience_replays")
        .map_err(|e| AlephError::config(format!("Failed to begin migration: {}", e)))?;

    // Check if experience_replays table already exists
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='experience_replays'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_experience_replays");
            AlephError::config(format!("Failed to check experience_replays table: {}", e))
        })?;

    if table_exists == 0 {
        // Create experience_replays table
        conn.execute_batch(
            r#"
            CREATE TABLE experience_replays (
                -- Primary key and indexing
                id TEXT PRIMARY KEY,
                pattern_hash TEXT NOT NULL,
                intent_vector BLOB,

                -- Core context snapshot
                user_intent TEXT NOT NULL,
                environment_context_json TEXT,
                thought_trace_distilled TEXT,
                tool_sequence_json TEXT NOT NULL,
                parameter_mapping TEXT,
                logic_trace_json TEXT,

                -- Evaluation metrics
                success_score REAL NOT NULL,
                token_efficiency REAL,
                latency_ms INTEGER,
                novelty_score REAL,

                -- Evolution status and statistics
                evolution_status TEXT NOT NULL,
                usage_count INTEGER DEFAULT 1,
                success_count INTEGER DEFAULT 0,
                last_success_rate REAL,

                -- Timestamps
                created_at INTEGER NOT NULL,
                last_used_at INTEGER NOT NULL,
                last_evaluated_at INTEGER,

                -- Prevent duplicate experiences
                UNIQUE(pattern_hash, user_intent)
            );

            -- Index for pattern-based queries
            CREATE INDEX idx_experience_pattern_hash ON experience_replays(pattern_hash);

            -- Index for evolution status filtering
            CREATE INDEX idx_experience_evolution_status ON experience_replays(evolution_status);

            -- Index for LRU-based decay
            CREATE INDEX idx_experience_last_used_at ON experience_replays(last_used_at);

            -- Index for success rate queries
            CREATE INDEX idx_experience_success_rate ON experience_replays(last_success_rate);

            -- Virtual table for vector search on intent_vector
            CREATE VIRTUAL TABLE IF NOT EXISTS experiences_vec USING vec0(
                embedding float[1024]
            );
            "#,
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_experience_replays");
            AlephError::config(format!("Failed to create experience_replays table: {}", e))
        })?;

        tracing::info!("Created experience_replays table and indexes");
    } else {
        tracing::debug!("experience_replays table already exists, skipping creation");
    }

    // Release savepoint (commits all changes)
    conn.execute_batch("RELEASE migration_experience_replays")
        .map_err(|e| AlephError::config(format!("Failed to commit migration: {}", e)))?;

    Ok(())
}

/// Migrate task_traces from legacy flat role/content storage to structured
/// AgentTraceEvent storage.
///
/// The legacy schema stored a best-effort `role` plus arbitrary `content_json`.
/// The new schema stores a stable `event_kind` alongside the full serialized
/// `AgentTraceEvent`, making replay consume the same structured facts as live
/// panels/debug tools.
pub fn migrate_task_traces_to_agent_trace(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_task_traces_agent_trace")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to begin task_traces agent trace migration: {}",
                e
            ))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='task_traces'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
            AlephError::config(format!("Failed to check task_traces table: {}", e))
        })?;

    if table_exists == 0 {
        conn.execute_batch("RELEASE migration_task_traces_agent_trace")
            .map_err(|e| AlephError::config(format!("Failed to commit migration: {}", e)))?;
        return Ok(());
    }

    let has_event_json: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('task_traces') WHERE name='event_json'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
            AlephError::config(format!("Failed to inspect task_traces columns: {}", e))
        })?;
    let has_event_kind: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('task_traces') WHERE name='event_kind'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
            AlephError::config(format!("Failed to inspect task_traces columns: {}", e))
        })?;

    if has_event_json > 0 && has_event_kind > 0 {
        conn.execute_batch("RELEASE migration_task_traces_agent_trace")
            .map_err(|e| AlephError::config(format!("Failed to commit migration: {}", e)))?;
        return Ok(());
    }

    conn.execute_batch(
        r#"
        ALTER TABLE task_traces RENAME TO task_traces_legacy;
        CREATE TABLE task_traces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            step_index INTEGER NOT NULL,
            event_kind TEXT NOT NULL,
            event_json TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
        );
        "#,
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
        AlephError::config(format!("Failed to recreate task_traces table: {}", e))
    })?;

    {
        let mut select = conn
            .prepare(
                r#"
                SELECT id, task_id, step_index, role, content_json, timestamp
                FROM task_traces_legacy
                ORDER BY id ASC
                "#,
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
                AlephError::config(format!("Failed to prepare legacy trace query: {}", e))
            })?;

        let legacy_rows = select
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
                AlephError::config(format!("Failed to load legacy traces: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
                AlephError::config(format!("Failed to collect legacy traces: {}", e))
            })?;

        let mut insert = conn
            .prepare(
                r#"
                INSERT INTO task_traces (id, task_id, step_index, event_kind, event_json, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
                AlephError::config(format!("Failed to prepare migrated trace insert: {}", e))
            })?;

        for (id, task_id, step_index, role, content_json, timestamp) in legacy_rows {
            let event = legacy_trace_to_agent_trace(step_index, &role, &content_json);
            let event_json = serde_json::to_string(&event).map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
                AlephError::config(format!("Failed to serialize migrated trace: {}", e))
            })?;

            insert
                .execute(rusqlite::params![
                    id,
                    task_id,
                    step_index,
                    event.kind(),
                    event_json,
                    timestamp
                ])
                .map_err(|e| {
                    let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
                    AlephError::config(format!("Failed to insert migrated trace: {}", e))
                })?;
        }
    }

    conn.execute_batch(
        r#"
        DROP TABLE task_traces_legacy;
        CREATE INDEX IF NOT EXISTS idx_task_traces_task ON task_traces(task_id, step_index);
        "#,
    )
    .map_err(|e| {
        let _ = conn.execute_batch("ROLLBACK TO migration_task_traces_agent_trace");
        AlephError::config(format!("Failed to finalize task_traces migration: {}", e))
    })?;

    conn.execute_batch("RELEASE migration_task_traces_agent_trace")
        .map_err(|e| AlephError::config(format!("Failed to commit migration: {}", e)))?;

    Ok(())
}

/// Migrate to add channel_offsets table for persistent polling offset tracking.
///
/// Stores the last processed update_id per channel so that restarts resume
/// from where they left off instead of dropping or re-processing updates.
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Uses savepoint for atomic migration
pub fn migrate_add_channel_offsets(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_channel_offsets")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin channel_offsets migration: {}", e))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='channel_offsets'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_channel_offsets");
            AlephError::config(format!("Failed to check channel_offsets table: {}", e))
        })?;

    if table_exists == 0 {
        conn.execute_batch(
            r#"
            CREATE TABLE channel_offsets (
                channel_id TEXT PRIMARY KEY,
                bot_id TEXT NOT NULL,
                last_update_id INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_channel_offsets");
            AlephError::config(format!("Failed to create channel_offsets table: {}", e))
        })?;

        tracing::info!("Created channel_offsets table");
    } else {
        tracing::debug!("channel_offsets table already exists, skipping creation");
    }

    conn.execute_batch("RELEASE migration_channel_offsets")
        .map_err(|e| {
            AlephError::config(format!("Failed to commit channel_offsets migration: {}", e))
        })?;

    Ok(())
}

/// Migrate to add paired_users table for pairing persistence.
///
/// Stores which Telegram users are paired (allowed to interact) per channel,
/// enabling pairing state to survive restarts.
///
/// # Safety
/// - Uses IF NOT EXISTS for idempotent table creation
/// - Uses savepoint for atomic migration
pub fn migrate_add_paired_users(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch("SAVEPOINT migration_paired_users")
        .map_err(|e| {
            AlephError::config(format!("Failed to begin paired_users migration: {}", e))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='paired_users'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_paired_users");
            AlephError::config(format!("Failed to check paired_users table: {}", e))
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
            let _ = conn.execute_batch("ROLLBACK TO migration_paired_users");
            AlephError::config(format!("Failed to create paired_users table: {}", e))
        })?;

        tracing::info!("Created paired_users table");
    } else {
        tracing::debug!("paired_users table already exists, skipping creation");
    }

    conn.execute_batch("RELEASE migration_paired_users")
        .map_err(|e| {
            AlephError::config(format!("Failed to commit paired_users migration: {}", e))
        })?;

    Ok(())
}

/// Migrate to add sticker_descriptions table for Telegram sticker cache.
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
                "Failed to begin sticker_descriptions migration: {}",
                e
            ))
        })?;

    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sticker_descriptions'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| {
            let _ = conn.execute_batch("ROLLBACK TO migration_sticker_descriptions");
            AlephError::config(format!("Failed to check sticker_descriptions table: {}", e))
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
            let _ = conn.execute_batch("ROLLBACK TO migration_sticker_descriptions");
            AlephError::config(format!(
                "Failed to create sticker_descriptions table: {}",
                e
            ))
        })?;

        tracing::info!("Created sticker_descriptions table");
    } else {
        tracing::debug!("sticker_descriptions table already exists, skipping creation");
    }

    conn.execute_batch("RELEASE migration_sticker_descriptions")
        .map_err(|e| {
            AlephError::config(format!(
                "Failed to commit sticker_descriptions migration: {}",
                e
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
        Ok(Value::Object(map)) => ["content", "text", "output", "message", "result"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| Value::Object(map).to_string()),
        Ok(value) => value.to_string(),
        Err(_) => content_json.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};
    use rusqlite::Connection;

    #[test]
    #[allow(clippy::missing_transmute_annotations)]
    fn test_migrate_add_experience_replays_idempotent() {
        // Register sqlite-vec extension BEFORE opening connection
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        // Create in-memory database
        let conn = Connection::open_in_memory().unwrap();

        // First migration should create table
        migrate_add_experience_replays(&conn).unwrap();

        // Verify table exists
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='experience_replays'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);

        // Verify indexes exist
        let idx_pattern: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_experience_pattern_hash'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_pattern, 1);

        let idx_status: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_experience_evolution_status'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_status, 1);

        // Second migration should be no-op
        migrate_add_experience_replays(&conn).unwrap();

        // Verify still only one table
        let table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='experience_replays'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_exists, 1);
    }

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
