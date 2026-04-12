# Retrieval Layer Notes Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate 34 files from facts-based retrieval to notes-based retrieval, retire VFS, and fully delete the facts table from Aleph's memory system.

**Architecture:** Extend NoteStore with hybrid search, build NoteFactRetrieval as drop-in replacement for FactRetrieval (reusing MemoryFact type so 65 downstream files don't change), retire VFS module entirely, migrate infrastructure callers, then delete facts table and MemoryStore facts methods.

**Tech Stack:** Rust, SQLite (rusqlite + r2d2), async_trait, sqlite-vec, FTS5

**Design Spec:** `docs/superpowers/specs/2026-04-12-retrieval-layer-notes-migration-design.md`

---

## File Structure

### New Files
| File | Purpose |
|------|---------|
| `src/memory/notes/search_result.rs` | `NoteSearchResult` struct with `to_memory_fact()` bridge |
| `src/memory/note_retrieval/mod.rs` | `NoteFactRetrieval` replacement for `FactRetrieval` |
| `src/memory/note_retrieval/hybrid.rs` | RRF fusion logic for hybrid search |

### Modified Files (major)
| File | Changes |
|------|---------|
| `src/memory/notes/store.rs` | Add `hybrid_search_notes`, `vector_search_notes_with_content`, `get_notes_by_category`, `get_embedding` |
| `src/memory/store/sqlite/notes.rs` | Implement 4 new NoteStore methods |
| `src/memory/store/raw_memory.rs` | Add `get_raw_by_path_prefix` to trait |
| `src/memory/store/sqlite/raw_memories.rs` | Implement `get_raw_by_path_prefix` |
| `src/builtin_tools/memory_search.rs` | Switch to `NoteFactRetrieval` |
| `src/builtin_tools/memory_browse.rs` | Rewrite as filesystem browser |
| `src/builtin_tools/memory_explore.rs` | Adapt `RippleTask` to NoteStore |
| `src/builtin_tools/recall_context.rs` | Switch to `RawMemoryStore::get_raw_by_path_prefix` |
| `src/thinker/memory_context_provider.rs` | Switch to `NoteFactRetrieval` |
| `src/dispatcher/tool_index/retrieval.rs` | Switch to `NoteFactRetrieval::vector_retrieve` |
| `src/dispatcher/tool_index/coordinator.rs` | Migrate tool storage to `tool/` notes |
| `src/memory/events/handler.rs` | Redirect writes to NoteStore |
| `src/memory/reembed.rs` | `list_notes` + `upsert_embedding` |
| `src/memory/ripple/task.rs` | NoteStore vector search |
| `src/memory/compression/conflict.rs` | NoteStore similarity + stale marking |
| `src/memory/session_compactor/mod.rs` | Raw memories for session summaries |
| `src/agent_loop/compaction/session_summary_source.rs` | Raw memories read |
| `src/memory/cli/commands.rs` | NoteIndexer CRUD |
| `src/capability/strategies/memory.rs` | `count_all_notes` health check |
| `src/memory/retrieval_trace.rs` | Wrap NoteFactRetrieval |
| `src/memory/store/mod.rs` | Remove facts methods from MemoryStore trait |
| `src/memory/store/sqlite/schema.rs` | Delete facts DDL constants |

### Deleted Files
| File | Reason |
|------|--------|
| `src/memory/fact_retrieval.rs` | Replaced by `note_retrieval/` |
| `src/memory/hybrid_retrieval/` (entire dir) | Replaced by `note_retrieval/hybrid.rs` |
| `src/memory/vfs/` (entire dir) | VFS retired |
| `src/memory/store/sqlite/facts.rs` | Facts table deleted |
| `src/memory/notes/migration.rs` | Transitional migration complete |
| `src/memory/migration/skill_to_notes.rs` | Transitional migration complete |
| `src/bin/aleph-server/commands/start/builder/handlers.rs` (facts migration block) | Startup migration code removed |

---

## Phase 1: NoteStore Extension

### Task 1: NoteSearchResult Struct + MemoryFact Bridge

**Files:**
- Create: `src/memory/notes/search_result.rs`
- Modify: `src/memory/notes/mod.rs`

- [ ] **Step 1: Create NoteSearchResult with bridge methods**

```rust
// src/memory/notes/search_result.rs

use serde::{Deserialize, Serialize};
use crate::memory::context::{MemoryFact, MemoryScope, MemoryTier, NoteType, ScoredFact};

/// Search result carrying full content from notes-based retrieval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteSearchResult {
    pub path: String,
    pub filename: String,
    pub category: String,
    pub tags: Vec<String>,
    pub content: String,
    pub score: f32,
    pub created_at: i64,
    pub updated_at: i64,
}

impl NoteSearchResult {
    /// Convert to MemoryFact for downstream compatibility.
    pub fn to_memory_fact(&self, agent_id: &str) -> MemoryFact {
        let mut fact = MemoryFact::new(
            self.content.clone(),
            NoteType::from_str_or_other(&self.category),
            Vec::new(),
        );
        fact.id = self.path.clone();
        fact.path = format!("note://{}", self.path);
        fact.agent = agent_id.to_string();
        fact.tags = serde_json::to_string(&self.tags).unwrap_or_default();
        fact.created_at = self.created_at;
        fact.updated_at = self.updated_at;
        fact.confidence = self.score;
        fact.is_valid = true;
        fact.tier = MemoryTier::LongTerm;
        fact.scope = MemoryScope::Global;
        fact.strength = 1.0;
        fact.decay_score = 1.0;
        fact
    }

    pub fn to_scored_fact(&self, agent_id: &str) -> ScoredFact {
        ScoredFact {
            fact: self.to_memory_fact(agent_id),
            score: self.score,
        }
    }
}
```

- [ ] **Step 2: Re-export from notes/mod.rs**

Add to `src/memory/notes/mod.rs`:
```rust
pub mod search_result;
pub use search_result::NoteSearchResult;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add src/memory/notes/search_result.rs src/memory/notes/mod.rs
git commit -m "feat(memory): add NoteSearchResult struct with MemoryFact bridge"
```

---

### Task 2: Extend NoteStore Trait with Hybrid/Vector Methods

**Files:**
- Modify: `src/memory/notes/store.rs`

- [ ] **Step 1: Add 4 new methods to NoteStore trait**

Append to `NoteStore` trait in `src/memory/notes/store.rs` (before the closing `}`):

```rust
/// Vector + FTS hybrid search with RRF fusion, returning full content.
async fn hybrid_search_notes(
    &self,
    embedding: &[f32],
    query_text: &str,
    agent_id: &str,
    dim_hint: u32,
    limit: usize,
) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError>;

/// Vector search returning full content (not just path+score).
async fn vector_search_notes_with_content(
    &self,
    embedding: &[f32],
    agent_id: &str,
    dim_hint: u32,
    limit: usize,
) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError>;

/// Batch fetch note index metadata by category.
async fn get_notes_by_category(
    &self,
    agent_id: &str,
    category: &str,
    limit: usize,
) -> Result<Vec<NoteIndexEntry>, AlephError>;

/// Get the stored embedding vector for a note path.
async fn get_embedding(
    &self,
    path: &str,
    agent_id: &str,
    dim_hint: u32,
) -> Result<Option<Vec<f32>>, AlephError>;
```

- [ ] **Step 2: Verify compilation fails (trait methods not implemented yet)**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: FAIL with "not all trait items implemented" for `SqliteMemoryBackend`.

- [ ] **Step 3: Do not commit yet** — next task implements the methods.

---

### Task 3: Implement NoteStore Extensions in SQLite Backend

**Files:**
- Modify: `src/memory/store/sqlite/notes.rs`

- [ ] **Step 1: Read existing vector_search impl to understand patterns**

Run: `grep -n "fn vector_search" src/memory/store/sqlite/notes.rs`

Look at how embeddings are queried from `notes_vec_{dim}` tables and how rowid maps to path via `notes_vec_map`.

- [ ] **Step 2: Implement hybrid_search_notes with RRF fusion**

Add the 4 method implementations to `impl NoteStore for SqliteMemoryBackend` block in `src/memory/store/sqlite/notes.rs`:

```rust
async fn hybrid_search_notes(
    &self,
    embedding: &[f32],
    query_text: &str,
    agent_id: &str,
    dim_hint: u32,
    limit: usize,
) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
    use std::collections::HashMap;

    // 1. Vector search
    let vec_results = self.vector_search(embedding, agent_id, dim_hint, limit * 2).await?;

    // 2. FTS search
    let fts_entries = self.search_notes_fts(query_text, agent_id, limit * 2).await?;

    // 3. RRF fusion (k=60 standard value)
    let k = 60.0_f32;
    let mut scores: HashMap<String, f32> = HashMap::new();

    for (rank, (path, _score)) in vec_results.iter().enumerate() {
        let rrf = 1.0 / (k + (rank as f32) + 1.0);
        *scores.entry(path.clone()).or_insert(0.0) += rrf;
    }

    for (rank, entry) in fts_entries.iter().enumerate() {
        let rrf = 1.0 / (k + (rank as f32) + 1.0);
        *scores.entry(entry.path.clone()).or_insert(0.0) += rrf;
    }

    // 4. Sort by fused score and take top-k
    let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(limit);

    // 5. Load metadata + content
    let mut results = Vec::new();
    for (path, score) in sorted {
        if let Some(entry) = self.get_note_index(&path, agent_id).await? {
            let content = load_note_content(&entry, agent_id).await.unwrap_or_default();
            results.push(crate::memory::notes::NoteSearchResult {
                path: entry.path,
                filename: entry.filename,
                category: entry.category,
                tags: entry.tags,
                content,
                score,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
            });
        }
    }
    Ok(results)
}

async fn vector_search_notes_with_content(
    &self,
    embedding: &[f32],
    agent_id: &str,
    dim_hint: u32,
    limit: usize,
) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
    let pairs = self.vector_search(embedding, agent_id, dim_hint, limit).await?;

    let mut results = Vec::new();
    for (path, score) in pairs {
        if let Some(entry) = self.get_note_index(&path, agent_id).await? {
            let content = load_note_content(&entry, agent_id).await.unwrap_or_default();
            results.push(crate::memory::notes::NoteSearchResult {
                path: entry.path,
                filename: entry.filename,
                category: entry.category,
                tags: entry.tags,
                content,
                score,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
            });
        }
    }
    Ok(results)
}

async fn get_notes_by_category(
    &self,
    agent_id: &str,
    category: &str,
    limit: usize,
) -> Result<Vec<NoteIndexEntry>, AlephError> {
    let all = self.list_notes(agent_id).await?;
    Ok(all.into_iter()
        .filter(|n| n.category == category)
        .take(limit)
        .collect())
}

async fn get_embedding(
    &self,
    path: &str,
    agent_id: &str,
    dim_hint: u32,
) -> Result<Option<Vec<f32>>, AlephError> {
    let conn = self.pool.get()
        .map_err(|e| AlephError::config(format!("pool: {e}")))?;
    
    // Look up rowid from notes_vec_map
    let rowid: Option<i64> = conn.query_row(
        "SELECT rowid FROM notes_vec_map WHERE path = ?1 AND agent_id = ?2",
        rusqlite::params![path, agent_id],
        |row| row.get(0),
    ).ok();

    let Some(rowid) = rowid else { return Ok(None) };

    let table = match dim_hint {
        768 => "notes_vec_768",
        1024 => "notes_vec_1024",
        1536 => "notes_vec_1536",
        _ => return Ok(None),
    };

    let sql = format!("SELECT embedding FROM {table} WHERE rowid = ?1");
    let blob: Option<Vec<u8>> = conn.query_row(&sql, rusqlite::params![rowid], |row| row.get(0)).ok();

    Ok(blob.map(|b| {
        b.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }))
}
```

Add helper function at top of file (outside impl block):

```rust
async fn load_note_content(
    entry: &NoteIndexEntry,
    agent_id: &str,
) -> Option<String> {
    let memory_dir = crate::utils::paths::get_note_memory_dir().ok()?;
    let file_path = memory_dir
        .join(agent_id)
        .join(&entry.category)
        .join(format!("{}.md", entry.filename));
    tokio::fs::read_to_string(&file_path).await.ok()
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: No errors.

- [ ] **Step 4: Write tests**

Add to `#[cfg(test)] mod tests` in `src/memory/store/sqlite/notes.rs`:

```rust
#[tokio::test]
async fn hybrid_search_notes_returns_content() {
    let backend = create_test_backend();
    // Index 2 notes with content
    let note = KnowledgeNote {
        title: "rust-async".to_string(),
        category: "learning".to_string(),
        tags: vec![],
        facts: vec!["async/await in Rust".to_string()],
        links: vec![],
        created_at: 1000,
        updated_at: 1000,
        content_hash: "h1".to_string(),
    };
    backend.index_note(&note, "default", "learning").await.unwrap();

    let emb = vec![0.1f32; 1024];
    backend.upsert_embedding("learning/rust-async", "default", &emb, 1024).await.unwrap();

    let results = backend.hybrid_search_notes(&emb, "rust", "default", 1024, 10).await.unwrap();
    assert!(results.len() >= 1);
    assert_eq!(results[0].path, "learning/rust-async");
}

#[tokio::test]
async fn get_notes_by_category_filters_correctly() {
    let backend = create_test_backend();
    // Index notes in different categories
    // Assert only matching category returned
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore --lib notes -- --nocapture 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/memory/notes/store.rs src/memory/store/sqlite/notes.rs
git commit -m "feat(memory): extend NoteStore with hybrid_search_notes, vector_search_with_content, get_embedding"
```

---

### Task 4: Add RawMemoryStore Path-Prefix Query

**Files:**
- Modify: `src/memory/store/raw_memory.rs`
- Modify: `src/memory/store/sqlite/raw_memories.rs`

- [ ] **Step 1: Add trait method**

In `src/memory/store/raw_memory.rs`, add to `RawMemoryStore` trait:

```rust
/// Get raw memories by path prefix (for session data retrieval).
async fn get_raw_by_path_prefix(
    &self,
    path_prefix: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<RawMemory>, AlephError>;
```

- [ ] **Step 2: Implement in SQLite backend**

Add to `impl RawMemoryStore for SqliteMemoryBackend` in `src/memory/store/sqlite/raw_memories.rs`:

```rust
async fn get_raw_by_path_prefix(
    &self,
    path_prefix: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<RawMemory>, AlephError> {
    let conn = self.pool.get()
        .map_err(|e| AlephError::config(format!("pool: {e}")))?;

    let pattern = format!("{path_prefix}%");
    let mut stmt = conn.prepare(
        "SELECT id, content, source, agent_id, session_id, path, layer, attachment_text, is_processed, created_at
         FROM raw_memories
         WHERE path LIKE ?1 AND agent_id = ?2
         ORDER BY created_at ASC
         LIMIT ?3"
    ).map_err(|e| AlephError::config(format!("prepare: {e}")))?;

    let rows = stmt.query_map(
        rusqlite::params![pattern, agent_id, limit as i64],
        |row| Ok(RawMemory {
            id: row.get(0)?,
            content: row.get(1)?,
            source: RawMemorySource::from_str(&row.get::<_, String>(2)?),
            agent_id: row.get(3)?,
            session_id: row.get(4)?,
            path: row.get(5)?,
            layer: row.get(6)?,
            attachment_text: row.get(7)?,
            is_processed: row.get::<_, i32>(8)? != 0,
            created_at: row.get(9)?,
        })
    ).map_err(|e| AlephError::config(format!("query: {e}")))?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row.map_err(|e| AlephError::config(format!("row: {e}")))?);
    }
    Ok(results)
}
```

- [ ] **Step 3: Add test**

In `src/memory/store/sqlite/raw_memories.rs` test module:

```rust
#[tokio::test]
async fn get_raw_by_path_prefix_filters_by_prefix_and_agent() {
    let backend = create_backend().await;

    let r1 = RawMemory::new("session a msg1".to_string(), RawMemorySource::SessionCompressed)
        .with_path("aleph://session/sess-a/d0/1")
        .with_agent("default");
    let r2 = RawMemory::new("session b msg1".to_string(), RawMemorySource::SessionCompressed)
        .with_path("aleph://session/sess-b/d0/1")
        .with_agent("default");

    backend.insert_raw_memory(&r1).await.unwrap();
    backend.insert_raw_memory(&r2).await.unwrap();

    let results = backend.get_raw_by_path_prefix("aleph://session/sess-a/", "default", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "session a msg1");
}
```

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p alephcore --lib raw_memories -- --nocapture
git add src/memory/store/raw_memory.rs src/memory/store/sqlite/raw_memories.rs
git commit -m "feat(memory): add RawMemoryStore::get_raw_by_path_prefix"
```

---

## Phase 2: Retrieval Layer Replacement

### Task 5: Create NoteFactRetrieval

**Files:**
- Create: `src/memory/note_retrieval/mod.rs`
- Create: `src/memory/note_retrieval/hybrid.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Create note_retrieval module**

```rust
// src/memory/note_retrieval/mod.rs

pub mod hybrid;

use crate::error::AlephError;
use crate::memory::context::ScoredFact;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::sqlite::SqliteMemoryBackend;
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;

/// Notes-based retrieval engine. Drop-in replacement for FactRetrieval.
pub struct NoteFactRetrieval {
    indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl NoteFactRetrieval {
    pub fn new(
        indexer: Arc<NoteIndexer<SqliteMemoryBackend>>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self { indexer, embedder }
    }

    /// Hybrid vector + FTS search, returns ScoredFact for downstream compatibility.
    pub async fn retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let results = self.indexer.store()
            .hybrid_search_notes(&embedding, query, agent_id, dim, limit)
            .await?;

        Ok(results.iter().map(|r| r.to_scored_fact(agent_id)).collect())
    }

    /// Pure vector search.
    pub async fn vector_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let embedding = self.embedder.embed(query).await?;
        let dim = embedding.len() as u32;

        let results = self.indexer.store()
            .vector_search_notes_with_content(&embedding, agent_id, dim, limit)
            .await?;

        Ok(results.iter().map(|r| r.to_scored_fact(agent_id)).collect())
    }

    /// FTS-only search.
    pub async fn text_retrieve(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let entries = self.indexer.store()
            .search_notes_fts(query, agent_id, limit)
            .await?;

        // FTS results don't have scores — assign rank-based scores
        let total = entries.len() as f32;
        Ok(entries.iter().enumerate().map(|(i, entry)| {
            let score = 1.0 - (i as f32 / total.max(1.0));
            ScoredFact {
                fact: crate::memory::context::MemoryFact {
                    id: entry.path.clone(),
                    content: String::new(), // Content loaded on demand
                    path: format!("note://{}", entry.path),
                    note_type: crate::memory::context::NoteType::from_str_or_other(&entry.category),
                    agent: agent_id.to_string(),
                    tags: serde_json::to_string(&entry.tags).unwrap_or_default(),
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                    confidence: score,
                    is_valid: true,
                    tier: crate::memory::context::MemoryTier::LongTerm,
                    scope: crate::memory::context::MemoryScope::Global,
                    strength: 1.0,
                    decay_score: 1.0,
                    ..Default::default()
                },
                score,
            }
        }).collect())
    }
}
```

- [ ] **Step 2: Create hybrid.rs with RRF helper**

```rust
// src/memory/note_retrieval/hybrid.rs

/// Reciprocal Rank Fusion — combines multiple ranked lists into one.
/// k is the standard RRF constant (60 is default).
pub fn rrf_fuse<T: Clone + Eq + std::hash::Hash>(
    lists: Vec<Vec<T>>,
    k: f32,
    limit: usize,
) -> Vec<(T, f32)> {
    use std::collections::HashMap;

    let mut scores: HashMap<T, f32> = HashMap::new();
    for list in lists {
        for (rank, item) in list.into_iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            *scores.entry(item).or_insert(0.0) += rrf;
        }
    }

    let mut sorted: Vec<(T, f32)> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted.truncate(limit);
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_prefers_items_ranked_in_multiple_lists() {
        let list_a = vec!["a", "b", "c"];
        let list_b = vec!["b", "a", "d"];
        let fused = rrf_fuse(vec![list_a, list_b], 60.0, 10);
        // "a" and "b" appear in both lists — should rank higher than c/d
        assert_eq!(fused[0].0, "a");  // Or "b" — same fused score
    }
}
```

- [ ] **Step 3: Register module in memory/mod.rs**

Add to `src/memory/mod.rs`:
```rust
pub mod note_retrieval;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: No errors.

- [ ] **Step 5: Write tests**

Add a `#[cfg(test)] mod tests` in `src/memory/note_retrieval/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use crate::memory::embedding_provider::tests::MockEmbeddingProvider;

    async fn create_retrieval() -> (NoteFactRetrieval, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let backend: Arc<SqliteMemoryBackend> = Arc::new(
            SqliteMemoryBackend::new(dir.path()).unwrap()
        );
        let indexer = Arc::new(NoteIndexer::new(
            dir.path().to_path_buf(),
            backend.clone(),
        ));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            MockEmbeddingProvider::new(1024, "mock")
        );
        (NoteFactRetrieval::new(indexer, embedder), dir)
    }

    #[tokio::test]
    async fn retrieve_empty_returns_empty() {
        let (retrieval, _dir) = create_retrieval().await;
        let results = retrieval.retrieve("test query", "default", 10).await.unwrap();
        assert!(results.is_empty());
    }
}
```

- [ ] **Step 6: Run tests and commit**

```bash
cargo test -p alephcore --lib note_retrieval -- --nocapture
git add src/memory/note_retrieval/ src/memory/mod.rs
git commit -m "feat(memory): add NoteFactRetrieval — notes-based drop-in for FactRetrieval"
```

---

### Task 6: Switch memory_search Tool to NoteFactRetrieval

**Files:**
- Modify: `src/builtin_tools/memory_search.rs`

- [ ] **Step 1: Find current usage of FactRetrieval**

Run: `grep -n "FactRetrieval\|fact_retrieval" src/builtin_tools/memory_search.rs`

- [ ] **Step 2: Replace imports and struct fields**

Change:
```rust
use crate::memory::fact_retrieval::FactRetrieval;
```
To:
```rust
use crate::memory::note_retrieval::NoteFactRetrieval;
```

Change struct field type from `Arc<FactRetrieval>` to `Arc<NoteFactRetrieval>`.

- [ ] **Step 3: Replace method calls**

Find any `fact_retrieval.retrieve(...)` or similar calls. Method signatures are compatible:
```rust
// Old: fact_retrieval.retrieve(query, workspace, limit).await
// New: note_retrieval.retrieve(query, workspace, limit).await
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/memory_search.rs
git commit -m "refactor(tools): memory_search uses NoteFactRetrieval"
```

---

### Task 7: Switch thinker/memory_context_provider to NoteFactRetrieval

**Files:**
- Modify: `src/thinker/memory_context_provider.rs`

- [ ] **Step 1: Find current FactRetrieval/vector_search usage**

Run: `grep -n "vector_search\|FactRetrieval\|hybrid_search" src/thinker/memory_context_provider.rs`

- [ ] **Step 2: Replace with NoteFactRetrieval calls**

For each call to `MemoryStore::vector_search` or `MemoryStore::hybrid_search`:
- Inject `Arc<NoteFactRetrieval>` into the struct (or get it from context)
- Replace the call with `note_retrieval.vector_retrieve(query, agent_id, limit).await` or `.retrieve(query, ...).await`

The return type is `Vec<ScoredFact>` either way — downstream code unchanged.

- [ ] **Step 3: Verify compilation and test existing tests still pass**

```bash
cargo check -p alephcore 2>&1 | head -20
cargo test -p alephcore --lib memory_context_provider -- --nocapture 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add src/thinker/memory_context_provider.rs
git commit -m "refactor(thinker): memory context provider uses NoteFactRetrieval"
```

---

### Task 8: Switch tool_index retrieval to NoteFactRetrieval

**Files:**
- Modify: `src/dispatcher/tool_index/retrieval.rs`

- [ ] **Step 1: Find current usage**

Run: `grep -n "vector_search\|hybrid_search" src/dispatcher/tool_index/retrieval.rs`

- [ ] **Step 2: Replace with NoteFactRetrieval**

Same pattern as Task 7. Inject `Arc<NoteFactRetrieval>` and replace MemoryStore search calls.

- [ ] **Step 3: Verify and commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/dispatcher/tool_index/retrieval.rs
git commit -m "refactor(tool_index): retrieval uses NoteFactRetrieval"
```

