# Retrieval Layer Notes Migration Design

**Date:** 2026-04-12
**Status:** Draft
**Scope:** Migrate retrieval system from facts table to notes, retire VFS, fully eliminate facts table
**Predecessor:** `docs/superpowers/specs/2026-04-12-dream-notes-migration-design.md`

---

## 1. Overview

The Dream daemon notes migration (completed) created the notes layer, refactored CompressionService, and removed most facts writes. However, 34 files still depend on `MemoryStore` facts methods — primarily the retrieval/recall system that serves LLM conversations. `MemoryFact` is referenced by 65 files.

This design completes the migration by:
- Extending `NoteStore` with hybrid search capabilities
- Building `NoteFactRetrieval` as a drop-in replacement for `FactRetrieval`
- Retiring the VFS (`aleph://` path model)
- Migrating all infrastructure callers from facts to notes
- Fully deleting the facts table, DDL, and MemoryStore facts methods

## 2. Key Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | VFS fully retired | Notes use real filesystem — VFS path model is redundant overhead |
| 2 | Reuse `MemoryFact` type, change data source | 65 files consume MemoryFact; introducing a new type creates unnecessary churn. Change the retrieval implementation, not the interface |
| 3 | Session data stays in `raw_memories` | Short-lived, high-volume, not worth creating markdown files |

## 3. NoteStore Extension

### 3.1 New Types

```rust
/// Search result from notes-based retrieval, carrying full content.
pub struct NoteSearchResult {
    pub path: String,          // "preference/editor"
    pub filename: String,
    pub category: String,
    pub tags: Vec<String>,
    pub content: String,       // markdown body (read from disk)
    pub score: f32,
    pub created_at: i64,
    pub updated_at: i64,
}
```

Bridge to existing type system (zero downstream changes):

```rust
impl NoteSearchResult {
    pub fn to_memory_fact(&self) -> MemoryFact {
        MemoryFact {
            id: self.path.clone(),
            content: self.content.clone(),
            note_type: NoteType::from_category(&self.category),
            path: format!("note://{}", self.path),
            tags: serde_json::to_string(&self.tags).unwrap_or_default(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            confidence: self.score,
            is_valid: true,
            tier: MemoryTier::LongTerm,
            strength: 1.0,
            decay_score: 1.0,
            ..Default::default()
        }
    }

    pub fn to_scored_fact(&self) -> ScoredFact {
        ScoredFact { fact: self.to_memory_fact(), score: self.score }
    }
}
```

### 3.2 New NoteStore Methods

```rust
/// Vector + FTS hybrid search with RRF fusion, returning full content.
async fn hybrid_search_notes(
    &self,
    embedding: &[f32],
    query_text: &str,
    agent_id: &str,
    dim_hint: u32,
    limit: usize,
) -> Result<Vec<NoteSearchResult>, AlephError>;

/// Vector search returning results with content (not just path+score).
async fn vector_search_notes_with_content(
    &self,
    embedding: &[f32],
    agent_id: &str,
    dim_hint: u32,
    limit: usize,
) -> Result<Vec<NoteSearchResult>, AlephError>;

/// Batch fetch note metadata by category.
async fn get_notes_by_category(
    &self,
    agent_id: &str,
    category: &str,
    limit: usize,
) -> Result<Vec<NoteIndexRow>, AlephError>;
```

### 3.3 hybrid_search_notes Implementation

1. Parallel: `notes_vec_{dim}` vector search + `notes_fts` FTS search
2. RRF (Reciprocal Rank Fusion) to merge two ranked lists
3. Sort by fused score, take top-k
4. Read markdown file content from disk for each result
5. Return `Vec<NoteSearchResult>`

## 4. Retrieval Layer Replacement

### 4.1 NoteFactRetrieval

