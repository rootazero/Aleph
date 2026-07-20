# Memory Layer Restructuring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure flat Knowledge Notes into a `memory/{agent_id}/{category}/` hierarchy with embedding index, Obsidian-compatible wikilink resolution, complete read path for LLM injection, wiki module merger, and default compression switch.

**Architecture:** The existing NoteStore/NoteIndexer (flat dir, title-as-PK) is refactored to use relative paths (`category/filename`) as primary keys within agent-scoped directories. Embedding vectors are indexed alongside notes for semantic retrieval. The compression pipeline defaults to writing notes. The standalone wiki module is absorbed.

**Tech Stack:** Rust, SQLite (rusqlite, sqlite-vec, FTS5), Leptos (WASM), tokio

**Spec:** `docs/superpowers/specs/2026-04-11-memory-layer-restructuring-design.md`

---

## File Structure

### Modified Files

| File | Changes |
|------|---------|
| `src/memory/notes/store.rs` | NoteIndexEntry gains path/filename/agent_id; NoteStore trait signatures use path + agent_id; add find_by_filename, upsert_embedding, vector_search |
| `src/memory/store/sqlite/notes.rs` | SQLite impl updated for new schema + embedding + vector search |
| `src/memory/store/sqlite/schema.rs` | Update notes_index DDL (add path/filename/agent_id/category columns), add notes_vec DDL |
| `src/memory/notes/indexer.rs` | Scan {agent_id}/{category}/ dirs, ensure_dirs(), path-based write/append/rename |
| `src/memory/notes/note.rs` | KnowledgeNote gains optional agent_id field for context |
| `src/memory/notes/extractor.rs` | NoteUpdate: note_title → note_path; prompt includes category list |
| `src/memory/notes/wikilink.rs` | Add resolve_wikilink() function |
| `src/memory/notes/migration.rs` | Update to write into category subdirs |
| `src/memory/compression/service.rs` | Update compress_to_notes for path-based notes |
| `src/memory/compression/extractor.rs` | Update extract_note_updates prompt |
| `src/memory/retrieval.rs` | Delegate to NoteRetrieval |
| `src/gateway/handlers/graph.rs` | Use path as node ID, filename as display name, add agent_id |
| `src/gateway/handlers/graph_types.rs` | NoteNodeDto gains path field |
| `interfaces/webchat/src/canvas_engine/adapter.rs` | Adapt for path-based nodes |
| `src/memory/notes/mod.rs` | Add new re-exports |

### New Files

| File | Responsibility |
|------|---------------|
| `src/memory/notes/retrieval.rs` | NoteRetrieval — embedding-based note retrieval for LLM injection |

### Files to Remove (Phase 7)

| File | Reason |
|------|--------|
| `src/wiki/mod.rs` | Absorbed into memory/notes |
| `src/wiki/tools.rs` | WikiManageTool rewritten to use NoteIndexer |
| `src/wiki/wikilink.rs` | Covered by memory/notes/wikilink.rs |
| `src/wiki/git.rs` | Optional, defer |
| `src/wiki/index.rs` | Optional, defer |

---

## Phase 1: Schema + NoteStore Restructure

### Task 1: Update NoteIndexEntry and NoteStore trait

**Files:**
- Modify: `src/memory/notes/store.rs`

- [ ] **Step 1: Update NoteIndexEntry struct**

Replace the current struct with:

```rust
#[derive(Debug, Clone)]
pub struct NoteIndexEntry {
    /// Relative path within agent dir: "wiki/rust-ownership"
    pub path: String,
    /// Filename without .md: "rust-ownership"
    pub filename: String,
    /// Agent ID: "default"
    pub agent_id: String,
    /// Category (FactType mapping): "wiki"
    pub category: String,
    /// Searchable tags
    pub tags: Vec<String>,
    /// Number of outgoing wikilinks
    pub link_count: usize,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
    /// SHA-256 of file content
    pub content_hash: String,
}
```

- [ ] **Step 2: Update NoteStore trait signatures**

Replace the current trait with:

