use std::path::Path;

use rusqlite::Connection;
use tracing::info;

use crate::gateway::session_store::file_backend::FileSessionStore;
use crate::gateway::session_store::types::{MessageRecord, SessionMetadata};

const MIGRATION_MARKER: &str = ".migrated_from_sqlite";

/// Check whether a legacy SQLite sessions database exists and has not yet been migrated.
pub fn migration_needed(base_dir: &Path) -> bool {
    let marker = base_dir.join(MIGRATION_MARKER);
    if marker.exists() {
        return false;
    }
    let legacy_db = crate::utils::paths::get_sessions_db_path()
        .unwrap_or_else(|_| std::env::temp_dir().join("aleph_data").join("sessions.db"));
    legacy_db.exists()
}

/// Export legacy SQLite sessions and messages into the file backend.
/// Writes a marker file on success so the migration is not re-run.
pub async fn export_legacy_messages(
    store: &FileSessionStore,
) -> Result<usize, crate::error::AlephError> {
    let legacy_db = crate::utils::paths::get_sessions_db_path().map_err(|e| {
        crate::error::AlephError::ConfigError {
            message: format!("Failed to resolve legacy DB path: {}", e),
            suggestion: None,
        }
    })?;

    if !legacy_db.exists() {
        info!(
            "No legacy SQLite session database found at {:?}; skipping migration.",
            legacy_db
        );
        return Ok(0);
    }

    info!("Starting session migration from {:?} ...", legacy_db);

    let conn = Connection::open(&legacy_db).map_err(|e| crate::error::AlephError::ConfigError {
        message: format!("Failed to open legacy session DB: {}", e),
        suggestion: None,
    })?;

    // Defensive: older legacy DBs may be missing columns the SELECT below
    // expects. PRAGMA-guard each ADD COLUMN so the migration succeeds against
    // any historical schema. SQLite's ALTER TABLE ADD COLUMN is fast, has no
    // table-rewrite cost, and is idempotent here because we check first.
    ensure_sessions_columns(&conn)?;

    // -----------------------------------------------------------------------
    // 1. Migrate sessions -> metadata.json
    // -----------------------------------------------------------------------
    let mut stmt = conn
        .prepare(
            "SELECT key, agent_id, session_type, created_at, last_active_at, message_count, total_tokens, auto_reset_at, state, metadata, label, input_tokens, output_tokens, model, model_provider, parent_session_key, compaction_count FROM sessions",
        )
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Prepare failed: {}", e),
            suggestion: None,
        })?;

    let session_rows = stmt
        .query_map([], |row| {
            let state_str: Option<String> = row.get(8)?;
            let state = state_str
                .and_then(|s| serde_json::from_str(&format!("\"{}\"", s)).ok())
                .unwrap_or_default();
            let metadata_json: Option<String> = row.get(9)?;
            let (topic, status, identity_meta) =
                SessionMetadata::parse_legacy_metadata_json(metadata_json.as_deref());
            Ok(SessionMetadata {
                key: row.get(0)?,
                agent_id: row.get(1)?,
                session_type: row.get(2)?,
                created_at: row.get(3)?,
                last_active_at: row.get(4)?,
                message_count: row.get(5)?,
                total_tokens: row.get(6)?,
                auto_reset_at: row.get(7)?,
                state: Some(state),
                topic,
                status,
                identity_meta,
                label: row.get(10)?,
                input_tokens: row.get(11)?,
                output_tokens: row.get(12)?,
                model: row.get(13)?,
                model_provider: row.get(14)?,
                parent_session_key: row.get(15)?,
                compaction_count: row.get(16)?,
                ..Default::default()
            })
        })
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Query failed: {}", e),
            suggestion: None,
        })?;

    let mut migrated_count = 0usize;
    for meta in session_rows {
        let meta = meta.map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Row error: {}", e),
            suggestion: None,
        })?;
        let key_str = &meta.key;
        store.write_metadata(key_str, &meta).await.map_err(|e| {
            crate::error::AlephError::ConfigError {
                message: format!("Write metadata failed for {}: {}", key_str, e),
                suggestion: None,
            }
        })?;
        migrated_count += 1;
    }
    drop(stmt);

    // -----------------------------------------------------------------------
    // 2. Migrate messages -> transcript.jsonl
    // -----------------------------------------------------------------------
    let mut msg_stmt = conn
        .prepare(
            "SELECT session_key, role, content, timestamp, metadata, input_tokens, output_tokens FROM messages ORDER BY id ASC",
        )
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Prepare messages failed: {}", e),
            suggestion: None,
        })?;

    let mut current_key: Option<String> = None;
    let mut current_batch: Vec<MessageRecord> = Vec::new();
    let mut total_messages = 0usize;

    let message_rows = msg_stmt
        .query_map([], |row| {
            let metadata_str: Option<String> = row.get(4)?;
            let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
            Ok((
                row.get::<usize, String>(0)?,
                MessageRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    metadata,
                    input_tokens: row.get(5)?,
                    output_tokens: row.get(6)?,
                    model: None,
                    model_provider: None,
                },
            ))
        })
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Query messages failed: {}", e),
            suggestion: None,
        })?;

    for row in message_rows {
        let (key, msg) = row.map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Message row error: {}", e),
            suggestion: None,
        })?;

        if current_key.as_ref() != Some(&key) {
            if let Some(ref k) = current_key {
                if !current_batch.is_empty() {
                    write_batch(store, k, &current_batch).await?;
                    total_messages += current_batch.len();
                }
            }
            current_key = Some(key);
            current_batch.clear();
        }
        current_batch.push(msg);
    }

    if let Some(ref k) = current_key {
        if !current_batch.is_empty() {
            write_batch(store, k, &current_batch).await?;
            total_messages += current_batch.len();
        }
    }
    drop(msg_stmt);

    // -----------------------------------------------------------------------
    // 3. Post-migration: re-derive titles and previews from transcripts
    // -----------------------------------------------------------------------
    let keys: Vec<String> = conn
        .prepare("SELECT key FROM sessions")
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Prepare post-migration failed: {}", e),
            suggestion: None,
        })?
        .query_map([], |row| row.get::<usize, String>(0))
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Query post-migration failed: {}", e),
            suggestion: None,
        })?
        .filter_map(|r| r.ok())
        .collect();

    for key in keys {
        if let Some(mut meta) = store.read_metadata(&key).await.ok().flatten() {
            if let Ok(messages) = store.read_transcript(&key, None).await {
                for msg in &messages {
                    if meta.derived_title.is_none() && msg.role == "user" {
                        let title = msg.content.trim();
                        let title = if title.chars().count() > 60 {
                            title.chars().take(60).collect::<String>() + "..."
                        } else {
                            title.to_string()
                        };
                        if !title.is_empty() {
                            meta.derived_title = Some(title);
                        }
                    }
                }
                if let Some(last) = messages.last() {
                    let preview = last.content.trim();
                    meta.last_message_preview = Some(if preview.chars().count() > 120 {
                        preview.chars().take(120).collect::<String>() + "..."
                    } else {
                        preview.to_string()
                    });
                }
                meta.message_count = messages.len() as i64;
                let _ = store.write_metadata(&key, &meta).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Write marker
    // -----------------------------------------------------------------------
    let marker = store.config().base_dir.join(MIGRATION_MARKER);
    let marker_content = format!(
        "migrated_at={}\nsessions={}\nmessages={}",
        chrono::Utc::now().to_rfc3339(),
        migrated_count,
        total_messages
    );
    tokio::fs::write(&marker, marker_content)
        .await
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("Failed to write migration marker: {}", e),
            suggestion: None,
        })?;

    info!(
        sessions = migrated_count,
        messages = total_messages,
        "Session migration from SQLite to file backend completed successfully"
    );

    Ok(total_messages)
}

