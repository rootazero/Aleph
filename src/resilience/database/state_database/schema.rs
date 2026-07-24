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

            -- ================================================================
            -- Audit Log for Memory Operations (Explainability)
            -- ================================================================

            -- Audit log for memory operations (explainability)
            CREATE TABLE IF NOT EXISTS memory_audit_log (
                id TEXT PRIMARY KEY,
                fact_id TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT,
                actor TEXT NOT NULL,
                details TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_audit_fact
                ON memory_audit_log(fact_id);
            CREATE INDEX IF NOT EXISTS idx_audit_time
                ON memory_audit_log(created_at);
            CREATE INDEX IF NOT EXISTS idx_audit_action
                ON memory_audit_log(action);

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
            -- POE Event Sourcing (append-only event log)
            -- ================================================================

            CREATE TABLE IF NOT EXISTS poe_events (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id        TEXT NOT NULL,
                seq            INTEGER NOT NULL,
                event_type     TEXT NOT NULL,
                event_json     TEXT NOT NULL,
                tier           TEXT NOT NULL CHECK(tier IN ('skeleton', 'pulse')),
                timestamp      INTEGER NOT NULL,
                correlation_id TEXT,

                UNIQUE(task_id, seq)
            );

            CREATE INDEX IF NOT EXISTS idx_pe_task_id
                ON poe_events(task_id);
            CREATE INDEX IF NOT EXISTS idx_pe_event_type
                ON poe_events(event_type);
            CREATE INDEX IF NOT EXISTS idx_pe_timestamp
                ON poe_events(timestamp);

            -- ================================================================
            -- POE Trust Scores: Pattern-level success metrics
            -- ================================================================

            CREATE TABLE IF NOT EXISTS poe_trust_scores (
                pattern_id TEXT PRIMARY KEY,
                total_executions INTEGER NOT NULL DEFAULT 0,
                successful_executions INTEGER NOT NULL DEFAULT 0,
                trust_score REAL NOT NULL DEFAULT 0.0,
                last_updated INTEGER NOT NULL
            );

            -- ================================================================
            -- POE Contracts: Pending contract persistence
            -- ================================================================

            CREATE TABLE IF NOT EXISTS poe_contracts (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                instruction TEXT NOT NULL,
                manifest_json TEXT NOT NULL,
                context_json TEXT,
                status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'signed', 'rejected', 'expired')),
                created_at INTEGER NOT NULL,
                signed_at INTEGER,
                expires_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_pc_status ON poe_contracts(status);
            CREATE INDEX IF NOT EXISTS idx_pc_task_id ON poe_contracts(task_id);

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

            -- Agent events (Skeleton & Pulse model)
            CREATE TABLE IF NOT EXISTS agent_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                is_structural INTEGER DEFAULT 0,  -- 1 for skeleton events, 0 for pulse
                timestamp INTEGER NOT NULL,
                FOREIGN KEY(task_id) REFERENCES agent_tasks(id)
            );

            CREATE INDEX IF NOT EXISTS idx_agent_events_task_seq ON agent_events(task_id, seq);
            CREATE INDEX IF NOT EXISTS idx_agent_events_structural ON agent_events(task_id, is_structural) WHERE is_structural = 1;

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
                updated_at INTEGER NOT NULL
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

    /// One-time migration: drop obsolete `memory_facts` / `facts_fts` / `facts_vec`
    /// tables from existing `state_database` files. Safe to re-run (uses IF EXISTS).
    ///
    /// These were a planned CQRS read model for `memory_events` but were never wired
    /// up. After the facts→notes migration the notes layer is the actual materialized
    /// view. The DROP order matters: virtual tables that reference `memory_facts` must
    /// be dropped before the base table.
    pub(super) fn drop_obsolete_state_facts_tables(
        conn: &rusqlite::Connection,
    ) -> rusqlite::Result<()> {
        conn.execute_batch(
            "DROP TABLE IF EXISTS facts_vec;
             DROP TABLE IF EXISTS facts_fts;
             DROP TABLE IF EXISTS memory_facts;",
        )
    }
}
