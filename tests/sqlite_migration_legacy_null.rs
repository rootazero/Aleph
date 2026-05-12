//! Regression test for the legacy SQLite → file-backend migration when
//! pre-existing rows have NULL values in columns the migration SELECT
//! treats as `i64` (compaction_count, input_tokens, output_tokens).
//!
//! Reported by user 2026-05-12 after upgrading from a pre-`DEFAULT 0`
//! schema where `ALTER TABLE ADD COLUMN compaction_count INTEGER` left
//! existing rows with NULL. Without the `Option<i64>::unwrap_or(0)`
//! coercion in `migration.rs`, the first such row aborts the entire
//! migration with `InvalidColumnType`.

use std::path::Path;

use rusqlite::params;
use tempfile::TempDir;

use alephcore::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
use alephcore::gateway::session_store::migration::export_legacy_messages_from;
use alephcore::gateway::session_store::types::SessionMetadata;

/// Build a legacy `sessions.db` whose schema is the *broken* shape that
/// shipped before commit 5d4fe7bcd — the columns exist but contain NULL
/// for historical rows.
fn write_legacy_db(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("open legacy db");

    // Legacy `sessions` schema as it appeared after older ALTER TABLE
    // migrations: `compaction_count` and token columns added WITHOUT
    // `DEFAULT 0`, so existing rows hold NULL.
    conn.execute_batch(
        r#"
        CREATE TABLE sessions (
            key TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            session_type TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            last_active_at INTEGER NOT NULL,
            message_count INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            auto_reset_at INTEGER,
            state TEXT,
            metadata TEXT,
            label TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            model TEXT,
            model_provider TEXT,
            parent_session_key TEXT,
            compaction_count INTEGER
        );
        CREATE TABLE messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            metadata TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER
        );
        "#,
    )
    .expect("create legacy schema");

    // Session row with explicit NULLs in every column the bug touches.
    conn.execute(
        r#"INSERT INTO sessions
            (key, agent_id, session_type, created_at, last_active_at,
             message_count, total_tokens, auto_reset_at, state, metadata,
             label, input_tokens, output_tokens, model, model_provider,
             parent_session_key, compaction_count)
           VALUES
            ('legacy-session-1', 'agent-a', 'chat', 1_700_000_000, 1_700_000_100,
             2, 0, NULL, NULL, NULL,
             NULL, NULL, NULL, NULL, NULL,
             NULL, NULL)"#,
        [],
    )
    .expect("insert legacy session");

    // Two messages, both with NULL token columns.
    for (role, content, ts) in [
        ("user", "hello from the past", 1_700_000_050i64),
        ("assistant", "hi, legacy world", 1_700_000_060i64),
    ] {
        conn.execute(
            "INSERT INTO messages (session_key, role, content, timestamp, metadata, input_tokens, output_tokens)
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL)",
            params!["legacy-session-1", role, content, ts],
        )
        .expect("insert legacy message");
    }
}

#[tokio::test]
async fn migrates_legacy_db_with_null_token_and_compaction_columns() {
    let tmp = TempDir::new().expect("tempdir");
    let legacy_db = tmp.path().join("sessions.db");
    let store_dir = tmp.path().join("file_store");
    write_legacy_db(&legacy_db);

    let store = FileSessionStore::new(FileSessionStoreConfig {
        base_dir: store_dir.clone(),
        ..Default::default()
    })
    .expect("file store");

    let migrated = export_legacy_messages_from(&legacy_db, &store)
        .await
        .expect("migration must succeed despite NULL legacy columns");
    assert_eq!(migrated, 2, "both legacy messages should be migrated");

    // The migration marker proves we reached the end of the pipeline,
    // not just the first SELECT.
    let marker = store_dir.join(".migrated_from_sqlite");
    assert!(marker.exists(), "marker file should be written on success");

    // NULL columns must surface as 0, never propagate as serialization errors.
    let metadata_path = store_dir
        .join("legacy-session-1")
        .join("metadata.json");
    let raw = std::fs::read_to_string(&metadata_path).expect("metadata.json");
    let meta: SessionMetadata = serde_json::from_str(&raw).expect("parse metadata.json");
    assert_eq!(meta.key, "legacy-session-1");
    assert_eq!(
        meta.compaction_count, 0,
        "NULL compaction_count must coerce to 0"
    );
    assert_eq!(meta.input_tokens, 0, "NULL input_tokens must coerce to 0");
    assert_eq!(
        meta.output_tokens, 0,
        "NULL output_tokens must coerce to 0"
    );

    // Transcript should hold both messages, both with 0 tokens.
    let transcript_path = store_dir
        .join("legacy-session-1")
        .join("transcript.jsonl");
    let transcript = std::fs::read_to_string(&transcript_path).expect("transcript.jsonl");
    let lines: Vec<&str> = transcript.lines().collect();
    assert_eq!(lines.len(), 2, "both legacy messages should be appended");
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("parse transcript line");
        assert_eq!(v["input_tokens"], 0, "NULL message input_tokens → 0");
        assert_eq!(v["output_tokens"], 0, "NULL message output_tokens → 0");
    }
}

#[tokio::test]
async fn migration_is_a_noop_when_legacy_db_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let store = FileSessionStore::new(FileSessionStoreConfig {
        base_dir: tmp.path().join("file_store"),
        ..Default::default()
    })
    .expect("file store");

    let count = export_legacy_messages_from(&tmp.path().join("does_not_exist.db"), &store)
        .await
        .expect("missing legacy db should not be an error");
    assert_eq!(count, 0);
}
