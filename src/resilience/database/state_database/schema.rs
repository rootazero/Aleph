/// Database schema definitions for `StateDatabase`
///
/// Contains the SQL DDL statements for all tables, indexes, triggers,
/// and virtual tables used by the resilience subsystem.
use super::StateDatabase;

impl StateDatabase {
    /// SQL for creating the database schema
    pub(super) const fn schema_sql() -> &'static str {
        r#"
            -- Metadata table for schema versioning
            CREATE TABLE IF NOT EXISTS schema_info (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Main memories table
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                window_title TEXT NOT NULL,
                user_input TEXT NOT NULL,
                ai_output TEXT NOT NULL,
                embedding BLOB NOT NULL,
                timestamp INTEGER NOT NULL,
                session_id TEXT NOT NULL
            );

            -- Index for fast window-title-based filtering
            CREATE INDEX IF NOT EXISTS idx_window_title ON memories(window_title);

            -- Index for timestamp-based queries (retention policy)
            CREATE INDEX IF NOT EXISTS idx_timestamp ON memories(timestamp);

            -- Index for topic-based queries (multi-turn conversation deletion)
            CREATE INDEX IF NOT EXISTS idx_session_id ON memories(session_id);

            -- Compression session audit table
            CREATE TABLE IF NOT EXISTS compression_sessions (
                id TEXT PRIMARY KEY,
                source_memory_ids TEXT NOT NULL,
                extracted_fact_ids TEXT NOT NULL,
                compressed_at INTEGER NOT NULL,
                provider_used TEXT NOT NULL,
                duration_ms INTEGER NOT NULL
            );

            -- Index for compression history queries
            CREATE INDEX IF NOT EXISTS idx_compression_time ON compression_sessions(compressed_at);

            -- ================================================================
            -- Memory Graph Tables (Phase 9 - The Brain)
            -- ================================================================

            -- Graph nodes (entities)
            CREATE TABLE IF NOT EXISTS graph_nodes (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                aliases_json TEXT NOT NULL DEFAULT '[]',
                metadata_json TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                decay_score REAL NOT NULL DEFAULT 1.0
            );

            CREATE INDEX IF NOT EXISTS idx_graph_nodes_kind_name ON graph_nodes(kind, name);
            CREATE INDEX IF NOT EXISTS idx_graph_nodes_updated ON graph_nodes(updated_at);

            -- Graph edges (relationships)
            CREATE TABLE IF NOT EXISTS graph_edges (
                id TEXT PRIMARY KEY,
                from_id TEXT NOT NULL,
                to_id TEXT NOT NULL,
                relation TEXT NOT NULL,
                weight REAL NOT NULL,
                confidence REAL NOT NULL,
                context_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL,
                decay_score REAL NOT NULL DEFAULT 1.0
            );

            CREATE INDEX IF NOT EXISTS idx_graph_edges_from ON graph_edges(from_id);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_to ON graph_edges(to_id);
            CREATE INDEX IF NOT EXISTS idx_graph_edges_context ON graph_edges(context_key);

