//! SQLite schema for session metadata and indexes.

use rusqlite::Connection;

use crate::error::AlephError;

/// Initialize all session-related tables.
pub fn init_session_tables(conn: &Connection) -> Result<(), AlephError> {
    conn.execute_batch(
        r#"
        -- Agents table
        CREATE TABLE IF NOT EXISTS agents (
            agent_id TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            last_active_at INTEGER NOT NULL
        );

        -- Workspaces table
        CREATE TABLE IF NOT EXISTS workspaces (
            workspace_id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );

        -- Sessions metadata table
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            workspace_id TEXT,
            started_at INTEGER NOT NULL,
            ended_at INTEGER,
            duration_seconds INTEGER,
            transcript_path TEXT NOT NULL,
            tool_outputs_path TEXT NOT NULL,
            message_count INTEGER DEFAULT 0,
            tool_call_count INTEGER DEFAULT 0,
            content_hash TEXT,
            compression_status TEXT DEFAULT 'pending',
            tags TEXT DEFAULT '[]',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (agent_id) REFERENCES agents(agent_id),
            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id)
        );

        -- Attachments table
        CREATE TABLE IF NOT EXISTS attachments (
            attachment_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            file_path TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            uploaded_at INTEGER NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
        );

        -- Raw-memory bodies and source details live in Markdown, never SQLite.
        -- Processing state belongs to this persistent work index.
        CREATE TABLE IF NOT EXISTS raw_memory_index (
            id TEXT PRIMARY KEY NOT NULL,
            file_id TEXT NOT NULL UNIQUE,
            agent_id TEXT NOT NULL,
            session_id TEXT,
            path TEXT,
            source TEXT NOT NULL,
            is_processed INTEGER NOT NULL CHECK (is_processed IN (0, 1)),
            created_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_raw_memory_pending
            ON raw_memory_index(agent_id, created_at) WHERE is_processed = 0;
        CREATE UNIQUE INDEX IF NOT EXISTS idx_raw_memory_agent_path_unique
            ON raw_memory_index(agent_id, path) WHERE path IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_raw_memory_path
            ON raw_memory_index(agent_id, path, created_at);
        CREATE INDEX IF NOT EXISTS idx_raw_memory_session
            ON raw_memory_index(agent_id, session_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_raw_memory_source
            ON raw_memory_index(agent_id, source, created_at);

        -- FTS5 index for session transcripts
        CREATE VIRTUAL TABLE IF NOT EXISTS session_transcripts_fts USING fts5(
            session_id UNINDEXED,
            content,
            tokenize = 'porter ascii'
        );

        -- Indexes for performance
        CREATE INDEX IF NOT EXISTS idx_sessions_agent_id ON sessions(agent_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_workspace_id ON sessions(workspace_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_started_at ON sessions(started_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_compression_status ON sessions(compression_status);
        CREATE INDEX IF NOT EXISTS idx_attachments_session_id ON attachments(session_id);
        CREATE INDEX IF NOT EXISTS idx_attachments_content_hash ON attachments(content_hash);
        "#,
    )
    .map_err(|e| AlephError::other(format!("Failed to create session tables: {}", e)))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_init_session_tables() {
        let conn = Connection::open_in_memory().unwrap();
        init_session_tables(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"agents".to_string()));
        assert!(tables.contains(&"workspaces".to_string()));
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"attachments".to_string()));
        assert!(tables.contains(&"raw_memory_index".to_string()));
        assert!(!tables.contains(&"raw_memories".to_string()));
    }
}
