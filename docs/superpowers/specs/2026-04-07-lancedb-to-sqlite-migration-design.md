# LanceDB → SQLite + sqlite-vec Migration Design

## Problem

LanceDB causes ~400MB memory usage at startup due to 35,000+ fragment files across 4 tables (2.1GB on disk). Each `insert_fact()` creates a new fragment file (append-only design), and LanceDB loads all fragment metadata into memory when opening tables. This is the dominant memory consumer in Aleph.

## Decision

Replace LanceDB entirely with SQLite + sqlite-vec. Remove the `SessionStore` trait (memories table). Delete all LanceDB code and dependencies. Cold start with no data migration.

## Architecture

```
Before:
  LanceMemoryBackend (LanceDB, 4 tables, 2.1GB, ~400MB RAM)
    ├── MemoryStore  (facts — vector + text + hybrid search)
    ├── GraphStore   (graph_nodes, graph_edges — relational)
    └── SessionStore (memories — vector search) ← REMOVE

After:
  SqliteMemoryBackend (SQLite + sqlite-vec, 1 file, ~50MB, <10MB RAM)
    ├── MemoryStore  (facts — vector via sqlite-vec, text via FTS5)
    └── GraphStore   (graph_nodes, graph_edges — pure SQL)
  SessionStore trait ← DELETED
```

## Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Vector search | sqlite-vec extension | Scales to 100K+ facts, true ANN index, statically linkable |
| SessionStore | Remove entirely | facts = distilled knowledge from memories; raw conversations already in SessionManager SQLite |
| DB file | `~/.aleph/data/memory.db` (independent) | Separate from SessionManager; facts are permanent, sessions are ephemeral |
| Data migration | Cold start, no migration | Avoids one-time migration code; facts will re-accumulate naturally |
| Old data | Delete LanceDB SDK, code, and data directory | Clean break, no legacy baggage |

## Storage Schema

### facts table

```sql
CREATE TABLE facts (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  fact_type TEXT NOT NULL,
  fact_source TEXT NOT NULL,
  specificity TEXT NOT NULL,
  temporal_scope TEXT NOT NULL,
  layer TEXT NOT NULL,
  category TEXT NOT NULL,
  path TEXT NOT NULL,
  parent_path TEXT NOT NULL,
  namespace TEXT NOT NULL,
  agent TEXT NOT NULL,
  tags TEXT,              -- JSON array
  source_memory_ids TEXT, -- JSON array
  content_hash TEXT NOT NULL,
  confidence REAL NOT NULL,
  decay_score REAL NOT NULL DEFAULT 1.0,
  is_valid INTEGER NOT NULL DEFAULT 1,
  invalidation_reason TEXT,
  embedding_model TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  decay_invalidated_at INTEGER,
  version INTEGER NOT NULL DEFAULT 1,
  tier TEXT NOT NULL DEFAULT 'core',
  scope TEXT NOT NULL DEFAULT 'global',
  persona_id TEXT,
  strength REAL NOT NULL DEFAULT 1.0,
  access_count INTEGER NOT NULL DEFAULT 0,
  last_accessed_at INTEGER
);

CREATE VIRTUAL TABLE facts_fts USING fts5(
  content,
  content=facts,
  content_rowid=rowid
);

CREATE INDEX idx_facts_path ON facts(path);
CREATE INDEX idx_facts_parent_path ON facts(parent_path);
CREATE INDEX idx_facts_namespace ON facts(namespace);
CREATE INDEX idx_facts_agent ON facts(agent);
CREATE INDEX idx_facts_is_valid ON facts(is_valid);
CREATE INDEX idx_facts_content_hash ON facts(content_hash);
CREATE INDEX idx_facts_category ON facts(category);
CREATE INDEX idx_facts_layer ON facts(layer);
```

### facts_vec (sqlite-vec virtual table)

```sql
-- One virtual table per embedding dimension.
-- Create tables for dimensions actually in use.
CREATE VIRTUAL TABLE facts_vec_768 USING vec0(
  fact_id TEXT PRIMARY KEY,
  embedding FLOAT[768]
);

CREATE VIRTUAL TABLE facts_vec_1024 USING vec0(
  fact_id TEXT PRIMARY KEY,
  embedding FLOAT[1024]
);

CREATE VIRTUAL TABLE facts_vec_1536 USING vec0(
  fact_id TEXT PRIMARY KEY,
  embedding FLOAT[1536]
);
```