```rust
/// Notes-based retrieval engine. Drop-in replacement for FactRetrieval.
/// Returns Vec<ScoredFact> — same type as before, different data source.
pub struct NoteFactRetrieval {
    indexer: NoteIndexer<SqliteMemoryBackend>,
    embedder: Arc<dyn EmbeddingProvider>,
    memory_dir: PathBuf,
}

impl NoteFactRetrieval {
    /// Hybrid search — replaces FactRetrieval::retrieve()
    pub async fn retrieve(
        &self,
        query: &str,
        workspace: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;

    /// Pure vector search — replaces MemoryStore::vector_search()
    pub async fn vector_retrieve(
        &self,
        query: &str,
        workspace: &str,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError>;
}
```

### 4.2 Replacement Mapping

| Old Call | New Call | Affected Files |
|----------|---------|----------------|
| `FactRetrieval::retrieve()` | `NoteFactRetrieval::retrieve()` | memory_search, thinker/memory_context_provider |
| `HybridRetrieval::search()` | `NoteFactRetrieval::retrieve()` | hybrid_retrieval (delete) |
| `MemoryStore::vector_search()` | `NoteFactRetrieval::vector_retrieve()` | memory_explore, ripple/task, tool_index |
| `MemoryStore::text_search()` | `NoteStore::search_notes_fts()` | few direct callers |
| `MemoryStore::get_facts_by_path_prefix()` | `RawMemoryStore::get_raw_by_path_prefix()` (session) or `NoteStore::list_notes()` (notes) | recall_context, session_summary_source |

### 4.3 Downstream Impact

**Zero.** All downstream consumers (thinker, payload assembler, scoring pipeline, 65 files referencing MemoryFact) continue to receive `Vec<ScoredFact>` with `MemoryFact` inside. The type contract is unchanged.

## 5. VFS Retirement

### 5.1 Modules Deleted

| Module | Replacement |
|--------|-------------|
| `src/memory/vfs/` | Filesystem operations on `~/.aleph/memory/note/` |
| `src/memory/vfs/l1_generator.rs` | Dream daemon NoteSynthesis stage |

### 5.2 memory_browse Rewrite

From VFS SQL queries to filesystem directory browsing:

```rust
pub struct NoteBrowseTool {
    memory_dir: PathBuf,
}

// Actions:
// list(category=None) → ls {agent}/ → list category directories
// list(category="wiki") → ls {agent}/wiki/ → list .md files
// read(path="wiki/rust-ownership") → cat {agent}/wiki/rust-ownership.md
```

### 5.3 memory_explore Adaptation

`RippleTask` changes from `MemoryStore::vector_search` to `NoteStore::vector_search_notes_with_content`. BFS logic unchanged.

## 6. Session Data Path

Session data (`aleph://session/` scoped summaries) stays in `raw_memories` table.

### 6.1 New RawMemoryStore Method

```rust
async fn get_raw_by_path_prefix(
    &self,
    path_prefix: &str,
    agent_id: &str,
    limit: usize,
) -> Result<Vec<RawMemory>, AlephError>;
```

### 6.2 Callers Migrated

| File | Current | After |
|------|---------|-------|
| `recall_context.rs` | `MemoryStore::get_facts_by_path_prefix("aleph://session/{id}/raw/")` | `RawMemoryStore::get_raw_by_path_prefix(...)` |
| `session_summary_source.rs` | `MemoryStore::get_facts_by_path_prefix("aleph://session/{id}/")` | `RawMemoryStore::get_raw_by_path_prefix(...)` |
| `session_compactor/mod.rs` | `invalidate_fact` during condensation | `mark_raw_as_processed` or new `invalidate_raw` method |

### 6.3 Rationale

Session data is short-lived (24h TTL), high-volume (every conversation turn), and session-scoped. Writing markdown files would create unnecessary filesystem I/O. `raw_memories` with `is_processed` flag is the right storage.

## 7. Infrastructure Migration