```rust
#[async_trait]
pub trait NoteStore: Send + Sync {
    async fn index_note(&self, note: &KnowledgeNote, agent_id: &str, category: &str) -> Result<(), AlephError>;
    async fn remove_note_index(&self, path: &str, agent_id: &str) -> Result<(), AlephError>;
    async fn get_note_index(&self, path: &str, agent_id: &str) -> Result<Option<NoteIndexEntry>, AlephError>;
    async fn list_notes(&self, agent_id: &str) -> Result<Vec<NoteIndexEntry>, AlephError>;
    async fn get_outgoing_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>, AlephError>;
    async fn get_incoming_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>, AlephError>;
    async fn search_notes_fts(&self, query: &str, agent_id: &str, limit: usize) -> Result<Vec<NoteIndexEntry>, AlephError>;
    async fn get_graph_data(&self, agent_id: &str, limit: usize) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;
    async fn get_neighbors(&self, center: &str, agent_id: &str, depth: u8, limit: usize) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;

    // New methods
    async fn find_by_filename(&self, filename: &str, agent_id: &str) -> Result<Vec<String>, AlephError>;
    async fn upsert_embedding(&self, path: &str, agent_id: &str, embedding: &[f32], dim: u32) -> Result<(), AlephError>;
    async fn vector_search(&self, embedding: &[f32], dim: u32, agent_id: &str, limit: usize) -> Result<Vec<(String, f32)>, AlephError>;
}
```

- [ ] **Step 3: Update tests**

Update `sample_note` helper and tests to use new signatures:

```rust
fn sample_note(title: &str, category: &str, links: Vec<&str>) -> KnowledgeNote {
    KnowledgeNote {
        title: title.to_string(),
        category: category.to_string(),
        tags: vec!["test".to_string()],
        facts: vec!["A test fact".to_string()],
        links: links.into_iter().map(|s| s.to_string()).collect(),
        created_at: 1_700_000_000,
        updated_at: 1_700_001_000,
        content_hash: format!("hash_{title}"),
    }
}

#[tokio::test]
async fn indexes_and_retrieves_note() {
    let db = create_test_db();
    let note = sample_note("editor", "preference", vec!["wiki/vim"]);

    db.index_note(&note, "default", "preference").await.unwrap();

    let entry = db
        .get_note_index("preference/editor", "default")
        .await
        .unwrap()
        .expect("should exist");

    assert_eq!(entry.path, "preference/editor");
    assert_eq!(entry.filename, "editor");
    assert_eq!(entry.agent_id, "default");
    assert_eq!(entry.category, "preference");
}

#[tokio::test]
async fn stores_and_queries_links() {
    let db = create_test_db();
    let note_a = sample_note("rust", "learning", vec!["tool/cargo"]);
    let note_b = sample_note("cargo", "tool", vec!["learning/rust"]);

    db.index_note(&note_a, "default", "learning").await.unwrap();
    db.index_note(&note_b, "default", "tool").await.unwrap();

    let out = db.get_outgoing_links("learning/rust", "default").await.unwrap();
    assert!(out.contains(&"tool/cargo".to_string()));

    let inc = db.get_incoming_links("learning/rust", "default").await.unwrap();
    assert!(inc.contains(&"tool/cargo".to_string()));
}

#[tokio::test]
async fn find_by_filename_resolves_unique() {
    let db = create_test_db();
    let note = sample_note("editor", "preference", vec![]);
    db.index_note(&note, "default", "preference").await.unwrap();

    let paths = db.find_by_filename("editor", "default").await.unwrap();
    assert_eq!(paths, vec!["preference/editor"]);
}
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: FAIL — SQLite impl doesn't match new trait yet. That's Task 2.

- [ ] **Step 5: Commit**

```bash
git add src/memory/notes/store.rs
git commit -m "refactor(notes): update NoteStore trait for path-based memory/{agent_id}/{category}/ structure"
```

---

### Task 2: Update SQLite schema + NoteStore implementation

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/notes.rs`

- [ ] **Step 1: Update notes DDL in schema.rs**

Replace the existing `notes_index`, `notes_links`, `notes_fts` DDL with:

