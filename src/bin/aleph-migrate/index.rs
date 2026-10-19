//! Index module: Build metadata.db from Markdown sessions

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub fn run_index(sessions_dir: PathBuf, output_db: PathBuf, rebuild_fts: bool) -> Result<()> {
    log::info!("Starting indexing: Markdown → metadata.db");
    log::info!("  Sessions directory: {}", sessions_dir.display());
    log::info!("  Output database: {}", output_db.display());

    // Scan session directories
    log::info!("[1/4] Scanning session directories...");
    let session_paths = scan_sessions(&sessions_dir)?;
    log::info!("  Found {} sessions", session_paths.len());

    // Create database
    log::info!("[2/4] Creating database schema...");
    let conn = Connection::open(&output_db).context("Failed to create database")?;
    init_schema(&conn)?;
    log::info!("  OK");

    // Index sessions
    log::info!("[3/4] Indexing sessions...");
    index_sessions(&conn, &session_paths)?;
    log::info!("  OK");

    // Build FTS5 index
    log::info!("[4/4] Building FTS5 index...");
    build_fts5_index(&conn, &sessions_dir, rebuild_fts)?;
    log::info!("  OK");

    // Print summary
    let stats = collect_stats(&conn)?;
    log::info!("");
    log::info!("✓ Indexed {} sessions", stats.session_count);
    log::info!("✓ Registered {} agents", stats.agent_count);
    log::info!("✓ Registered {} workspaces", stats.workspace_count);
    log::info!("✓ Indexed {} attachments", stats.attachment_count);
    log::info!("✓ FTS5 index: {} tokens", stats.fts_token_count);

    Ok(())
}

