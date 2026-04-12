# LanceDB → SQLite + sqlite-vec Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace LanceDB with SQLite + sqlite-vec for the memory store, eliminating ~400MB memory overhead, 35K fragment files, and 2.1GB disk usage. Delete all LanceDB code and the SessionStore trait.

**Architecture:** Create a new `SqliteMemoryBackend` implementing `MemoryStore` + `GraphStore` traits using rusqlite + sqlite-vec (already in the project). Remove `SessionStore` trait and all callers. Delete `src/memory/store/lance/` and LanceDB dependencies. Upper layers only depend on traits — no API changes.

**Tech Stack:** Rust, rusqlite (bundled), sqlite-vec 0.1.6 (already in Cargo.toml), FTS5

**Important:** The project already uses sqlite-vec in `src/resilience/database/state_database/` — follow that established pattern for extension loading, vec0 table creation, and vector operations.

**Important:** There are TWO different `SessionStore` traits in the codebase. Only delete the MEMORY one at `src/memory/store/mod.rs:355-430`. Do NOT touch the TEAM one at `src/teams/sessions/store.rs`.

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/memory/store/sqlite/mod.rs` | Create | `SqliteMemoryBackend` struct, init, connection, extension loading |
| `src/memory/store/sqlite/schema.rs` | Create | DDL for facts, graph_nodes, graph_edges, vec0 tables |
| `src/memory/store/sqlite/facts.rs` | Create | `MemoryStore` trait implementation |
| `src/memory/store/sqlite/graph.rs` | Create | `GraphStore` trait implementation |
| `src/memory/store/sqlite/vec.rs` | Create | sqlite-vec helpers (insert/search/delete vectors) |
| `src/memory/store/mod.rs` | Modify | Remove `SessionStore` trait, add `pub mod sqlite`, update re-exports |
| `src/memory/store/lance/` | Delete | Entire LanceDB backend |
| `src/memory/ingestion.rs` | Modify | Remove SessionStore/insert_memory usage |
| `src/memory/retrieval.rs` | Modify | Remove search_memories fallback |
| `src/memory/fact_retrieval.rs` | Modify | Remove search_memories fallback |
| `src/memory/compression/service.rs` | Modify | Remove get_uncompressed_memories |
| `src/memory/cleanup.rs` | Modify | Remove delete_older_than (memories) |
| `src/memory/reembed.rs` | Modify | Remove get_all_memories |
| `src/gateway/handlers/memory.rs` | Modify | Remove get_recent_memories endpoint |
| `src/thinker/memory_context_provider.rs` | Modify | Remove search_memories |
| `src/capability/strategies/memory.rs` | Modify | Remove SessionStore import |
| `src/memory/store/lance/integration_tests.rs` | Delete | LanceDB tests |
| `src/bin/aleph-server/commands/start/builder/agent_init.rs` | Modify | Replace LanceMemoryBackend with SqliteMemoryBackend |
| `Cargo.toml` | Modify | Remove lancedb, lance-index, arrow-array, arrow-schema |

---

### Task 1: Create SqliteMemoryBackend Skeleton + Schema

Create the new SQLite backend structure with schema initialization. Follow the pattern established in `src/resilience/database/state_database/mod.rs` for sqlite-vec extension loading.

**Files:**
- Create: `src/memory/store/sqlite/mod.rs`
- Create: `src/memory/store/sqlite/schema.rs`
- Create: `src/memory/store/sqlite/vec.rs`
- Create: `src/memory/store/sqlite/facts.rs` (empty, with module declaration)
- Create: `src/memory/store/sqlite/graph.rs` (empty, with module declaration)

**Context files to read first:**
- `src/resilience/database/state_database/mod.rs` — existing sqlite-vec registration pattern (lines 28-43 for extension loading)
- `src/resilience/database/state_database/schema.rs` — existing vec0 table creation pattern
- `src/memory/store/lance/mod.rs` — current LanceMemoryBackend struct to understand the interface
- `src/memory/store/mod.rs` — trait definitions

- [ ] **Step 1: Create `src/memory/store/sqlite/schema.rs`**

DDL constants for all tables:

```rust
//! SQLite schema definitions for the memory store.

/// Schema version for migration tracking.
pub const SCHEMA_VERSION: i32 = 1;