```rust
pub(crate) const NOTES_INDEX_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_index (
    path            TEXT NOT NULL,
    filename        TEXT NOT NULL,
    agent_id        TEXT NOT NULL DEFAULT 'default',
    category        TEXT NOT NULL,
    tags_json       TEXT NOT NULL DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash    TEXT NOT NULL,
    PRIMARY KEY (agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_notes_filename ON notes_index(filename);
CREATE INDEX IF NOT EXISTS idx_notes_agent ON notes_index(agent_id);
CREATE INDEX IF NOT EXISTS idx_notes_category ON notes_index(category);
";

pub(crate) const NOTES_LINKS_DDL: &str = "
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
";

pub(crate) const NOTES_FTS_DDL: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path,
    filename,
    content,
    agent_id UNINDEXED,
    tokenize='unicode61'
);
";
```

- [ ] **Step 2: Rewrite SQLite NoteStore implementation**

In `src/memory/store/sqlite/notes.rs`, update `row_to_entry` and all trait methods to use the new schema. Key changes:

- `index_note` receives `agent_id` and `category`, computes `path = format!("{category}/{note.title}")` and `filename = note.title`
- All queries filter by `agent_id`
- Links stored with `agent_id` scope
- `find_by_filename`: `SELECT path FROM notes_index WHERE filename = ?1 AND agent_id = ?2`
- `upsert_embedding` and `vector_search`: placeholder implementations that return `Ok(())` and `Ok(vec![])` — real embedding wiring is Task 4

```rust
fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<NoteIndexEntry> {
    let tags_json: String = row.get("tags_json")?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    let link_count: i64 = row.get("link_count")?;

    Ok(NoteIndexEntry {
        path: row.get("path")?,
        filename: row.get("filename")?,
        agent_id: row.get("agent_id")?,
        category: row.get("category")?,
        tags,
        link_count: link_count.max(0) as usize,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        content_hash: row.get("content_hash")?,
    })
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib notes -- --nocapture`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/schema.rs src/memory/store/sqlite/notes.rs
git commit -m "refactor(notes): update SQLite schema and impl for path-based agent-scoped notes"
```

---

## Phase 2: NoteIndexer Restructure

### Task 3: Update NoteIndexer for directory hierarchy

**Files:**
- Modify: `src/memory/notes/indexer.rs`

- [ ] **Step 1: Add ensure_dirs and update constructor**

```rust
/// The 15 FactType category directory names.
pub const CATEGORY_DIRS: &[&str] = &[
    "preference", "plan", "learning", "project", "personal",
    "tool", "lesson", "skill", "wiki", "transcript",
    "subagent-run", "subagent-session", "subagent-checkpoint",
    "subagent-transcript", "other",
];

impl<S: NoteStore> NoteIndexer<S> {
    /// memory_dir is ~/.aleph/data/memory/
    pub fn new(memory_dir: PathBuf, store: Arc<S>) -> Self { ... }

