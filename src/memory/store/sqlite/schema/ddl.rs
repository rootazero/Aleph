pub const RECALL_SIGNALS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS recall_signals (
    id          TEXT PRIMARY KEY,
    note_path   TEXT NOT NULL,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    query_hash  TEXT NOT NULL,
    query_text  TEXT NOT NULL,
    channel     TEXT NOT NULL DEFAULT 'unknown',
    score       REAL NOT NULL,
    session_id  TEXT,
    namespace   TEXT NOT NULL DEFAULT 'owner',
    created_at  INTEGER NOT NULL,
    day_bucket  TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_recall_dedup
    ON recall_signals(agent_id, note_path, query_hash, day_bucket, channel);
CREATE INDEX IF NOT EXISTS idx_recall_agent_path
    ON recall_signals(agent_id, note_path);
CREATE INDEX IF NOT EXISTS idx_recall_day_bucket
    ON recall_signals(day_bucket);
CREATE INDEX IF NOT EXISTS idx_recall_query_bucket_channel
    ON recall_signals(query_hash, day_bucket, channel);
"#;

pub const DREAM_REPORTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_reports (
    id              TEXT PRIMARY KEY,
    pipeline_type   TEXT NOT NULL,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER NOT NULL,
    duration_ms     INTEGER NOT NULL,
    synthesis_count INTEGER NOT NULL DEFAULT 0,
    notes_consolidated INTEGER NOT NULL DEFAULT 0,
    notes_woven        INTEGER NOT NULL DEFAULT 0,
    notes_archived     INTEGER NOT NULL DEFAULT 0,
    feedback_distilled INTEGER NOT NULL DEFAULT 0,
    errors          TEXT,
    -- Storage partition key: the base agent id, or `{base}__proj-*` for a
    -- project sub-cycle. The default must stay in step with `DEFAULT_AGENT_ID`
    -- (guarded by `dream_reports_namespace_default_is_the_default_agent`); it
    -- used to read `'owner'`, an id no agent has ever had.
    namespace       TEXT NOT NULL DEFAULT 'main',
    evolution_json  TEXT,
    decision_json   TEXT
);

CREATE INDEX IF NOT EXISTS idx_dream_reports_started
    ON dream_reports(started_at);
"#;

pub const DREAM_STATUS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS dream_status (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at      INTEGER,
    last_status      TEXT,
    last_duration_ms INTEGER
);
"#;

pub const DAILY_INSIGHTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS daily_insights (
    date                 TEXT PRIMARY KEY,
    content              TEXT NOT NULL,
    source_memory_count  INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL
);
"#;

pub const COMPRESSION_METADATA_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS compression_metadata (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

pub const CREATE_RAW_MEMORIES: &str = "
CREATE TABLE IF NOT EXISTS raw_memories (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    source          TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    session_id      TEXT,
    path            TEXT,
    attachment_text TEXT,
    is_processed    INTEGER DEFAULT 0,
    created_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_raw_unprocessed ON raw_memories(is_processed, created_at)
    WHERE is_processed = 0;
CREATE INDEX IF NOT EXISTS idx_raw_agent ON raw_memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_raw_session ON raw_memories(session_id);
";

pub const NOTES_INDEX_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_index (
    path            TEXT NOT NULL,
    filename        TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    category        TEXT NOT NULL,
    tags_json       TEXT NOT NULL DEFAULT '[]',
    aliases_json    TEXT NOT NULL DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash    TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_notes_filename ON notes_index(filename);
CREATE INDEX IF NOT EXISTS idx_notes_agent ON notes_index(agent_id);
CREATE INDEX IF NOT EXISTS idx_notes_category ON notes_index(category);
CREATE INDEX IF NOT EXISTS idx_notes_filename_agent ON notes_index(agent_id, filename);
"#;

pub const NOTES_LINKS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    relation    TEXT,
    confidence  REAL NOT NULL DEFAULT 1.0,
    resolved_by TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    label       TEXT,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to_raw ON notes_links(agent_id, to_raw);
"#;

pub const NOTES_SOURCES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_sources (
    agent_id   TEXT NOT NULL DEFAULT 'default',
    note_path  TEXT NOT NULL,
    source_ref TEXT NOT NULL,
    UNIQUE(agent_id, note_path, source_ref)
);
CREATE INDEX IF NOT EXISTS idx_notes_sources_ref ON notes_sources(agent_id, source_ref);
"#;

pub const NOTES_GRAPH_CACHE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_graph_cache (
    agent_id     TEXT NOT NULL DEFAULT 'default',
    node_path    TEXT NOT NULL,
    community_id INTEGER NOT NULL,
    cohesion     REAL NOT NULL DEFAULT 0,
    degree       INTEGER NOT NULL DEFAULT 0,
    updated_at   INTEGER NOT NULL,
    PRIMARY KEY (agent_id, node_path)
);
"#;

pub const NOTES_GRAPH_INSIGHTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_graph_insights (
    agent_id   TEXT NOT NULL DEFAULT 'default',
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notes_graph_insights ON notes_graph_insights(agent_id, kind);
"#;

pub const NOTES_GRAPH_RELATED_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_graph_related (
    agent_id     TEXT NOT NULL DEFAULT 'default',
    node_path    TEXT NOT NULL,
    related_path TEXT NOT NULL,
    score        REAL NOT NULL DEFAULT 0,
    PRIMARY KEY (agent_id, node_path, related_path)
);
CREATE INDEX IF NOT EXISTS idx_notes_graph_related ON notes_graph_related(agent_id, node_path);
"#;

pub const NOTES_FTS_DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path,
    filename,
    content,
    agent_id UNINDEXED,
    tokenize='unicode61'
);
"#;

pub const NOTES_FTS_META_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_fts_meta (
    agent_id     TEXT NOT NULL,
    path         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
);
"#;

// Trigram-tokenized companion to `notes_fts`. `unicode61` indexes a run of
// CJK ideographs as a single token, so a substring query (`记忆`) can never
// match inside a longer token (`记忆管理`). The `trigram` tokenizer indexes
// overlapping 3-char windows, enabling substring/phrase recall for CJK (and
// any script) at the cost of requiring queries ≥3 chars. Kept byte-for-byte
// in sync with `notes_fts` on every write/delete; queried only for CJK-bearing
// queries so ASCII search behaviour is unchanged. See `search_notes_fts`.
pub const NOTES_FTS_TRIGRAM_DDL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts_trigram USING fts5(
    path,
    filename,
    content,
    agent_id UNINDEXED,
    tokenize='trigram'
);
"#;

pub const NOTES_PROVENANCE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_provenance (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    fact_idx    INTEGER NOT NULL,
    origin      TEXT NOT NULL,
    source_kind TEXT,
    source_id   TEXT,
    inferred    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_prov_path ON notes_provenance(agent_id, note_path);
CREATE INDEX IF NOT EXISTS idx_prov_source ON notes_provenance(source_kind, source_id);
"#;

pub const NOTES_REVIEW_QUEUE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_review_queue (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    candidate_json  TEXT NOT NULL,
    severity        TEXT NOT NULL,
    confidence      REAL NOT NULL,
    reason          TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    retry_count     INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    decided_at      INTEGER,
    decision_actor  TEXT
);
CREATE INDEX IF NOT EXISTS idx_review_pending
    ON notes_review_queue(agent_id, status, created_at);
"#;

pub const NOTES_REVIEW_ARCHIVE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_review_archive (
    id              TEXT PRIMARY KEY,
    agent_id        TEXT NOT NULL,
    candidate_json  TEXT NOT NULL,
    final_status    TEXT NOT NULL,
    reason          TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    archived_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_archive_age
    ON notes_review_archive(archived_at);
"#;

pub const ASSEMBLY_LOGS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS assembly_logs (
    id                 TEXT PRIMARY KEY,
    agent_id           TEXT NOT NULL,
    session_id         TEXT,
    query_hash         TEXT NOT NULL,
    strategy           TEXT NOT NULL,
    used_fallback      INTEGER NOT NULL DEFAULT 0,
    fallback_reason    TEXT,
    candidates_count   INTEGER NOT NULL,
    selected_item_ids  TEXT NOT NULL,
    total_tokens       INTEGER NOT NULL,
    rerank_latency_ms  INTEGER,
    total_latency_ms   INTEGER NOT NULL,
    created_at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_assembly_logs_agent_created
    ON assembly_logs(agent_id, created_at);
"#;

pub const MEMORY_WRITE_DECISIONS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS memory_write_decisions (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    action      TEXT NOT NULL,
    reason      TEXT NOT NULL,
    subject     TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memory_write_decisions_agent_created
    ON memory_write_decisions(agent_id, created_at);
"#;

pub(crate) const CREATE_QUERY_FILED: &str = r#"
CREATE TABLE IF NOT EXISTS query_filed (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    query_hash  TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    session_id  TEXT,
    filed_at    INTEGER NOT NULL,
    UNIQUE(agent_id, query_hash)
);
CREATE INDEX IF NOT EXISTS idx_query_filed_agent ON query_filed(agent_id);
"#;

pub const NOTES_VEC_MAP_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS notes_vec_map (
    rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    -- The note's `content_hash` at the moment its vector was computed. Without
    -- it nothing can tell a fresh vector from one left behind by a swallowed
    -- embed failure, and `reembed_all` has to re-embed the whole corpus to be
    -- sure. Empty string = provenance unknown => always treated as stale, so a
    -- caller that does not supply a hash fails safe toward re-embedding.
    embedded_hash   TEXT NOT NULL DEFAULT '',
    embedded_at     INTEGER NOT NULL DEFAULT 0,
    UNIQUE(agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_notes_vec_map_agent ON notes_vec_map(agent_id);
"#;

pub const ROUTING_EXPERIENCE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS routing_experiences (
    id                  TEXT PRIMARY KEY,
    agent_id            TEXT NOT NULL,
    model_id            TEXT NOT NULL,
    provider_id         TEXT NOT NULL,
    terminate_reason    TEXT NOT NULL,
    iterations          INTEGER NOT NULL,
    tool_calls          INTEGER NOT NULL,
    tool_error_count    INTEGER NOT NULL,
    tool_call_total     INTEGER NOT NULL,
    tok_input           INTEGER NOT NULL,
    tok_output          INTEGER NOT NULL,
    tok_cache_read      INTEGER NOT NULL,
    tok_cache_creation  INTEGER NOT NULL,
    tok_reasoning       INTEGER NOT NULL,
    estimated_cost      REAL,
    duration_ms         INTEGER NOT NULL,
    context_tokens      INTEGER NOT NULL,
    context_window      INTEGER NOT NULL,
    created_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_routing_experiences_agent
    ON routing_experiences(agent_id, created_at DESC);
CREATE TABLE IF NOT EXISTS routing_exp_vec_map (
    rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
    routing_exp_id  TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    dim             INTEGER NOT NULL DEFAULT 768,
    UNIQUE(agent_id, routing_exp_id)
);
CREATE INDEX IF NOT EXISTS idx_routing_exp_vec_map_agent ON routing_exp_vec_map(agent_id);
"#;

#[must_use]
pub fn vec_table_ddl(dim: u32, table_name: &str) -> String {
    format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {table_name} USING vec0(\n    \
             rowid   INTEGER PRIMARY KEY,\n    \
             embedding float[{dim}]\n\
         );\n"
    )
}