Vector operations:
- **Insert**: insert into both `facts` and the matching `facts_vec_{dim}` table
- **Search**: query the vec table with `WHERE embedding MATCH ?` and join with `facts`
- **Delete**: delete from both tables
- **dim_hint**: routes to the correct vec table (768/1024/1536)

### graph_nodes table

```sql
CREATE TABLE graph_nodes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  aliases TEXT,     -- JSON array
  metadata TEXT,    -- JSON object
  decay_score REAL NOT NULL DEFAULT 1.0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  agent TEXT NOT NULL
);

CREATE INDEX idx_nodes_name ON graph_nodes(name);
CREATE INDEX idx_nodes_kind ON graph_nodes(kind);
CREATE INDEX idx_nodes_agent ON graph_nodes(agent);
```

### graph_edges table

```sql
CREATE TABLE graph_edges (
  id TEXT PRIMARY KEY,
  from_id TEXT NOT NULL,
  to_id TEXT NOT NULL,
  relation TEXT NOT NULL,
  weight REAL NOT NULL,
  confidence REAL NOT NULL,
  context_key TEXT NOT NULL,
  decay_score REAL NOT NULL DEFAULT 1.0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  agent TEXT NOT NULL
);

CREATE INDEX idx_edges_from ON graph_edges(from_id);
CREATE INDEX idx_edges_to ON graph_edges(to_id);
CREATE INDEX idx_edges_context ON graph_edges(context_key);
CREATE INDEX idx_edges_relation ON graph_edges(relation);
```

## New Backend Structure

```
src/memory/store/
├── mod.rs          -- MemoryStore + GraphStore traits (SessionStore REMOVED)
├── types.rs        -- Unchanged
└── sqlite/         -- NEW (replaces lance/)
    ├── mod.rs      -- SqliteMemoryBackend struct, init, connection management
    ├── schema.rs   -- DDL statements, migration logic
    ├── facts.rs    -- MemoryStore trait implementation
    ├── graph.rs    -- GraphStore trait implementation
    └── vec.rs      -- sqlite-vec extension loading + vector operation helpers
```

## Trait Changes

### MemoryStore — UNCHANGED

All method signatures remain identical. Only the implementation changes from LanceDB to SQLite.

### GraphStore — UNCHANGED

All method signatures remain identical.

### SessionStore — DELETED

Remove the entire trait definition from `src/memory/store/mod.rs` and all implementations/callers.

## Deletion Checklist

### Code to delete

| Path | Reason |
|------|--------|
| `src/memory/store/lance/` | Entire LanceDB backend directory |
| `SessionStore` trait in `src/memory/store/mod.rs` | No longer needed |
| `src/memory/store/lance/sessions.rs` | SessionStore impl |
| `src/memory/store/lance/arrow_convert/` | Arrow ↔ Rust conversion |
| `src/memory/store/lance/schema.rs` | LanceDB Arrow schemas |
| `src/memory/store/lance/facts/` | LanceDB facts impl |
| `src/memory/store/lance/integration_tests.rs` | LanceDB integration tests |
| All `SessionStore` callers | retrieval fallback, compression, dreaming references |

### Dependencies to delete (Cargo.toml)

- `lancedb = "0.26"`
- `lance-index = "2.0"`
- `arrow-array = "57"`
- `arrow-schema = "57"`

### Dependencies to add (Cargo.toml)

- `sqlite-vec` (via rusqlite loadable extension or bundled build)

### Runtime cleanup

- Aleph no longer opens `~/.aleph/data/memory.lance/`
- Users should manually `rm -rf ~/.aleph/data/memory.lance` to free 2.1GB
- Startup log should print a notice if old `memory.lance` directory exists

## Interface Guarantee

All consumers of `MemoryStore` and `GraphStore` traits are unaffected. The only change visible to upper layers:

1. Initialization: `SqliteMemoryBackend::new(path)` replaces `LanceMemoryBackend::new(path)`
2. `SessionStore` references must be removed from callers (retrieval, compression, dreaming)
3. `StoreStats::total_memories` field → always 0 or remove

## Expected Impact

| Metric | Before | After |
|--------|--------|-------|
| Memory (startup) | ~400MB | <10MB |
| Disk | 2.1GB (35K files) | ~50MB (1 file) |
| Fragment files | 35,000+ | 0 |
| Dependencies | lancedb, lance-index, arrow-array, arrow-schema | sqlite-vec (via rusqlite) |
| Startup time | Slow (load fragment metadata) | Fast |
| Write pattern | 1 fragment per insert | In-place SQLite write |