    /// Create all 15 category subdirs under memory_dir/{agent_id}/
    pub async fn ensure_dirs(&self, agent_id: &str) -> Result<(), AlephError> {
        let agent_dir = self.memory_dir.join(agent_id);
        for cat in CATEGORY_DIRS {
            fs::create_dir_all(agent_dir.join(cat)).await.map_err(|e| {
                AlephError::ConfigError {
                    message: format!("Failed to create {}/{cat}: {e}", agent_dir.display()),
                    suggestion: None,
                }
            })?;
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Update full_rebuild to scan {agent_id}/{category}/**

```rust
pub async fn full_rebuild(&self, agent_id: &str) -> Result<IndexStats, AlephError> {
    self.ensure_dirs(agent_id).await?;
    let mut stats = IndexStats::default();
    let agent_dir = self.memory_dir.join(agent_id);

    for category in CATEGORY_DIRS {
        let cat_dir = agent_dir.join(category);
        let mut entries = match fs::read_dir(&cat_dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            match self.index_file(agent_id, category, &path).await {
                Ok(true) => stats.indexed += 1,
                Ok(false) => stats.skipped += 1,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Failed to index note");
                    stats.errors += 1;
                }
            }
        }
    }
    Ok(stats)
}
```

- [ ] **Step 3: Update index_file, write_note, append_to_note, rename_note**

All methods now take `agent_id` and derive `category` from the path. Key signature changes:

```rust
pub async fn index_file(&self, agent_id: &str, category: &str, path: &Path) -> Result<bool, AlephError>;

pub async fn write_note(&self, agent_id: &str, category: &str, note: &KnowledgeNote) -> Result<PathBuf, AlephError> {
    let safe_title = sanitize_title(&note.title);
    let path = self.memory_dir.join(agent_id).join(category).join(format!("{safe_title}.md"));
    // ... write file, index with store.index_note(note, agent_id, category)
}

pub async fn append_to_note(&self, agent_id: &str, note_path: &str, new_facts: &[String], new_links: &[String]) -> Result<(), AlephError> {
    // note_path = "preference/editor"
    // Split into category + filename
    let (category, filename) = split_note_path(note_path)?;
    let file_path = self.memory_dir.join(agent_id).join(format!("{note_path}.md"));
    // ... read, parse, append, write, index
}
```

Helper:
```rust
fn split_note_path(note_path: &str) -> Result<(&str, &str), AlephError> {
    note_path.split_once('/').ok_or_else(|| AlephError::ConfigError {
        message: format!("Invalid note path (must be category/filename): {note_path}"),
        suggestion: None,
    })
}
```

- [ ] **Step 4: Update tests**

Update all tests to use `agent_id` and `category` parameters. Write to `{tempdir}/default/preference/*.md` etc.

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib notes -- --nocapture`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/indexer.rs
git commit -m "refactor(notes): update NoteIndexer for memory/{agent_id}/{category}/ hierarchy"
```

---

### Task 4: Wikilink resolution

**Files:**
- Modify: `src/memory/notes/wikilink.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Add resolve_wikilink function**

```rust
use crate::memory::notes::store::NoteStore;

/// Resolve a wikilink target to a note path using Obsidian-compatible rules.
///
/// 1. If link contains '/' → exact path match
/// 2. If no '/' → search by filename across all categories
/// 3. If exactly one match → return it
/// 4. If ambiguous or not found → return None
pub async fn resolve_wikilink<S: NoteStore>(
    store: &S,
    link: &str,
    agent_id: &str,
) -> Option<String> {
    // 1. Exact path match (link contains '/')
    if link.contains('/') {
        if store.get_note_index(link, agent_id).await.ok()?.is_some() {
            return Some(link.to_string());
        }
        return None;
    }

    // 2. Global filename search
    let matches = store.find_by_filename(link, agent_id).await.ok()?;
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }

    None // Ambiguous or not found
}
```

- [ ] **Step 2: Add test**

```rust
#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::memory::store::SqliteMemoryBackend;
    use std::sync::Arc;

    #[tokio::test]
    async fn resolves_exact_path() {
        let db = /* create_test_db + index a note at "wiki/rust" */;
        let result = resolve_wikilink(&*db, "wiki/rust", "default").await;
        assert_eq!(result, Some("wiki/rust".to_string()));
    }

    #[tokio::test]
    async fn resolves_unique_filename() {
        let db = /* create_test_db + index a note at "wiki/rust" */;
        let result = resolve_wikilink(&*db, "rust", "default").await;
        assert_eq!(result, Some("wiki/rust".to_string()));
    }

    #[tokio::test]
    async fn returns_none_for_ambiguous() {
        let db = /* create_test_db + index "wiki/rust" AND "learning/rust" */;
        let result = resolve_wikilink(&*db, "rust", "default").await;
        assert_eq!(result, None);
    }
}
```

- [ ] **Step 3: Export from mod.rs**

Add `pub use wikilink::resolve_wikilink;` to `src/memory/notes/mod.rs`.

- [ ] **Step 4: Run tests, commit**

```bash
cargo test -p alephcore --lib notes::wikilink -- --nocapture
git add src/memory/notes/wikilink.rs src/memory/notes/mod.rs
git commit -m "feat(notes): add Obsidian-compatible wikilink resolution (exact path + global filename)"
```

---

## Phase 3: Embedding Index

### Task 5: Add embedding storage and vector search

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`
- Modify: `src/memory/store/sqlite/notes.rs`

- [ ] **Step 1: Add notes_vec DDL**

In schema.rs, add embedding table creation. Look at how existing `facts_vec_{dim}` tables are created — follow the same pattern with sqlite-vec `vec0`:

```rust
pub(crate) fn create_notes_vec_table(conn: &rusqlite::Connection, dim: u32) -> Result<(), AlephError> {
    let sql = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS notes_vec_{dim} USING vec0(
            path TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            embedding float[{dim}]
        )"
    );
    conn.execute_batch(&sql).map_err(|e| AlephError::config(format!("notes_vec DDL: {e}")))?;
    Ok(())
}
```

- [ ] **Step 2: Implement upsert_embedding and vector_search**

Replace the placeholder implementations in `sqlite/notes.rs`:

```rust
async fn upsert_embedding(&self, path: &str, agent_id: &str, embedding: &[f32], dim: u32) -> Result<(), AlephError> {
    let conn = lock_conn!(self)?;
    let table = format!("notes_vec_{dim}");

    // Ensure table exists
    create_notes_vec_table(&conn, dim)?;

    // Delete existing, insert new
    conn.execute(
        &format!("DELETE FROM {table} WHERE path = ?1 AND agent_id = ?2"),
        params![path, agent_id],
    ).map_err(|e| AlephError::config(format!("upsert_embedding delete: {e}")))?;

    // Insert with vec blob
    let blob = embedding.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    conn.execute(
        &format!("INSERT INTO {table} (path, agent_id, embedding) VALUES (?1, ?2, ?3)"),
        params![path, agent_id, blob],
    ).map_err(|e| AlephError::config(format!("upsert_embedding insert: {e}")))?;

    Ok(())
}

async fn vector_search(&self, embedding: &[f32], dim: u32, agent_id: &str, limit: usize) -> Result<Vec<(String, f32)>, AlephError> {
    let conn = lock_conn!(self)?;
    let table = format!("notes_vec_{dim}");

    let blob = embedding.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();

    let mut stmt = conn.prepare(&format!(
        "SELECT path, distance FROM {table} WHERE embedding MATCH ?1 AND agent_id = ?2 ORDER BY distance LIMIT ?3"
    )).map_err(|e| AlephError::config(format!("vector_search prepare: {e}")))?;

    let rows = stmt.query_map(params![blob, agent_id, limit as i64], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
    }).map_err(|e| AlephError::config(format!("vector_search query: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| AlephError::config(format!("vector_search row: {e}")))?);
    }
    Ok(results)
}
```

IMPORTANT: Read how the existing `facts_vec_{dim}` tables handle embedding blobs — the exact sqlite-vec API (MATCH syntax, blob format) may differ. Match the existing pattern exactly.

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/schema.rs src/memory/store/sqlite/notes.rs
git commit -m "feat(notes): add embedding vector index (notes_vec) with upsert and search"
```

---

## Phase 4: Read Path

### Task 6: NoteRetrieval service

**Files:**
- Create: `src/memory/notes/retrieval.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Implement NoteRetrieval**

```rust
// src/memory/notes/retrieval.rs
use std::path::PathBuf;

use crate::error::AlephError;
use crate::memory::notes::store::NoteStore;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

/// A retrieved note with its content and relevance score.
#[derive(Debug, Clone)]
pub struct NoteContent {
    pub path: String,
    pub content: String,
    pub score: f32,
}

/// Note-based memory retrieval for LLM injection.
///
/// Embeds the query, searches notes_vec for nearest neighbors,
/// reads markdown files, and returns content for prompt injection.
pub struct NoteRetrieval<S: NoteStore> {
    memory_dir: PathBuf,
    store: Arc<S>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl<S: NoteStore> NoteRetrieval<S> {
    pub fn new(
        memory_dir: PathBuf,
        store: Arc<S>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self { memory_dir, store, embedder }
    }

    /// Retrieve relevant notes for the given query.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<NoteContent>, AlephError> {
        // 1. Embed the query
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        // 2. Vector search
        let results = self.store.vector_search(&embedding, dim, agent_id, limit).await?;

        // 3. Read markdown files
        let mut notes = Vec::new();
        for (path, score) in results {
            let file_path = self.memory_dir
                .join(agent_id)
                .join(format!("{path}.md"));
            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(c) => c,
                Err(_) => continue, // Skip if file missing
            };
            notes.push(NoteContent { path, content, score });
        }

        Ok(notes)
    }
}
```

- [ ] **Step 2: Export from mod.rs**

Add to `src/memory/notes/mod.rs`:
```rust
pub mod retrieval;
pub use retrieval::{NoteRetrieval, NoteContent};
```

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/notes/retrieval.rs src/memory/notes/mod.rs
git commit -m "feat(notes): add NoteRetrieval service for embedding-based memory injection"
```

---

### Task 7: Wire NoteRetrieval into MemoryRetrieval

**Files:**
- Modify: `src/memory/retrieval.rs`

- [ ] **Step 1: Read the existing retrieval.rs**

Read `src/memory/retrieval.rs` to understand how `FactRetrieval` is currently delegated to.

- [ ] **Step 2: Add note-based retrieval path**

Add a new method or modify `retrieve_memories` to try NoteRetrieval first, falling back to FactRetrieval if no notes are indexed:

```rust
use crate::memory::notes::{NoteRetrieval, NoteContent};

impl MemoryRetrieval {
    pub async fn retrieve_memories(&self, context: &ContextAnchor, query: &str) -> Result<Vec<MemoryEntry>, AlephError> {
        record_activity();
        if !self.config.enabled {
            return Ok(Vec::new());
        }

        // Try note-based retrieval first
        let memory_dir = crate::utils::paths::get_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("aleph_data"))
            .join("memory");