/// Create all tables, indexes, and virtual tables.
pub fn init_schema(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(FACTS_DDL)?;
    conn.execute_batch(FACTS_INDEXES)?;
    conn.execute_batch(FACTS_FTS)?;
    conn.execute_batch(GRAPH_NODES_DDL)?;
    conn.execute_batch(GRAPH_EDGES_DDL)?;
    conn.execute_batch(GRAPH_INDEXES)?;
    // Vec0 tables created separately after sqlite-vec is loaded
    Ok(())
}

/// Create sqlite-vec virtual tables for vector search.
/// Must be called AFTER sqlite-vec extension is registered.
pub fn init_vec_tables(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(FACTS_VEC_768)?;
    conn.execute_batch(FACTS_VEC_1024)?;
    conn.execute_batch(FACTS_VEC_1536)?;
    Ok(())
}

const FACTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS facts (
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
  tags TEXT,
  source_memory_ids TEXT,
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
"#;

const FACTS_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_facts_path ON facts(path);
CREATE INDEX IF NOT EXISTS idx_facts_parent_path ON facts(parent_path);
CREATE INDEX IF NOT EXISTS idx_facts_namespace ON facts(namespace);
CREATE INDEX IF NOT EXISTS idx_facts_agent ON facts(agent);
CREATE INDEX IF NOT EXISTS idx_facts_is_valid ON facts(is_valid);
CREATE INDEX IF NOT EXISTS idx_facts_content_hash ON facts(content_hash);
CREATE INDEX IF NOT EXISTS idx_facts_category ON facts(category);
CREATE INDEX IF NOT EXISTS idx_facts_layer ON facts(layer);
"#;

const FACTS_FTS: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
  content,
  content=facts,
  content_rowid=rowid
);
"#;

const FACTS_VEC_768: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec_768 USING vec0(
  embedding float[768]
);
"#;

const FACTS_VEC_1024: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec_1024 USING vec0(
  embedding float[1024]
);
"#;

const FACTS_VEC_1536: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS facts_vec_1536 USING vec0(
  embedding float[1536]
);
"#;

const GRAPH_NODES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS graph_nodes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  aliases TEXT,
  metadata TEXT,
  decay_score REAL NOT NULL DEFAULT 1.0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  agent TEXT NOT NULL
);
"#;

const GRAPH_EDGES_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS graph_edges (
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
"#;

const GRAPH_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_edges_from ON graph_edges(from_id);
CREATE INDEX IF NOT EXISTS idx_edges_to ON graph_edges(to_id);
CREATE INDEX IF NOT EXISTS idx_edges_context ON graph_edges(context_key);
CREATE INDEX IF NOT EXISTS idx_edges_relation ON graph_edges(relation);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON graph_nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON graph_nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_agent ON graph_nodes(agent);
"#;
```

- [ ] **Step 2: Create `src/memory/store/sqlite/vec.rs`**

Helper functions for sqlite-vec operations. Follow the pattern from `src/resilience/database/state_database/mod.rs` lines 28-43.

```rust
//! sqlite-vec extension loading and vector operation helpers.

use rusqlite::params;
use sqlite_vec::sqlite3_vec_init;

/// Register the sqlite-vec extension globally for all future connections.
/// Safe to call multiple times — uses sqlite3_auto_extension which is idempotent.
pub fn register_sqlite_vec() {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }
}

/// Select the correct vec table name based on embedding dimension.
pub fn vec_table_for_dim(dim: u32) -> &'static str {
    match dim {
        768 => "facts_vec_768",
        1024 => "facts_vec_1024",
        1536 => "facts_vec_1536",
        _ => "facts_vec_1536", // fallback to largest
    }
}

/// Convert f32 slice to little-endian byte blob for sqlite-vec.
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Insert a vector into the appropriate vec0 table.
/// `rowid` must match the rowid of the fact in the `facts` table.
pub fn insert_vec(
    conn: &rusqlite::Connection,
    rowid: i64,
    embedding: &[f32],
    dim: u32,
) -> Result<(), rusqlite::Error> {
    let table = vec_table_for_dim(dim);
    let blob = embedding_to_blob(embedding);
    conn.execute(
        &format!("INSERT INTO {} (rowid, embedding) VALUES (?1, ?2)", table),
        params![rowid, blob],
    )?;
    Ok(())
}

/// Delete a vector from the appropriate vec0 table.
pub fn delete_vec(
    conn: &rusqlite::Connection,
    rowid: i64,
    dim: u32,
) -> Result<(), rusqlite::Error> {
    let table = vec_table_for_dim(dim);
    conn.execute(
        &format!("DELETE FROM {} WHERE rowid = ?1", table),
        params![rowid],
    )?;
    Ok(())
}