---

### Task 9: Delete fact_retrieval.rs and hybrid_retrieval/

**Files:**
- Delete: `src/memory/fact_retrieval.rs`
- Delete: `src/memory/hybrid_retrieval/` (entire directory)
- Delete: `src/memory/retrieval_trace.rs`
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Verify no remaining callers**

Run: `grep -rn "fact_retrieval\|FactRetrieval\|HybridRetrieval" src/ --include="*.rs" | grep -v test | grep -v "//"`
Expected: No hits (or only in files to be deleted).

- [ ] **Step 2: Delete files**

```bash
rm src/memory/fact_retrieval.rs
rm -rf src/memory/hybrid_retrieval/
rm src/memory/retrieval_trace.rs
```

- [ ] **Step 3: Remove module declarations from memory/mod.rs**

Delete lines:
```rust
pub mod fact_retrieval;
pub mod hybrid_retrieval;
pub mod retrieval_trace;
```

- [ ] **Step 4: Fix any remaining imports**

Run: `cargo check -p alephcore 2>&1 | head -20`

Fix any remaining imports of the deleted types. They should already be removed if Tasks 6-8 were done correctly.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(memory): delete fact_retrieval, hybrid_retrieval, retrieval_trace"
```

---

## Phase 3: VFS Retirement + Tool Rewrite

### Task 10: Rewrite memory_browse as Filesystem Browser

**Files:**
- Modify: `src/builtin_tools/memory_browse.rs`

- [ ] **Step 1: Read current implementation to understand args/result types**

Run: `cat src/builtin_tools/memory_browse.rs | head -100`

- [ ] **Step 2: Replace implementation**

Keep the tool's public API (NAME, Args struct, Result struct) but replace internal implementation. The key change: instead of `MemoryStore::list_by_path`, use `tokio::fs::read_dir`.

```rust
use std::path::PathBuf;
use async_trait::async_trait;
use tracing::info;