        let note_retrieval = NoteRetrieval::new(
            memory_dir,
            self.database.clone(),
            self.embedder.clone(),
        );

        let notes = note_retrieval.retrieve(query, "default", self.config.max_facts_per_query.unwrap_or(5) as usize).await?;

        if !notes.is_empty() {
            return Ok(notes.into_iter().map(note_to_entry).collect());
        }

        // Fallback to fact-based retrieval
        let fact_retrieval = FactRetrieval::with_defaults(self.database.clone(), self.embedder.clone());
        let result = fact_retrieval.retrieve(query).await?;
        Ok(result.facts.into_iter().map(fact_to_entry).collect())
    }
}

fn note_to_entry(note: NoteContent) -> MemoryEntry {
    MemoryEntry {
        id: note.path.clone(),
        context: ContextAnchor::now(String::new()),
        user_input: note.content,
        ai_output: String::new(),
        embedding: None,
        namespace: "owner".to_string(),
        agent: "default".to_string(),
        similarity_score: Some(note.score),
    }
}
```

IMPORTANT: Read the actual `MemoryRetrieval` struct to confirm field names. The `config` may not have `max_facts_per_query` — adapt to the real field. Check the `MemoryConfig` struct.

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/retrieval.rs
git commit -m "feat(notes): wire NoteRetrieval into MemoryRetrieval with FactRetrieval fallback"
```