/// Add columns the migration SELECT depends on, if missing. PRAGMA-guarded
/// per column so this is safe to run on any historical legacy schema.
fn ensure_sessions_columns(conn: &Connection) -> Result<(), crate::error::AlephError> {
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if table_exists == 0 {
        return Ok(());
    }

    let existing: std::collections::HashSet<String> = conn
        .prepare("PRAGMA table_info(sessions)")
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("PRAGMA table_info(sessions) prepare failed: {}", e),
            suggestion: None,
        })?
        .query_map([], |row| row.get::<usize, String>(1))
        .map_err(|e| crate::error::AlephError::ConfigError {
            message: format!("PRAGMA table_info(sessions) query failed: {}", e),
            suggestion: None,
        })?
        .filter_map(|r| r.ok())
        .collect();

    let needed: &[(&str, &str)] = &[
        ("auto_reset_at", "INTEGER"),
        ("state", "TEXT"),
        ("metadata", "TEXT"),
        ("label", "TEXT"),
        ("input_tokens", "INTEGER"),
        ("output_tokens", "INTEGER"),
        ("model", "TEXT"),
        ("model_provider", "TEXT"),
        ("parent_session_key", "TEXT"),
        ("compaction_count", "INTEGER"),
    ];
    for (col, ty) in needed {
        if !existing.contains(*col) {
            let sql = format!("ALTER TABLE sessions ADD COLUMN {} {}", col, ty);
            conn.execute(&sql, [])
                .map_err(|e| crate::error::AlephError::ConfigError {
                    message: format!("ALTER TABLE sessions ADD COLUMN {} failed: {}", col, e),
                    suggestion: None,
                })?;
        }
    }
    Ok(())
}

async fn write_batch(
    store: &FileSessionStore,
    key: &str,
    messages: &[MessageRecord],
) -> Result<(), crate::error::AlephError> {
    for msg in messages {
        store.append_transcript(key, msg).await.map_err(|e| {
            crate::error::AlephError::ConfigError {
                message: format!("Append transcript failed for {}: {}", key, e),
                suggestion: None,
            }
        })?;
    }
    Ok(())
}