| Module | Current Dependency | Migration | Complexity |
|--------|-------------------|-----------|------------|
| `events/handler.rs` | Full CRUD on facts | Write target → NoteStore + markdown files | Medium |
| `reembed.rs` | `get_all_facts`, `update_fact` | → `list_notes` + `upsert_embedding` | Simple |
| `cli/commands.rs` | Full CRUD | → NoteIndexer CRUD API | Medium |
| `tool_index/coordinator.rs` | `insert_fact(NoteType::Tool)` | → `tool/` category notes | Medium |
| `ripple/task.rs` | `vector_search`, `load_embedding` | → `NoteStore::vector_search_notes_with_content` | Medium |
| `compression/conflict.rs` | `find_similar_facts`, `invalidate_fact` | → `NoteStore::vector_search` + `stale: true` marking | Medium |
| `capability/strategies/memory.rs` | `get_fact_stats` | → `NoteStore::count_all_notes()` | Simple |
| `bin/.../handlers.rs` | `get_all_facts` (startup migration) | Delete after migration complete | Simple |

## 8. Implementation Phases

### Phase 1: NoteStore Extension (Foundation)

- Add `NoteSearchResult` struct with `to_memory_fact()` / `to_scored_fact()` bridge
- Implement `hybrid_search_notes()` with RRF fusion
- Implement `vector_search_notes_with_content()`
- Implement `get_notes_by_category()`
- Unit tests for each new method

### Phase 2: Retrieval Layer Replacement (Critical Switch)

- Create `NoteFactRetrieval` with `retrieve()` and `vector_retrieve()`
- Swap all `FactRetrieval` callers to `NoteFactRetrieval`
- Swap all `HybridRetrieval` callers
- Delete `fact_retrieval.rs`, `hybrid_retrieval/`, `retrieval_trace.rs`
- Integration test: query → notes search → ScoredFact returned

### Phase 3: VFS Retirement + Tool Rewrite

- Delete `src/memory/vfs/` entirely
- Rewrite `memory_browse` as filesystem browser
- Adapt `memory_explore` / `RippleTask` to NoteStore
- Test browse list/read actions

### Phase 4: Session Data Path

- Add `RawMemoryStore::get_raw_by_path_prefix()`
- Migrate `recall_context.rs`
- Migrate `session_summary_source.rs`
- Migrate `session_compactor` invalidation
- Test session context restoration

### Phase 5: Infrastructure Migration

- Migrate `events/handler.rs` write target
- Migrate `reembed.rs`
- Migrate `cli/commands.rs`
- Migrate `tool_index/coordinator.rs`
- Migrate `ripple/task.rs`
- Migrate `compression/conflict.rs`
- Migrate `capability/strategies/memory.rs`
- Delete startup migration code

### Phase 6: Final Cleanup

- Delete `src/memory/store/sqlite/facts.rs`
- Remove facts DDL from `schema.rs`
- Remove all facts methods from `MemoryStore` trait
- Delete transition migration scripts
- Verify: `grep "facts" src/` zero table references
- Full test suite passes

## 9. Success Criteria

| Criterion | Verification |
|-----------|-------------|
| facts table DDL fully deleted | `schema.rs` contains no `CREATE TABLE facts` |
| MemoryStore has zero facts methods | Trait only has non-facts methods (or is deleted entirely) |
| All retrieval goes through notes | Integration test: insert note → search → get result |
| VFS module deleted | `src/memory/vfs/` does not exist |
| memory_browse uses filesystem | Tool test: list categories, read note file |
| Session data reads from raw_memories | Test: insert raw → query by path prefix → get result |
| Zero runtime facts table queries | Log analysis: no SQL against `facts` table |
| `grep -rn "facts" src/ \| grep -v test \| grep -v comment` | Zero facts table references in production code |

## 10. Risks

| Risk | Mitigation |
|------|------------|
| Retrieval quality regression | A/B test: run both old and new retrieval in parallel, compare results before switching |
| RRF fusion scoring differs from old hybrid | Tune RRF k parameter; existing scoring pipeline handles post-retrieval ranking |
| VFS retirement breaks unknown callers | `grep -rn "list_by_path\|get_by_path\|aleph://" src/` to find all VFS users before deletion |
| Session data path prefix mismatch | raw_memories may use different path format than facts; verify path conventions match |
| Large file I/O for content loading | Cache loaded content in NoteSearchResult; limit content preview size in retrieval |