            -- Graph aliases
            CREATE TABLE IF NOT EXISTS graph_aliases (
                alias TEXT NOT NULL,
                normalized_alias TEXT NOT NULL,
                node_id TEXT NOT NULL,
                PRIMARY KEY (normalized_alias, node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_graph_aliases_norm ON graph_aliases(normalized_alias);

            -- Memory-to-entity links
            CREATE TABLE IF NOT EXISTS memory_entities (
                memory_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                weight REAL NOT NULL DEFAULT 1.0,
                source TEXT NOT NULL,
                PRIMARY KEY (memory_id, node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_memory_entities_node ON memory_entities(node_id);

            -- Daily insight summaries
            CREATE TABLE IF NOT EXISTS daily_insights (
                date TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                source_memory_count INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );

            -- DreamDaemon status tracking
            CREATE TABLE IF NOT EXISTS dream_status (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                last_run_at INTEGER,
                last_status TEXT,
                last_duration_ms INTEGER
            );

            -- NOTE: `memory_audit_log` used to be created here. It had an
            -- `actor` column and zero writers anywhere in the tree, so it was
            -- an audit trail that could only ever answer "nothing happened".
            -- Dropped by `drop_obsolete_tables`; agent accountability lives in
            -- `crate::identity` (recorded at the tool chokepoint, with a
            -- read/verify surface). Do not recreate it.

            -- ================================================================
            -- Memory Event Sourcing (append-only event log)
            -- ================================================================

            CREATE TABLE IF NOT EXISTS memory_events (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                fact_id        TEXT NOT NULL,
                seq            INTEGER NOT NULL,
                event_type     TEXT NOT NULL,
                event_json     TEXT NOT NULL,
                actor          TEXT NOT NULL,
                tier           TEXT NOT NULL,
                timestamp      INTEGER NOT NULL,
                correlation_id TEXT,

                UNIQUE(fact_id, seq)
            );

            CREATE INDEX IF NOT EXISTS idx_me_fact_id
                ON memory_events(fact_id);
            CREATE INDEX IF NOT EXISTS idx_me_timestamp
                ON memory_events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_me_event_type
                ON memory_events(event_type);
            CREATE INDEX IF NOT EXISTS idx_me_correlation
                ON memory_events(correlation_id);


            -- ================================================================
            -- sqlite-vec Virtual Tables: created dynamically via vec_schema_sql()
            -- ================================================================

            -- ================================================================
            -- FTS5 Full-Text Search Tables (Hybrid Search)
            -- ================================================================

            -- Full-text index for memories
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                user_input,
                ai_output,
                id UNINDEXED,
                content='memories',
                content_rowid='rowid'
            );

            -- Sync trigger: memories insert
            CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
                INSERT INTO memories_fts(rowid, user_input, ai_output, id)
                VALUES (new.rowid, new.user_input, new.ai_output, new.id);
            END;

            -- Sync trigger: memories delete
            CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
                INSERT INTO memories_fts(memories_fts, rowid, user_input, ai_output, id)
                VALUES ('delete', old.rowid, old.user_input, old.ai_output, old.id);
            END;

            -- ================================================================
            -- Multi-Agent Resilience Tables (Phase 10)
            -- ================================================================

            -- Agent task tracking with recovery support
            CREATE TABLE IF NOT EXISTS agent_tasks (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                task_prompt TEXT NOT NULL,
                status TEXT NOT NULL,  -- Pending, Running, Completed, Failed, Interrupted, Idle, Swapped
                risk_level TEXT NOT NULL,  -- Low, High
                lane TEXT NOT NULL DEFAULT 'subagent',  -- main, subagent

                -- Recovery data (for Shadow Replay)
                checkpoint_snapshot_path TEXT,
                last_tool_call_id TEXT,

                -- Governance
                recursion_depth INTEGER DEFAULT 0,
                parent_task_id TEXT,

                -- Audit timestamps
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                started_at INTEGER,
                completed_at INTEGER,

                -- Extensible metadata
                metadata_json TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_agent_tasks_parent_session ON agent_tasks(parent_session_id);
            CREATE INDEX IF NOT EXISTS idx_agent_tasks_status ON agent_tasks(status);
            CREATE INDEX IF NOT EXISTS idx_agent_tasks_parent_task ON agent_tasks(parent_task_id);

            -- Task execution traces (for Shadow Replay / deterministic recovery)
            CREATE TABLE IF NOT EXISTS task_traces (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                step_index INTEGER NOT NULL,
                event_kind TEXT NOT NULL,  -- turn_started, tool_call_completed, etc.
                event_json TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
            );

            CREATE INDEX IF NOT EXISTS idx_task_traces_task ON task_traces(task_id, step_index);

            -- ================================================================
            -- Group Chat Tables
            -- ================================================================

            CREATE TABLE IF NOT EXISTS group_chat_sessions (
                id TEXT PRIMARY KEY,
                topic TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                source_channel TEXT NOT NULL,
                source_session_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                -- P1 ownership stamp (mirrors SessionMetadata::stamp_attribution).
                -- NULL is the documented "operator-era fallback" sentinel —
                -- `stamped_owner_visible` resolves it like a pre-P1 session row.
                owner_user_id TEXT
            );

            CREATE TABLE IF NOT EXISTS group_chat_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES group_chat_sessions(id),
                round INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                speaker_type TEXT NOT NULL,
                speaker_id TEXT,
                speaker_name TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_gc_turns_session ON group_chat_turns(session_id);
            "#
    }

    /// SQL for creating vec0 virtual tables with dynamic dimension
    pub(super) fn vec_schema_sql(dim: u32) -> String {
        debug_assert!(
            dim > 0 && dim <= 16_384,
            "embedding dimension must be in 1..=16384, got {dim}"
        );
        format!(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS memories_vec USING vec0(
                embedding float[{dim}]
            );
            "#
        )
    }

    /// One-time migration: drop tables that exist in older `state_database`
    /// files but that nothing on HEAD reads or writes. Safe to re-run (uses
    /// IF EXISTS).
    ///
    /// * `memory_facts` / `facts_fts` / `facts_vec` — a planned CQRS read model
    ///   for `memory_events` that was never wired up. After the facts→notes
    ///   migration the notes layer is the actual materialized view. The DROP
    ///   order matters: virtual tables that reference `memory_facts` must be
    ///   dropped before the base table.
    /// * `memory_audit_log` — an explainability audit table with an `actor`
    ///   column and **no `INSERT` anywhere in the tree**. An empty table that
    ///   looks like an audit trail is worse than no table: an operator who
    ///   queries it concludes nothing happened. (Aleph deleted an entire
    ///   approval-audit subsystem for exactly this in 2026-07-14.) Agent
    ///   accountability now lives in [`crate::identity`], which records from
    ///   the tool chokepoint and ships its own read/verify surface.
    pub(super) fn drop_obsolete_tables(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "DROP TABLE IF EXISTS facts_vec;
             DROP TABLE IF EXISTS facts_fts;
             DROP TABLE IF EXISTS memory_facts;
             DROP TABLE IF EXISTS memory_audit_log;",
        )
    }
}