/// Delete vectors from ALL dimension tables for a given rowid.
pub fn delete_vec_all_dims(
    conn: &rusqlite::Connection,
    rowid: i64,
) -> Result<(), rusqlite::Error> {
    for table in &["facts_vec_768", "facts_vec_1024", "facts_vec_1536"] {
        conn.execute(
            &format!("DELETE FROM {} WHERE rowid = ?1", table),
            params![rowid],
        )?;
    }
    Ok(())
}

/// KNN search: find nearest neighbors in the specified vec table.
/// Returns Vec<(rowid, distance)> sorted by distance ascending.
pub fn knn_search(
    conn: &rusqlite::Connection,
    query_embedding: &[f32],
    dim: u32,
    k: usize,
) -> Result<Vec<(i64, f64)>, rusqlite::Error> {
    let table = vec_table_for_dim(dim);
    let blob = embedding_to_blob(query_embedding);
    let mut stmt = conn.prepare(&format!(
        "SELECT rowid, distance FROM {} WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
        table
    ))?;
    let results = stmt
        .query_map(params![blob, k as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(results)
}
```

- [ ] **Step 3: Create `src/memory/store/sqlite/mod.rs`**

```rust
//! SQLite + sqlite-vec backend for the memory store.
//!
//! Replaces the LanceDB backend with a single-file SQLite database.
//! Uses sqlite-vec extension for ANN vector search and FTS5 for text search.

pub mod schema;
pub mod vec;
pub mod facts;
pub mod graph;

use std::path::Path;
use std::sync::Mutex;
use tracing::info;

use crate::error::AlephError;

/// SQLite-backed memory store.
///
/// Provides MemoryStore (facts) and GraphStore (graph nodes/edges)
/// implementations using a single SQLite database file.
pub struct SqliteMemoryBackend {
    conn: Mutex<rusqlite::Connection>,
}

impl SqliteMemoryBackend {
    /// Open (or create) the memory database at the given path.
    pub fn new(db_path: &Path) -> Result<Self, AlephError> {
        // Register sqlite-vec extension (idempotent)
        vec::register_sqlite_vec();

        let conn = rusqlite::Connection::open(db_path)
            .map_err(|e| AlephError::Storage(format!("Failed to open memory DB: {}", e)))?;

        // Enable WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| AlephError::Storage(format!("PRAGMA failed: {}", e)))?;

        // Initialize schema
        schema::init_schema(&conn)
            .map_err(|e| AlephError::Storage(format!("Schema init failed: {}", e)))?;
        schema::init_vec_tables(&conn)
            .map_err(|e| AlephError::Storage(format!("Vec table init failed: {}", e)))?;

        info!(
            subsystem = "memory",
            event = "store_initialized",
            backend = "sqlite",
            db_path = %db_path.display(),
            "SQLite memory store initialized"
        );

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
}
```

- [ ] **Step 4: Create stub `facts.rs` and `graph.rs`**

Create minimal files with module structure so the project compiles:

`src/memory/store/sqlite/facts.rs`:
```rust
//! MemoryStore trait implementation for SqliteMemoryBackend.
// Implementation in Task 2
```

`src/memory/store/sqlite/graph.rs`:
```rust
//! GraphStore trait implementation for SqliteMemoryBackend.
// Implementation in Task 4
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore`
Note: This will NOT pass yet because SqliteMemoryBackend doesn't implement the traits. That's expected — we verify the module structure compiles.

- [ ] **Step 6: Commit**

```
feat(memory): add SqliteMemoryBackend skeleton with schema and sqlite-vec helpers
```

---

### Task 2: Implement MemoryStore (Facts CRUD + Search)

Implement the full `MemoryStore` trait for `SqliteMemoryBackend`. This is the largest task — covers fact CRUD, vector search via sqlite-vec, text search via FTS5, and hybrid search.

**Files:**
- Modify: `src/memory/store/sqlite/facts.rs`

**Context files to read first:**
- `src/memory/store/mod.rs` — full `MemoryStore` trait definition (lines 168-296)
- `src/memory/store/lance/facts/mod.rs` — existing LanceDB implementation (to understand behavior)
- `src/memory/store/lance/facts/helpers.rs` — helper functions
- `src/memory/context/mod.rs` — `MemoryFact` struct definition
- `src/memory/store/types.rs` — `SearchFilter`, `ScoredFact` types
- `src/memory/store/sqlite/vec.rs` — vector helpers (from Task 1)

- [ ] **Step 1: Implement `MemoryStore` for `SqliteMemoryBackend`**

Implement all methods from the `MemoryStore` trait. Key implementation notes:

1. **`insert_fact`** — INSERT into `facts` table + insert embedding into appropriate `facts_vec_{dim}` table + sync FTS5 index. Use the fact's embedding field and `embedding_model` to determine dimension. Get the rowid from `last_insert_rowid()`.

2. **`get_fact`** — SELECT by id, map row to `MemoryFact`.

3. **`update_fact`** — DELETE old row + INSERT new (same pattern as LanceDB impl). Delete old vec entry, insert new.

4. **`delete_fact`** — DELETE from `facts`, `facts_fts`, and all `facts_vec_*` tables.

5. **`batch_insert_facts`** — Use a transaction wrapping multiple insert_fact calls.

6. **`vector_search`** — Call `vec::knn_search()` to get `(rowid, distance)` pairs, then JOIN with `facts` to get full records. Apply `SearchFilter` post-query (namespace, agent, is_valid filters). Convert distance to similarity score.

7. **`text_search`** — Query `facts_fts` with `MATCH ?`, join with `facts`, apply filters.

8. **`hybrid_search`** — Run vector_search and text_search separately, merge results with weighted scoring (using `vector_weight` and `text_weight` from `HybridSearchParams`).

9. **VFS path operations** (`list_by_path`, `get_by_path`, `get_facts_by_path_prefix`) — SQL queries with `parent_path = ?` and `path LIKE ? || '%'`.

10. **`count_facts`** — `SELECT COUNT(*) FROM facts WHERE ...`

11. **`get_all_facts`** — `SELECT * FROM facts` with optional `is_valid` filter.

12. **`invalidate_fact`** — `UPDATE facts SET is_valid = 0, invalidation_reason = ?, decay_invalidated_at = ? WHERE id = ?`

13. **`apply_fact_decay`** — `UPDATE facts SET decay_score = decay_score * ? WHERE is_valid = 1` then invalidate those below min_score.

14. **`find_similar_facts`** — Same as vector_search but filter by distance threshold.

15. **MemoryFact ↔ row mapping** — Create helper functions `row_to_fact(row: &Row) -> MemoryFact` and `fact_to_params(fact: &MemoryFact)`. Handle `tags` and `source_memory_ids` as JSON arrays. Handle embedding as a field on MemoryFact — check which field holds the vector (likely `embedding: Option<Vec<f32>>`).

16. **SearchFilter mapping** — Convert `SearchFilter` fields to SQL WHERE clauses. Check `SearchFilter` struct in `types.rs` for available fields.

- [ ] **Step 2: Add unit tests**

Add `#[cfg(test)] mod tests` at bottom of `facts.rs` with tests for:
- insert + get round-trip
- update replaces content
- delete removes fact
- vector_search returns nearest neighbors
- text_search matches content
- invalidate_fact marks as invalid
- count_facts with filters

Use `SqliteMemoryBackend::new()` with a tempdir for test isolation.

- [ ] **Step 3: Verify**

Run: `cargo test -p alephcore --lib -- sqlite::facts`
Expected: All tests pass

- [ ] **Step 4: Commit**

```
feat(memory): implement MemoryStore for SqliteMemoryBackend

Full fact CRUD, vector search (sqlite-vec), text search (FTS5),
hybrid search, VFS path operations, decay, and statistics.
```

---

### Task 3: Implement GraphStore

Implement the `GraphStore` trait for `SqliteMemoryBackend`. Pure SQL — no vectors needed.

**Files:**
- Modify: `src/memory/store/sqlite/graph.rs`

**Context files to read first:**
- `src/memory/store/mod.rs` — `GraphStore` trait definition (lines 302-349)
- `src/memory/store/mod.rs` — `GraphNode`, `GraphEdge`, `ResolvedEntity`, `DecayStats` types
- `src/memory/store/lance/` — existing graph implementation (grep for `GraphStore`)
- `src/config/types/memory.rs` — `GraphDecayPolicy` struct

- [ ] **Step 1: Implement `GraphStore` for `SqliteMemoryBackend`**

1. **`upsert_node`** — `INSERT OR REPLACE INTO graph_nodes ...`
2. **`get_node`** — `SELECT * FROM graph_nodes WHERE id = ? AND agent = ?`
3. **`upsert_edge`** — `INSERT OR REPLACE INTO graph_edges ...`
4. **`resolve_entity`** — Query `graph_nodes` by name/aliases (case-insensitive LIKE match), join with `graph_edges` if context_key provided, return `ResolvedEntity` list. Handle `aliases` as JSON array.
5. **`get_edges_for_node`** — `SELECT * FROM graph_edges WHERE (from_id = ? OR to_id = ?) AND agent = ?` with optional context_key filter.
6. **`count_edges_in_context`** — `SELECT COUNT(*) FROM graph_edges WHERE (from_id = ? OR to_id = ?) AND context_key = ? AND agent = ?`
7. **`apply_decay`** — Apply `GraphDecayPolicy` decay_factor to all nodes/edges, prune those below threshold. Return `DecayStats`.

- [ ] **Step 2: Add unit tests**

Tests for: upsert_node + get_node round-trip, upsert_edge + get_edges, resolve_entity, apply_decay prunes low-score entries.

- [ ] **Step 3: Verify**

Run: `cargo test -p alephcore --lib -- sqlite::graph`

- [ ] **Step 4: Commit**

```
feat(memory): implement GraphStore for SqliteMemoryBackend
```

---

### Task 4: Remove SessionStore Trait and All Callers

Delete the `SessionStore` trait from `src/memory/store/mod.rs` and remove all references throughout the codebase. **Do NOT touch** `src/teams/sessions/store.rs` — that's a different `SessionStore` for team collaboration.

**Files to modify:**
- `src/memory/store/mod.rs` — Delete `SessionStore` trait (lines 355-430), `MemoryEntry` references, `MemoryFilter`, `StoreStats::total_memories`
- `src/memory/ingestion.rs` — Remove `insert_memory` calls
- `src/memory/retrieval.rs` — Remove `search_memories` and `get_memories_for_entity` fallback paths
- `src/memory/fact_retrieval.rs` — Remove `search_memories` fallback
- `src/memory/compression/service.rs` — Remove `get_uncompressed_memories` usage
- `src/memory/cleanup.rs` — Remove `delete_older_than` for memories
- `src/memory/reembed.rs` — Remove `get_all_memories`
- `src/gateway/handlers/memory.rs` — Remove `get_recent_memories` endpoint
- `src/thinker/memory_context_provider.rs` — Remove `search_memories`
- `src/capability/strategies/memory.rs` — Remove `SessionStore` import

**Context:** Before modifying each file, read it to understand the full context. The `search_memories` is used as a fallback when facts are insufficient — just remove the fallback branch and let the facts-only path handle all retrieval.

- [ ] **Step 1: Delete `SessionStore` trait from `src/memory/store/mod.rs`**

Remove the trait definition (lines 355-430) and any associated imports/types that are only used by SessionStore. Keep `MemoryEntry` if it's used elsewhere (check first). Set `StoreStats::total_memories` to always 0 or remove the field.

- [ ] **Step 2: Remove SessionStore from each caller file**

For each file listed above:
1. Read the file
2. Remove `SessionStore` imports
3. Remove `search_memories` / `insert_memory` / `get_recent_memories` / etc. call sites
4. For retrieval fallback paths: remove the `if facts_insufficient { search_memories }` branches, keep only the facts-first path
5. For compression service: remove `get_uncompressed_memories` — compression now only works with facts already in the store (this is fine; raw memories are in SessionManager's SQLite, compression can be adapted later if needed)

- [ ] **Step 3: Fix compilation**

Run: `cargo check -p alephcore`

This will likely surface additional files that reference `SessionStore` or `MemoryEntry` via the memory store. Fix each one.

- [ ] **Step 4: Commit**

```
refactor(memory): remove SessionStore trait and all callers

Raw memory storage (Layer 1) is no longer needed — facts are the
primary retrieval target, and raw conversations are already stored
in SessionManager's SQLite. Removes ~10 files' worth of fallback
retrieval, memory ingestion, and cleanup code.
```

---

### Task 5: Wire SqliteMemoryBackend into Server Startup

Replace `LanceMemoryBackend` with `SqliteMemoryBackend` in the server initialization code.

**Files:**
- Modify: `src/memory/store/mod.rs` — Change re-export from `lance::LanceMemoryBackend` to `sqlite::SqliteMemoryBackend`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init.rs` — Replace LanceMemoryBackend construction

**Context files to read first:**
- `src/bin/aleph-server/commands/start/builder/agent_init.rs` — find where `LanceMemoryBackend` is constructed (grep for `LanceMemoryBackend::new` or `memory::store::lance`)
- `src/memory/store/mod.rs` — current `pub use lance::LanceMemoryBackend`

- [ ] **Step 1: Update `src/memory/store/mod.rs`**

Change:
```rust
pub mod lance;
pub use lance::LanceMemoryBackend;
```
To:
```rust
pub mod sqlite;
pub use sqlite::SqliteMemoryBackend;
```

Remove the `lance` module declaration entirely.

- [ ] **Step 2: Update agent_init.rs**

Find where `LanceMemoryBackend` is constructed and replace with `SqliteMemoryBackend::new()`. The DB path should be `~/.aleph/data/memory.db`.

Also add a startup notice if old `memory.lance` directory exists:
```rust
let lance_path = data_dir.join("memory.lance");
if lance_path.exists() {
    println!("  Note: Old LanceDB data found at {:?}. You can safely delete it: rm -rf {:?}", lance_path, lance_path);
}
```

- [ ] **Step 3: Fix all remaining compilation errors**

Run `cargo check -p alephcore` and `cargo check --bin aleph-server`. Fix any remaining references to `LanceMemoryBackend`, `lance::*`, arrow types, etc.

- [ ] **Step 4: Commit**

```
refactor(memory): wire SqliteMemoryBackend into server startup

Server now uses SQLite + sqlite-vec for memory storage instead of
LanceDB. DB file at ~/.aleph/data/memory.db.
```

---

### Task 6: Delete LanceDB Code and Dependencies

Remove all LanceDB-related code and Cargo dependencies.

**Files:**
- Delete: `src/memory/store/lance/` — entire directory
- Delete: `src/memory/store/lance/arrow_convert/` — Arrow conversion code
- Modify: `Cargo.toml` — remove lancedb, lance-index, arrow-array, arrow-schema

- [ ] **Step 1: Delete `src/memory/store/lance/` directory**

Remove the entire directory:
- `src/memory/store/lance/mod.rs`
- `src/memory/store/lance/schema.rs`
- `src/memory/store/lance/facts/mod.rs`
- `src/memory/store/lance/facts/helpers.rs`
- `src/memory/store/lance/facts/tests.rs`
- `src/memory/store/lance/sessions.rs`
- `src/memory/store/lance/integration_tests.rs`
- `src/memory/store/lance/arrow_convert/mod.rs`
- `src/memory/store/lance/arrow_convert/memory.rs`
- `src/memory/store/lance/arrow_convert/fact.rs`

- [ ] **Step 2: Remove LanceDB dependencies from Cargo.toml**

Remove these lines:
```toml
lancedb = "0.26"
lance-index = "2.0"
arrow-array = "57"
arrow-schema = "57"
```

- [ ] **Step 3: Search and fix any remaining LanceDB references**

```bash
grep -r "lancedb\|lance_index\|LanceMemoryBackend\|arrow_array\|arrow_schema\|arrow_convert" src/ --include="*.rs" -l
```

Fix any remaining files that still reference LanceDB or Arrow types.

- [ ] **Step 4: Full compilation check**

Run: `cargo check -p alephcore`
Run: `cargo check --bin aleph-server`
Expected: Clean compilation with no LanceDB references

- [ ] **Step 5: Commit**

```
chore(deps): remove LanceDB and Arrow dependencies

Deletes src/memory/store/lance/ (entire directory), removes lancedb,
lance-index, arrow-array, arrow-schema from Cargo.toml. All memory
storage now uses SQLite + sqlite-vec.
```

---

### Task 7: Build, Test, Verify

Full build, restart, measure memory, functional verification.

**Files:** None (verification only)

- [ ] **Step 1: Run all tests**

```bash
cargo test -p alephcore --lib
```

Fix any failures.

- [ ] **Step 2: Full release build**

```bash
just build
```

- [ ] **Step 3: Restart and measure memory**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
nohup target/release/aleph-server start > /tmp/aleph-server.log 2>&1 &
sleep 10
PID=$(pgrep -f "target/release/aleph-server" | head -1)
vmmap --summary $PID 2>/dev/null | grep "Physical footprint"
```

Expected: Physical footprint < 50MB (down from ~400MB)

- [ ] **Step 4: Delete old LanceDB data**

```bash
rm -rf ~/.aleph/data/memory.lance
```

Verify disk savings (was 2.1GB).

- [ ] **Step 5: Functional verification**

Send a test message through the system to verify:
- Facts can be stored and retrieved
- Vector search returns results
- Graph nodes/edges work
- No crashes or errors in logs

- [ ] **Step 6: Final commit if any fixes needed**

```
fix(memory): address issues found during verification
```