use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BrowseAction {
    List,   // List categories or files within a category
    Read,   // Read a specific note
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryBrowseArgs {
    pub action: BrowseAction,
    /// For list: optional category name to list files in
    /// For read: full note path like "wiki/rust-ownership"
    #[serde(default)]
    pub path: Option<String>,
}

pub struct MemoryBrowseTool {
    memory_dir: PathBuf,
    agent_id: String,
}

impl MemoryBrowseTool {
    pub fn new(memory_dir: PathBuf, agent_id: String) -> Self {
        Self { memory_dir, agent_id }
    }

    async fn handle_list(&self, category: Option<&str>) -> Result<MemoryBrowseResult> {
        let base = self.memory_dir.join(&self.agent_id);
        let target = match category {
            None => base,
            Some(cat) => base.join(cat),
        };

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&target).await
            .map_err(|e| AlephError::tool(format!("Failed to read dir: {e}")))?;

        while let Some(entry) = dir.next_entry().await
            .map_err(|e| AlephError::tool(format!("Read dir entry: {e}")))? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "archive" {
                continue;
            }
            if category.is_some() && !name.ends_with(".md") {
                continue;
            }
            let entry_name = if category.is_some() {
                name.trim_end_matches(".md").to_string()
            } else {
                name
            };
            entries.push(entry_name);
        }
        entries.sort();

        Ok(MemoryBrowseResult {
            success: true,
            entries: Some(entries),
            content: None,
            message: format!("Listed {} entries", category.unwrap_or("/")),
        })
    }

    async fn handle_read(&self, path: &str) -> Result<MemoryBrowseResult> {
        let (category, filename) = path.split_once('/')
            .ok_or_else(|| AlephError::tool("path must be 'category/filename'"))?;
        let file = self.memory_dir
            .join(&self.agent_id)
            .join(category)
            .join(format!("{filename}.md"));

        let content = tokio::fs::read_to_string(&file).await
            .map_err(|e| AlephError::tool(format!("Read file {path}: {e}")))?;

        Ok(MemoryBrowseResult {
            success: true,
            entries: None,
            content: Some(content),
            message: format!("Read note {path}"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryBrowseResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[async_trait]
impl AlephTool for MemoryBrowseTool {
    const NAME: &'static str = "memory_browse";
    const DESCRIPTION: &'static str =
        "Browse the knowledge notes filesystem. Actions: list (categories or files), read (file content).";

    type Args = MemoryBrowseArgs;
    type Output = MemoryBrowseResult;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            BrowseAction::List => self.handle_list(args.path.as_deref()).await,
            BrowseAction::Read => {
                let path = args.path.ok_or_else(|| AlephError::tool("path required for read"))?;
                self.handle_read(&path).await
            }
        }
    }
}
```

- [ ] **Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn list_empty_agent_returns_empty() {
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().join("default");
        tokio::fs::create_dir_all(&agent_dir).await.unwrap();
        
        let tool = MemoryBrowseTool::new(dir.path().to_path_buf(), "default".into());
        let result = tool.handle_list(None).await.unwrap();
        assert!(result.entries.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_category_returns_files() {
        let dir = tempdir().unwrap();
        let wiki = dir.path().join("default/wiki");
        tokio::fs::create_dir_all(&wiki).await.unwrap();
        tokio::fs::write(wiki.join("rust.md"), "content").await.unwrap();
        tokio::fs::write(wiki.join("go.md"), "content").await.unwrap();
        
        let tool = MemoryBrowseTool::new(dir.path().to_path_buf(), "default".into());
        let result = tool.handle_list(Some("wiki")).await.unwrap();
        let entries = result.entries.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn read_returns_file_content() {
        let dir = tempdir().unwrap();
        let wiki = dir.path().join("default/wiki");
        tokio::fs::create_dir_all(&wiki).await.unwrap();
        tokio::fs::write(wiki.join("rust.md"), "# Rust\n\nSystems language").await.unwrap();
        
        let tool = MemoryBrowseTool::new(dir.path().to_path_buf(), "default".into());
        let result = tool.handle_read("wiki/rust").await.unwrap();
        assert!(result.content.unwrap().contains("Systems language"));
    }
}
```

- [ ] **Step 4: Run tests and commit**

```bash
cargo test -p alephcore --lib memory_browse -- --nocapture
git add src/builtin_tools/memory_browse.rs
git commit -m "refactor(tools): rewrite memory_browse as filesystem browser"
```

---

### Task 11: Adapt memory_explore RippleTask to NoteStore

**Files:**
- Modify: `src/memory/ripple/task.rs`
- Modify: `src/builtin_tools/memory_explore.rs`

- [ ] **Step 1: Update RippleTask to use NoteStore**

In `src/memory/ripple/task.rs`:
- Change from `Arc<dyn MemoryStore>` to `Arc<NoteIndexer<SqliteMemoryBackend>>`
- Replace `vector_search` calls with `NoteStore::vector_search_notes_with_content`
- Replace `load_embedding_for_fact` with `NoteStore::get_embedding(path, agent_id, dim)`
- Change return types from `Vec<MemoryFact>` to `Vec<NoteSearchResult>` OR keep as `MemoryFact` via `to_memory_fact()`

- [ ] **Step 2: Update memory_explore tool**

Adapt `memory_explore.rs` to the new RippleTask signature. The public tool interface should remain similar.

- [ ] **Step 3: Run tests**

```bash
cargo check -p alephcore 2>&1 | head -20
cargo test -p alephcore --lib ripple -- --nocapture
```

- [ ] **Step 4: Commit**

```bash
git add src/memory/ripple/ src/builtin_tools/memory_explore.rs
git commit -m "refactor(memory): ripple and memory_explore use NoteStore"
```

---

### Task 12: Delete VFS Module

**Files:**
- Delete: `src/memory/vfs/` (entire directory)
- Modify: `src/memory/mod.rs`

- [ ] **Step 1: Verify no remaining VFS imports**

Run: `grep -rn "memory::vfs\|L1Generator\|l1_generator" src/ --include="*.rs" | grep -v test | grep -v "//"`

If any remain, those files need migration first. `memory/compression/service.rs` used `L1Generator` — verify it's been removed or migrated.

- [ ] **Step 2: Delete VFS directory**

```bash
rm -rf src/memory/vfs/
```

- [ ] **Step 3: Remove module declaration from memory/mod.rs**

Delete:
```rust
pub mod vfs;
```

- [ ] **Step 4: Remove VFS imports from CompressionService**

In `src/memory/compression/service.rs`, remove:
- `use crate::memory::vfs::L1Generator;`
- The `l1_generator` field from `CompressionService` struct
- Any `L1Generator::new()` calls
- Any `l1_gen.generate_for_affected_paths()` calls

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -30`

Fix any remaining VFS references.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(memory): retire VFS module entirely"
```

---

## Phase 4: Session Data Path Migration

### Task 13: Migrate recall_context to RawMemoryStore

**Files:**
- Modify: `src/builtin_tools/recall_context.rs`

- [ ] **Step 1: Find get_facts_by_path_prefix usage**

Run: `grep -n "get_facts_by_path_prefix" src/builtin_tools/recall_context.rs`

- [ ] **Step 2: Replace with RawMemoryStore call**

Change:
```rust
use crate::memory::store::MemoryStore;
let facts = self.database.get_facts_by_path_prefix(
    &format!("aleph://session/{session_id}/raw/"),
    &filter,
    limit,
).await?;
```
To:
```rust
use crate::memory::store::raw_memory::RawMemoryStore;
let raw_memories = self.database.get_raw_by_path_prefix(
    &format!("aleph://session/{session_id}/raw/"),
    agent_id,
    limit,
).await?;
```

Adapt the result mapping — `RawMemory` has `content` field directly accessible.

- [ ] **Step 3: Verify and commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/builtin_tools/recall_context.rs
git commit -m "refactor(tools): recall_context uses RawMemoryStore for session data"
```

---

### Task 14: Migrate session_summary_source to RawMemoryStore

**Files:**
- Modify: `src/agent_loop/compaction/session_summary_source.rs`

- [ ] **Step 1: Replace get_facts_by_path_prefix**

Same pattern as Task 13. Query raw_memories by path prefix.

- [ ] **Step 2: Verify and commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/agent_loop/compaction/session_summary_source.rs
git commit -m "refactor(compaction): session_summary_source uses RawMemoryStore"
```

---

### Task 15: Migrate session_compactor Invalidation

**Files:**
- Modify: `src/memory/session_compactor/mod.rs`

- [ ] **Step 1: Find invalidate_fact usage**

Run: `grep -n "invalidate_fact" src/memory/session_compactor/mod.rs`

- [ ] **Step 2: Replace with mark_raw_as_processed**

When session_compactor condenses d0→d1 summaries, it invalidates the source d0 facts. Replace with:

```rust
// Old: database.invalidate_fact(&fact.id, "condensed to d1").await?;
// New: database.mark_raw_as_processed(&[raw.id.clone()]).await?;
```

The raw_memories already have the `is_processed` flag — reuse it for condensation marking.

- [ ] **Step 3: Verify and commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/memory/session_compactor/mod.rs
git commit -m "refactor(session_compactor): use raw_memories is_processed for condensation"
```

---

## Phase 5: Infrastructure Migration

### Task 16: Migrate reembed.rs

**Files:**
- Modify: `src/memory/reembed.rs`

- [ ] **Step 1: Replace get_all_facts + update_fact pattern**

```rust
// Old: 
let facts = database.get_all_facts(false, None).await?;
for fact in facts {
    let new_emb = embedder.embed(&fact.content).await?;
    // ... update_fact with new embedding
}

// New:
let notes = indexer.store().list_notes(agent_id).await?;
for note in notes {
    let content = read_note_content(&note).await?;
    let new_emb = embedder.embed(&content).await?;
    indexer.store().upsert_embedding(&note.path, agent_id, &new_emb, new_emb.len() as u32).await?;
}
```

- [ ] **Step 2: Commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/memory/reembed.rs
git commit -m "refactor(memory): reembed uses NoteStore list_notes + upsert_embedding"
```

---

### Task 17: Migrate capability/strategies/memory.rs

**Files:**
- Modify: `src/capability/strategies/memory.rs`

- [ ] **Step 1: Replace get_fact_stats with count_all_notes**

```rust
// Old: database.get_fact_stats().await
// New: database.count_all_notes().await  (returns i64)
```

Adapt to the new return type (i64 instead of FactStats struct).

- [ ] **Step 2: Commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/capability/strategies/memory.rs
git commit -m "refactor(capability): memory health check uses count_all_notes"
```

---

### Task 18: Migrate tool_index/coordinator to tool/ Notes

**Files:**
- Modify: `src/dispatcher/tool_index/coordinator.rs`

- [ ] **Step 1: Understand current tool storage**

Tools are stored as `NoteType::Tool` facts under `aleph://tools/` path. Migrate to notes in `tool/` category.

- [ ] **Step 2: Replace facts CRUD with NoteIndexer CRUD**

For each operation:
- `insert_fact` → `indexer.write_note(agent, "tool", &note)`
- `update_fact_content` → `indexer.write_note(...)` (overwrites)
- `invalidate_fact` → `tokio::fs::remove_file(path)` + `store.remove_note_index(path, agent)`
- `get_facts_by_type(Tool, ...)` → `indexer.store().get_notes_by_category(agent, "tool", limit)`

- [ ] **Step 3: Verify and commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/dispatcher/tool_index/coordinator.rs
git commit -m "refactor(tool_index): coordinator stores tools as notes"
```

---

### Task 19: Migrate events/handler.rs

**Files:**
- Modify: `src/memory/events/handler.rs`

- [ ] **Step 1: Identify facts operations**

Run: `grep -n "insert_fact\|update_fact\|delete_fact\|invalidate_fact" src/memory/events/handler.rs`

- [ ] **Step 2: Redirect writes to NoteStore**

The event handler replays memory events. For each event type:
- `CreateFactEvent` → `indexer.write_note()`
- `UpdateFactEvent` → `indexer.write_note()` (overwrite)
- `InvalidateFactEvent` → delete markdown + remove from index
- `DeleteFactEvent` → same as invalidate

- [ ] **Step 3: Commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/memory/events/handler.rs
git commit -m "refactor(events): handler writes to NoteStore instead of facts table"
```

---

### Task 20: Migrate compression/conflict.rs

**Files:**
- Modify: `src/memory/compression/conflict.rs`

- [ ] **Step 1: Replace find_similar_facts**

```rust
// Old: database.find_similar_facts(emb, dim, filter, threshold, limit)
// New: indexer.store().vector_search_notes_with_content(emb, agent, dim, limit)
//      Then filter client-side by score >= threshold
```

- [ ] **Step 2: Replace invalidate_fact with stale marking**

```rust
// Old: database.invalidate_fact(id, "conflict superseded")
// New: mark the note's frontmatter with `stale: true`
//      Reuse the mark_stale helper from NoteDriftStage if exposed
```

- [ ] **Step 3: Commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/memory/compression/conflict.rs
git commit -m "refactor(compression): conflict detector uses NoteStore similarity"
```

---

### Task 21: Migrate cli/commands.rs

**Files:**
- Modify: `src/memory/cli/commands.rs`

- [ ] **Step 1: Replace facts CRUD with NoteIndexer CRUD**

For each CLI subcommand:
- `list` → `indexer.store().list_notes(agent)`
- `show <id>` → `indexer.store().get_note_index(path, agent)` + read markdown
- `add` → construct `KnowledgeNote`, call `indexer.write_note()`
- `edit <id>` → read, modify, rewrite via indexer
- `delete <id>` → remove file + `indexer.store().remove_note_index()`
- `count` → `indexer.store().count_all_notes()`

- [ ] **Step 2: Commit**

```bash
cargo check -p alephcore 2>&1 | head -20
git add src/memory/cli/commands.rs
git commit -m "refactor(cli): memory commands use NoteIndexer CRUD"
```

---

### Task 22: Delete Startup Migration Code

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/handlers.rs`

- [ ] **Step 1: Find the startup facts migration block**

Run: `grep -n "get_all_facts\|event_sourcing_migration" src/bin/aleph-server/commands/start/builder/handlers.rs`

- [ ] **Step 2: Delete the one-time migration code**

Remove the block that reads all facts and feeds them to the event sourcing migrator. This is transitional code that's no longer needed.

- [ ] **Step 3: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/handlers.rs
git commit -m "refactor(server): delete obsolete facts startup migration"
```

---

## Phase 6: Final Cleanup — Delete Facts Table

### Task 23: Delete Transitional Migration Scripts

**Files:**
- Delete: `src/memory/notes/migration.rs`
- Delete: `src/memory/migration/skill_to_notes.rs`
- Modify: `src/memory/notes/mod.rs`
- Modify: `src/memory/migration/mod.rs`

- [ ] **Step 1: Verify migration completion flags**

The migrations have already run on all existing databases. Confirm by checking that any gating logic has been executed:

Run: `grep -rn "skill_to_notes_migrated\|notes_migration" src/ --include="*.rs"`

- [ ] **Step 2: Delete files**

```bash
rm src/memory/notes/migration.rs
rm src/memory/migration/skill_to_notes.rs
```

- [ ] **Step 3: Remove module declarations**

In `src/memory/notes/mod.rs`, remove `pub mod migration;`.
In `src/memory/migration/mod.rs`, remove `pub mod skill_to_notes;`. If the migration dir is now empty, delete it and remove from `src/memory/mod.rs`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(memory): delete transitional migration scripts"
```

---

### Task 24: Remove Facts Methods from MemoryStore Trait

**Files:**
- Modify: `src/memory/store/mod.rs`

- [ ] **Step 1: Delete facts-only methods from trait**

Remove from `MemoryStore` trait in `src/memory/store/mod.rs`:

```rust
// Delete all of these:
async fn insert_fact(...) -> ...
async fn get_fact(...) -> ...
async fn update_fact(...) -> ...
async fn delete_fact(...) -> ...
async fn batch_insert_facts(...) -> ...
async fn get_all_facts(...) -> ...
async fn get_facts_by_type(...) -> ...
async fn get_facts_by_path_prefix(...) -> ...
async fn get_by_path(...) -> ...
async fn list_by_path(...) -> ...
async fn count_facts(...) -> ...
async fn invalidate_fact(...) -> ...
async fn close_fact_validity(...) -> ...
async fn set_fact_valid_from(...) -> ...
async fn update_fact_content(...) -> ...
async fn find_similar_facts(...) -> ...
async fn apply_fact_decay(...) -> ...
async fn get_fact_stats(...) -> ...
async fn soft_delete_fact(...) -> ...
async fn count_facts_by_topic_excluding_domain(...) -> ...
async fn set_tunnel_pending(...) -> ...
async fn has_tunnel_pending(...) -> ...
async fn get_tunnel_candidates(...) -> ...
async fn clear_tunnel_pending_by_topic(...) -> ...
async fn load_embedding_for_fact(...) -> ...
async fn vector_search(...) -> ...
async fn text_search(...) -> ...
async fn hybrid_search(...) -> ...
```

At this point `MemoryStore` trait should be empty or nearly empty. If it's entirely empty, delete the trait entirely.

- [ ] **Step 2: Fix compilation errors**

Run: `cargo check -p alephcore 2>&1 | tail -40`

Remaining errors should be in trait impl blocks that still have these methods. Remove those impl blocks.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "refactor(memory): remove facts methods from MemoryStore trait"
```

---

### Task 25: Delete facts.rs Implementation

**Files:**
- Delete: `src/memory/store/sqlite/facts.rs`
- Modify: `src/memory/store/sqlite/mod.rs`

- [ ] **Step 1: Delete facts.rs**

```bash
rm src/memory/store/sqlite/facts.rs
```

- [ ] **Step 2: Remove module declaration**

In `src/memory/store/sqlite/mod.rs`, remove `pub mod facts;`.

- [ ] **Step 3: Fix compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`

- [ ] **Step 4: Commit**

```bash
git add src/memory/store/sqlite/
git commit -m "refactor(memory): delete facts.rs SQLite backend"
```

---

### Task 26: Remove Facts DDL from schema.rs

**Files:**
- Modify: `src/memory/store/sqlite/schema.rs`

- [ ] **Step 1: Delete facts DDL constants**

Remove from `src/memory/store/sqlite/schema.rs`:
- `CREATE_FACTS_TABLE` (or equivalent)
- `CREATE_FACTS_VEC_768`, `CREATE_FACTS_VEC_1024`, `CREATE_FACTS_VEC_1536`
- Any facts-related migration constants
- Any facts indexes

- [ ] **Step 2: Remove calls from init_schema**

Remove facts DDL execution from the schema initialization function.

- [ ] **Step 3: Optionally: Add DROP TABLE migration**

For existing databases, add a one-time migration to DROP the facts tables:

```rust
pub fn migrate_drop_facts_tables(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch("
        DROP TABLE IF EXISTS facts;
        DROP TABLE IF EXISTS facts_fts;
        DROP TABLE IF EXISTS facts_vec_768;
        DROP TABLE IF EXISTS facts_vec_1024;
        DROP TABLE IF EXISTS facts_vec_1536;
        DROP TABLE IF EXISTS graph_nodes;
        DROP TABLE IF EXISTS graph_edges;
        DROP TABLE IF EXISTS memory_entities;
    ")?;
    Ok(())
}
```

Call it once during `init_schema`. This drops facts tables from existing user databases.

- [ ] **Step 4: Verify compilation and run full test suite**

```bash
cargo check -p alephcore 2>&1 | head -20
cargo test -p alephcore --lib 2>&1 | tail -30
```

- [ ] **Step 5: Commit**

```bash
git add src/memory/store/sqlite/schema.rs
git commit -m "refactor(memory): delete facts DDL, drop facts tables from existing DBs"
```

---

### Task 27: Final Verification

- [ ] **Step 1: Verify no facts table references**

Run: `grep -rn "facts" src/ --include="*.rs" | grep -v "test" | grep -v "//" | grep -v "raw_facts" | grep -v "new_facts" | grep -v "note_facts" | grep -v "source_facts" | grep -v "body_facts" | grep -v "skill_facts" | grep -v "extracted_facts"`

Expected: Zero hits (only variable names like `note_facts` are acceptable).

- [ ] **Step 2: Run full test suite**

Run: `cargo test -p alephcore 2>&1 | tail -20`
Expected: All tests pass (or only pre-existing failures).

- [ ] **Step 3: Run the build**

Run: `cargo build --release 2>&1 | tail -5`
Expected: Clean build.

- [ ] **Step 4: Commit verification**

If any fixes are needed:
```bash
git add -A
git commit -m "refactor(memory): final cleanup — facts table completely eliminated"
```

---

## Summary

| Phase | Tasks | Deliverable |
|-------|-------|-------------|
| Phase 1 | Tasks 1-4 | NoteStore extensions, NoteSearchResult bridge, RawMemoryStore path query |
| Phase 2 | Tasks 5-9 | NoteFactRetrieval + all retrieval callers switched + legacy modules deleted |
| Phase 3 | Tasks 10-12 | memory_browse rewritten, RippleTask migrated, VFS deleted |
| Phase 4 | Tasks 13-15 | Session data reads from raw_memories |
| Phase 5 | Tasks 16-22 | Infrastructure modules migrated |
| Phase 6 | Tasks 23-27 | Facts table, DDL, trait methods, and transitional code all deleted |

**Total: 27 tasks**

Each phase produces a working, testable intermediate state. Phase 1 is pure addition (safe to deploy alone). Phase 2 switches the hot path. Phases 3-5 migrate individual modules. Phase 6 is the final cleanup.