---

## Phase 5: Extraction + Compression Switch

### Task 8: Update extraction prompt and NoteUpdate for paths

**Files:**
- Modify: `src/memory/notes/extractor.rs`

- [ ] **Step 1: Update NoteUpdate struct**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteUpdate {
    /// Path within agent dir: "preference/editor"
    pub note_path: String,
    pub action: NoteAction,
    #[serde(default)]
    pub new_facts: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}
```

Remove the `category` field — it's derived from the first segment of `note_path`.

- [ ] **Step 2: Update extraction prompt**

Update `build_note_extraction_prompt` to include the category list and instruct LLM to use `note_path`:

```rust
pub fn build_note_extraction_prompt(existing_titles: &[String]) -> String {
    // ... existing logic, but change the output format to use note_path
    // and add the CATEGORIES list:
    // "CATEGORIES (use as path prefix):
    //  preference, plan, learning, project, personal, tool, lesson,
    //  skill, wiki, transcript, other
    //
    //  note_path format: category/filename (e.g., preference/editor, wiki/rust-ownership)"
}
```

- [ ] **Step 3: Update tests**

Update `parses_extraction_response` test to use `note_path` instead of `note_title`.

- [ ] **Step 4: Run tests, commit**

```bash
cargo test -p alephcore --lib notes::extractor -- --nocapture
git add src/memory/notes/extractor.rs
git commit -m "refactor(notes): update NoteUpdate to use note_path (category/filename)"
```

---

### Task 9: Update CompressionService for path-based notes

**Files:**
- Modify: `src/memory/compression/service.rs`
- Modify: `src/memory/compression/extractor.rs`

- [ ] **Step 1: Update compress_to_notes method**

Read the existing `compress_to_notes` at line ~410. Update it to:
- Call `indexer.ensure_dirs(workspace_id)` at the start
- Use `update.note_path` instead of `update.note_title`
- Split `note_path` into `(category, filename)` for write operations
- Generate embedding after writing and call `store.upsert_embedding()`

Key changes:
```rust
for update in &note_updates.updates {
    let (category, filename) = match update.note_path.split_once('/') {
        Some((c, f)) => (c, f),
        None => {
            tracing::warn!(path = %update.note_path, "Invalid note_path, skipping");
            continue;
        }
    };

    match update.action {
        NoteAction::Create => {
            let note = KnowledgeNote {
                title: filename.to_string(),
                category: category.to_string(),
                // ... rest of fields
            };
            indexer.write_note(workspace_id, category, &note).await?;

            // Generate and store embedding
            if let Ok(embedding) = self.extractor.embedder().embed(&note.body_text()).await {
                let dim = embedding.len() as u32;
                let _ = indexer.store().upsert_embedding(&update.note_path, workspace_id, &embedding, dim).await;
            }
        }
        NoteAction::Append | NoteAction::Update => {
            indexer.append_to_note(workspace_id, &update.note_path, &update.new_facts, &update.links).await?;

            // Re-embed the updated note
            let file_path = indexer.memory_dir().join(workspace_id).join(format!("{}.md", update.note_path));
            if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
                if let Ok(note) = KnowledgeNote::from_markdown(filename, &content) {
                    if let Ok(embedding) = self.extractor.embedder().embed(&note.body_text()).await {
                        let dim = embedding.len() as u32;
                        let _ = indexer.store().upsert_embedding(&update.note_path, workspace_id, &embedding, dim).await;
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Update extract_note_updates in extractor.rs**

The method should pass the updated prompt from Task 8.

- [ ] **Step 3: Run cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/memory/compression/service.rs src/memory/compression/extractor.rs
git commit -m "feat(notes): update compression pipeline for path-based notes with embedding"
```

---

### Task 10: Switch default compression to notes

**Files:**
- Modify: Where the Dream pipeline calls `compress_in_workspace()` — search for call sites

- [ ] **Step 1: Find all callers of compress_in_workspace**

```bash
cargo grep "compress_in_workspace\|compress()" --include "*.rs" | grep -v test
```

- [ ] **Step 2: Switch each call site to compress_to_notes**

Each call site needs a `NoteIndexer` instance. Create it from the memory_dir and database:

```rust
let memory_dir = crate::utils::paths::get_data_dir()?.join("memory");
let indexer = NoteIndexer::new(memory_dir, database.clone());
service.compress_to_notes(workspace_id, &indexer).await?;
```

- [ ] **Step 3: Run cargo check + test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(notes): switch Dream pipeline default compression to notes"
```

---

## Phase 6: Wiki Merger + Data Migration

### Task 11: Merge wiki module into memory/notes

**Files:**
- Modify: `src/wiki/tools.rs` — rewrite WikiManageTool to use NoteIndexer
- Modify: `src/memory/notes/migration.rs` — update for category subdirs
- Remove: `src/wiki/wikilink.rs`, `src/wiki/index.rs` (covered by memory/notes)

- [ ] **Step 1: Update WikiManageTool**

Read `src/wiki/tools.rs`. Rewrite the tool's create/update/delete operations to call `NoteIndexer` methods targeting `memory/{agent_id}/wiki/` instead of the old `data/wiki/{agent_id}/` path.

- [ ] **Step 2: Update migration.rs**

Update `migrate_facts_to_notes` to:
- Map `fact_type` → category directory name
- Use `indexer.write_note(agent_id, category, &note)` instead of flat write
- Add a `migrate_wiki_files` function:

```rust
pub async fn migrate_wiki_files(
    old_wiki_dir: &Path,    // ~/.aleph/data/wiki/{agent_id}/
    indexer: &NoteIndexer<impl NoteStore>,
    agent_id: &str,
) -> Result<usize, AlephError> {
    let mut migrated = 0;
    let mut entries = fs::read_dir(old_wiki_dir).await?;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let dest = indexer.memory_dir()
            .join(agent_id)
            .join("wiki")
            .join(path.file_name().unwrap());
        fs::copy(&path, &dest).await?;
        indexer.index_file(agent_id, "wiki", &dest).await?;
        migrated += 1;
    }
    Ok(migrated)
}
```

- [ ] **Step 3: Remove redundant wiki submodules**

Delete `src/wiki/wikilink.rs` and `src/wiki/index.rs` (if still present). Keep `src/wiki/mod.rs` and `src/wiki/tools.rs` (rewritten) and `src/wiki/git.rs` (optional, can keep for now).

- [ ] **Step 4: Run cargo check + tests**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(notes): merge wiki module into memory/notes, update migration for category dirs"
```

---

## Phase 7: Cleanup + Canvas Update

### Task 12: Update gateway handlers and Canvas for path-based nodes

**Files:**
- Modify: `src/gateway/handlers/graph.rs`
- Modify: `src/gateway/handlers/graph_types.rs`
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs`

- [ ] **Step 1: Update graph_types.rs**

Add `path` field to `NoteNodeDto`:

```rust
#[derive(Debug, Serialize)]
pub struct NoteNodeDto {
    pub id: String,         // path: "wiki/rust-ownership"
    pub name: String,       // display: "rust-ownership"
    pub path: String,       // full path for linking
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
}
```

- [ ] **Step 2: Update graph.rs handlers**

All `_impl` handlers now pass `agent_id` (default to `"default"` from request params or header):

```rust
fn entry_to_dto(entry: &NoteIndexEntry) -> NoteNodeDto {
    NoteNodeDto {
        id: entry.path.clone(),
        name: entry.filename.clone(),
        path: entry.path.clone(),
        category: entry.category.clone(),
        tags: entry.tags.clone(),
        link_count: entry.link_count,
    }
}
```

Update `handle_query_impl` to call `db.get_graph_data("default", params.limit)`.
Update `notes_dir()` to use `get_data_dir().join("memory")`.
Update `handle_node_detail_impl` to build path: `memory_dir/{agent_id}/{path}.md`.

- [ ] **Step 3: Update frontend adapter**

In `interfaces/webchat/src/canvas_engine/adapter.rs`, add `path` field to `NoteNodeDto`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NoteNodeDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub link_count: usize,
}
```

Node display name stays `dto.name` (filename portion).

- [ ] **Step 4: Build check**

Run: `cargo check -p alephcore && cargo check -p aleph-panel`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/ interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "refactor(canvas): update handlers and frontend for path-based note nodes"
```

---

### Task 13: Deprecate facts table usage

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs` — stop creating facts/facts_fts/facts_vec tables for new DBs

- [ ] **Step 1: Comment out facts DDL from init_schema**

Do NOT drop existing tables. Just stop executing the DDL for new databases. Mark with:

```rust
// DEPRECATED: facts tables are superseded by notes_index + notes_vec.
// Existing databases retain these tables until explicit cleanup.
// const FACTS_DDL: &str = "...";
```

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore`
Expected: PASS (some tests may need updating if they create fresh DBs and expect facts tables)

- [ ] **Step 3: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "chore(notes): deprecate facts table DDL — notes are the sole memory storage"
```

---

## Summary

| Phase | Tasks | Delivers |
|-------|-------|----------|
| Phase 1 | 1-2 | Schema + NoteStore restructured for `path/agent_id/category` |
| Phase 2 | 3-4 | NoteIndexer scans `{agent_id}/{category}/`, wikilink resolution |
| Phase 3 | 5 | Embedding vector index (`notes_vec`) |
| Phase 4 | 6-7 | NoteRetrieval service + wired into MemoryRetrieval |
| Phase 5 | 8-10 | Extraction uses `note_path`, compression defaults to notes |
| Phase 6 | 11 | Wiki merged, data migration updated |
| Phase 7 | 12-13 | Canvas uses paths, facts table deprecated |