fn scan_sessions(sessions_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut session_paths = Vec::new();

    for entry in WalkDir::new(sessions_dir)
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_entry(|e| e.file_type().is_dir())
    {
        let entry = entry?;
        let path = entry.path();

        // Check if metadata.json exists
        if path.join("metadata.json").exists() {
            session_paths.push(path.to_path_buf());
        }
    }

    session_paths.sort();
    Ok(session_paths)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agents (
            agent_id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS workspaces (
            workspace_id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            workspace_id TEXT,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            duration_sec INTEGER,
            message_count INTEGER DEFAULT 0,
            tool_call_count INTEGER DEFAULT 0,
            attachment_count INTEGER DEFAULT 0,
            compression_status TEXT DEFAULT 'pending',
            content_hash TEXT,
            tags TEXT,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (agent_id) REFERENCES agents(agent_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id)
        );

        CREATE TABLE IF NOT EXISTS attachments (
            content_hash TEXT PRIMARY KEY,
            mime_type TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            file_path TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_agent ON sessions(agent_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_workspace ON sessions(workspace_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions(started_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(compression_status);

        CREATE VIRTUAL TABLE IF NOT EXISTS session_transcripts_fts USING fts5(
            session_id UNINDEXED,
            content,
            tokenize = 'porter'
        );
        "#,
    )
    .context("Failed to create schema")?;

    Ok(())
}

fn index_sessions(conn: &Connection, session_paths: &[PathBuf]) -> Result<()> {
    let total = session_paths.len();

    for (idx, session_path) in session_paths.iter().enumerate() {
        if idx % 10 == 0 {
            log::info!("  Progress: {}/{} ({}%)", idx, total, (idx * 100) / total);
        }

        index_single_session(conn, session_path)
            .with_context(|| format!("Failed to index session at {}", session_path.display()))?;
    }

    log::info!("  Progress: {}/{} (100%)", total, total);
    Ok(())
}

fn index_single_session(conn: &Connection, session_path: &Path) -> Result<()> {
    use serde_json::Value;

    // Read metadata.json
    let metadata_path = session_path.join("metadata.json");
    let metadata_content =
        fs::read_to_string(&metadata_path).context("Failed to read metadata.json")?;
    let metadata: Value =
        serde_json::from_str(&metadata_content).context("Failed to parse metadata.json")?;

    let session_id = metadata["session_id"]
        .as_str()
        .context("Missing session_id")?;
    let agent_id = metadata["agent_id"].as_str().context("Missing agent_id")?;
    let workspace_id = metadata["workspace_id"].as_str();
    let started_at = metadata["started_at"]
        .as_i64()
        .context("Missing started_at")?;
    let ended_at = metadata["ended_at"].as_i64();
    let tags = metadata["tags"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(",")
    });
    let compression_status = metadata["compression_status"].as_str().unwrap_or("pending");
    let message_count = metadata["message_count"].as_i64().unwrap_or(0);
    let tool_call_count = metadata["tool_call_count"].as_i64().unwrap_or(0);
    let attachment_count = metadata["attachment_count"].as_i64().unwrap_or(0);

    let duration_sec = ended_at.map(|end| end - started_at);
    let now = chrono::Utc::now().timestamp();

    // Insert or ignore agent
    conn.execute(
        "INSERT OR IGNORE INTO agents (agent_id, created_at) VALUES (?1, ?2)",
        rusqlite::params![agent_id, now],
    )?;

    // Insert or ignore workspace
    if let Some(ws_id) = workspace_id {
        conn.execute(
            "INSERT OR IGNORE INTO workspaces (workspace_id, created_at) VALUES (?1, ?2)",
            rusqlite::params![ws_id, now],
        )?;
    }

    conn.execute(
        "INSERT OR REPLACE INTO sessions (
            session_id, agent_id, workspace_id, started_at, ended_at, duration_sec,
            message_count, tool_call_count, attachment_count, compression_status,
            tags, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            session_id,
            agent_id,
            workspace_id,
            started_at,
            ended_at,
            duration_sec,
            message_count,
            tool_call_count,
            attachment_count,
            compression_status,
            tags,
            now,
        ],
    )?;

    index_attachments(conn, session_path, session_id, started_at, attachment_count)?;

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct IndexedAttachment {
    #[allow(dead_code)]
    attachment_id: String,
    content_hash: String,
    mime_type: String,
    size_bytes: i64,
    file_path: String,
}

fn index_attachments(
    conn: &Connection,
    session_path: &Path,
    session_id: &str,
    created_at: i64,
    attachment_count: i64,
) -> Result<()> {
    let manifest_path = session_path.join("attachments.json");
    if attachment_count == 0 {
        return Ok(());
    }
    if !manifest_path.exists() {
        anyhow::bail!(
            "Session {} declares {} attachments but attachments.json is missing",
            session_id,
            attachment_count
        );
    }

    let manifest: Vec<IndexedAttachment> = serde_json::from_str(
        &fs::read_to_string(&manifest_path).context("Failed to read attachments.json")?,
    )
    .context("Failed to parse attachments.json")?;
    if manifest.len() as i64 != attachment_count {
        anyhow::bail!(
            "Session {} declares {} attachments but manifest contains {}",
            session_id,
            attachment_count,
            manifest.len()
        );
    }

    for attachment in manifest {
        conn.execute(
            "INSERT OR REPLACE INTO attachments
             (content_hash, mime_type, size_bytes, file_path, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                attachment.content_hash,
                attachment.mime_type,
                attachment.size_bytes,
                attachment.file_path,
                created_at,
            ],
        )?;
    }
    Ok(())
}

fn build_fts5_index(conn: &Connection, sessions_dir: &Path, rebuild: bool) -> Result<()> {
    if rebuild {
        conn.execute("DELETE FROM session_transcripts_fts", [])?;
    }

    // Get all session IDs
    let mut stmt = conn.prepare("SELECT session_id FROM sessions")?;
    let session_ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    let total = session_ids.len();

    for (idx, session_id) in session_ids.iter().enumerate() {
        if idx % 10 == 0 {
            log::info!("  Indexing transcripts: {}/{}", idx, total);
        }

        let transcript_path = sessions_dir.join(session_id).join("transcript.md");
        if !transcript_path.exists() {
            continue;
        }

        let content = fs::read_to_string(&transcript_path)?;

        conn.execute(
            "INSERT OR REPLACE INTO session_transcripts_fts (session_id, content) VALUES (?1, ?2)",
            rusqlite::params![session_id, content],
        )?;
    }

    Ok(())
}

struct IndexStats {
    session_count: i64,
    agent_count: i64,
    workspace_count: i64,
    attachment_count: i64,
    fts_token_count: i64,
}

fn collect_stats(conn: &Connection) -> Result<IndexStats> {
    let session_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;

    let agent_count: i64 = conn.query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))?;

    let workspace_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;

    let attachment_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| row.get(0))?;

    // Estimate FTS token count
    let fts_token_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM session_transcripts_fts", [], |row| {
            row.get(0)
        })
        .unwrap_or(0)
        * 100; // Rough estimate

    Ok(IndexStats {
        session_count,
        agent_count,
        workspace_count,
        attachment_count,
        fts_token_count,
    })
}
