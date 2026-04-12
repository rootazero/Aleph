# Dream Daemon Notes Migration Design

**Date:** 2026-04-12
**Status:** Draft
**Scope:** Dream daemon refactoring, CompressionService quality improvement, facts table elimination, tool unification

---

## 1. Overview

Aleph's memory system has been partially migrated from a `facts` table to a markdown-based Knowledge Notes system (`notes_index` + markdown files). However, three critical subsystems remain on the old `facts` table:

1. **Dream daemon** (6 stages) — deeply coupled to facts for decay, consolidation, drift detection, wiki sync
2. **CompressionService** — reads raw session data from facts, writes to notes (but with quality issues)
3. **Skill/Wiki tools** — maintain independent write paths bypassing the notes pipeline

This design completes the migration by:
- Creating a dedicated `raw_memories` table for raw data storage
- Refactoring CompressionService to fix 5 quality issues and integrate attachment processing
- Redesigning Dream daemon's 6 stages to operate entirely on the notes layer
- Unifying `skill_manage` and `wiki_manage` into a single `note_manage` tool
- Completely deleting the `facts` table

## 2. Data Layer Architecture

### 2.1 Two-Layer Model

```
L0: raw_memories (SQLite)
  Raw conversation data, session summaries, transcripts, attachment text.
  Ephemeral — consumed by CompressionService, marked as processed.

L1: notes (Markdown files + SQLite index)
  Persistent knowledge. Markdown files are source of truth.
  SQLite tables (notes_index, notes_links, notes_fts, notes_vec_*) are rebuildable indexes.
```

### 2.2 Data Flow

```
Gateway ──► raw_memories ──CompressionService──► notes (markdown + index)
  │              │              (realtime)            │
  │              │                                    ▼
  │         is_processed=1                    Dream daemon (daily/weekly)
  │                                           maintains notes quality
  │
  ├─ SessionCompactor: conversation summaries → raw_memories
  ├─ TranscriptIndexer: raw conversation text → raw_memories
  └─ Media pipeline: attachment text extraction → raw_memories.attachment_text
```

### 2.3 raw_memories Table Schema

```sql
CREATE TABLE IF NOT EXISTS raw_memories (
    id              TEXT PRIMARY KEY,
    content         TEXT NOT NULL,
    source          TEXT NOT NULL,       -- "session_compressed" | "transcript" | "tool_output" | "attachment"
    agent_id        TEXT NOT NULL DEFAULT 'default',
    session_id      TEXT,
    path            TEXT,                -- aleph:// traceability path
    layer           TEXT,                -- d0/d1/d2 for session summaries
    attachment_text TEXT,                -- extracted text from attachments (PDF, Word, images)
    is_processed    INTEGER DEFAULT 0,   -- set to 1 after CompressionService consumes
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_raw_unprocessed ON raw_memories(is_processed, created_at)
    WHERE is_processed = 0;
CREATE INDEX idx_raw_agent ON raw_memories(agent_id);
CREATE INDEX idx_raw_session ON raw_memories(session_id);
```

### 2.4 Tables Retained

| Table | Status | Purpose |
|-------|--------|---------|
| `raw_memories` | **New** | Replaces facts as raw data store |
| `notes_index` | Retained | Note metadata index |
| `notes_links` | Retained | Wikilink graph |
| `notes_fts` | Retained | Full-text search |
| `notes_vec_map` + `notes_vec_*` | Retained | Vector retrieval |
| `recall_signals` | Retained | Access tracking for NoteDecay; `ALTER TABLE recall_signals RENAME COLUMN fact_id TO note_path` |
| `dream_reports` | Retained | Schema updated with new field names |
| `dream_status` | Retained | No changes |
| `daily_insights` | Retained | No changes |
| `compression_metadata` | Retained | No changes |

### 2.5 Tables Deleted

- `facts` — completely removed
- `facts_vec_768`, `facts_vec_1024`, `facts_vec_1536` — removed
- `facts_fts` — already not created for new DBs
- `graph_nodes`, `graph_edges`, `memory_entities` — already not created

## 3. CompressionService Refactoring

### 3.1 Data Source Switch

```rust
// Old: database.get_uncompressed_session_facts(last_timestamp, ...)
// New: database.get_unprocessed_raw_memories(agent_id, batch_size)
//      → SELECT * FROM raw_memories WHERE is_processed = 0
//        AND agent_id = ? ORDER BY created_at ASC LIMIT ?

// After processing:
// Old: database.invalidate_consumed_chunks(&consumed_ids)
// New: database.mark_raw_as_processed(&consumed_ids)
//      → UPDATE raw_memories SET is_processed = 1 WHERE id IN (...)
```

### 3.2 Quality Fixes (5 Issues)

| # | Issue | Root Cause | Fix |
|---|-------|-----------|-----|
| 1 | AI response lost | `MemoryEntry` sets `ai_output = ""` at `service.rs:208-218` | Preserve full AI response from raw_memories content |
| 2 | AI response truncated | `extractor.rs:347` truncates to 500 chars | Increase to 2000 chars; use summary for longer content |
| 3 | Note content invisible | Only `existing_titles` passed to LLM | Pass first 500 chars of target note body as context |
| 4 | Attachments ignored | No media handling in compression pipeline | Read `raw_memories.attachment_text` and inject into prompt |
| 5 | No quality validation | Extraction results written without checks | Validate: non-empty, valid category, no duplicate facts |

### 3.3 Attachment Text in Extraction Prompt

```
--- Conversation 1 (ID: mem-123) ---
User: Help me analyze this architecture document
[Attachment]: The system uses microservices architecture, including user service, order service...
Assistant: This document has several issues: 1. Service granularity is too fine...

Extract notes (JSON only):
```

### 3.4 Upstream Writers Adaptation

| Writer | Current | After |
|--------|---------|-------|
| `SessionCompactor` | `insert_fact()` → facts | `insert_raw_memory()` → raw_memories |
| `TranscriptIndexer` | `insert_fact()` → facts | `insert_raw_memory()` → raw_memories |
| Gateway media pipeline | Understanding result in context only | Extract text → `attachment_text` field in raw_memories |

## 4. Dream Daemon 6-Stage Redesign

### 4.1 New DreamContext

```rust
pub struct DreamContext {
    pub notes: Vec<NoteEntry>,
    pub note_contents: HashMap<String, String>,  // path → markdown body (lazy loaded)
    pub agent_id: String,
    pub database: MemoryBackend,
    pub indexer: NoteIndexer<SqliteMemoryBackend>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub report: DreamReport,
}

pub struct NoteEntry {
    pub path: String,           // "preference/editor"
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_accessed_at: Option<i64>,
    pub content_hash: String,
}
```

### 4.2 Stage Definitions

#### Stage 1: NoteConsolidate — Merge Similar/Duplicate Notes

- **Input:** notes_index full scan + notes_vec similarity search
- **Logic:**
  1. Group by category, compute pairwise embedding similarity within each group
  2. Pairs with similarity > 0.85 → candidate merge list
  3. LLM decides: merge / coexist / absorb
  4. Merge: combine markdown content into one file, delete the other
  5. Absorb: append unique facts from absorbed note to primary, delete absorbed
- **Output:** `consolidated_count`, `merged_pairs`
- **Safety:** Back up original files before merge

#### Stage 2: NoteDrift — Detect Contradictions/Stale Information

- **Input:** Notes updated in the last 7 days (`updated_at > now - 7d`)
- **Logic:**
  1. For each recently updated note, retrieve wikilink-connected notes
  2. Submit related facts to LLM for consistency check
  3. LLM verdict: consistent / contradictory / stale
  4. Contradictory: mark outdated facts in older note (strikethrough or "## Superseded" section)
  5. Stale: add `stale: true` to frontmatter
- **Output:** `contradictions_found`, `notes_marked_stale`

#### Stage 3: NoteSynthesis — Cross-Note Insight Generation (Weekly Only)

- **Input:** All notes grouped by category
- **Logic:**
  1. DBSCAN clustering within each category (embedding-based)
  2. Extract representative facts per cluster, submit to LLM for synthesis
  3. Write synthesis to `synthesis/{category}-insights.md`
  4. Add wikilinks to source notes
- **Trigger:** Weekly pipeline only
- **Output:** `synthesis_count`, `clusters_found`

#### Stage 4: NoteLint — Format Normalization + Broken Link Repair

- **Input:** notes_index full scan + notes_links
- **Logic:**
  1. Frontmatter completeness check (required: category, tags, created, updated)
  2. Broken link detection: `notes_links.to_note` not in `notes_index`
  3. Auto-fix:
     - Missing frontmatter fields → fill defaults
     - Broken links → fuzzy match repair if target is filename-only without category
     - Unfixable broken links → add to report
  4. Rebuild FTS/embedding index if `content_hash` changed
- **Output:** `format_fixed`, `broken_links_found`, `links_repaired`

#### Stage 5: NoteDecay — Access-Based Cleanup

- **Input:** notes_index full scan + recall_signals
- **Logic:**
  1. Compute activity score per note:
     `score = access_count * 0.4 + recency_weight * 0.3 + link_count * 0.3`
     `recency_weight = 1.0 / (1.0 + days_since_last_access / 30.0)`
  2. Within each category, bottom 10% with score < threshold → cleanup candidates
  3. Protection rules:
     - wiki/skill categories have lower threshold (harder to clean)
     - Notes with 3+ incoming links are protected
     - Notes created < 7 days ago are protected
  4. Candidates moved to `archive/{category}/` (not deleted)
- **Output:** `notes_archived`, `notes_protected`

#### Stage 6: DailyDigest — Daily Summary Generation

- **Input:** Notes created or updated in the last 24 hours
- **Logic:**
  1. Collect notes with `created_at` or `updated_at > now - 24h`
  2. Read note body content
  3. LLM generates daily activity summary + key insights
  4. Write to `daily_insights` table (existing schema)
- **Output:** Daily insight text

### 4.3 Pipeline Orchestration

```rust
impl DreamPipeline {
    pub fn daily() -> Self {
        Self::new(vec![
            Box::new(NoteConsolidateStage),  // merge first to reduce volume
            Box::new(NoteDriftStage),         // detect contradictions
            Box::new(NoteLintStage),          // format fixes
            Box::new(NoteDecayStage),         // cleanup low-value
            Box::new(DailyDigestStage),       // generate daily report
        ])
    }

    pub fn weekly() -> Self {
        Self::new(vec![
            Box::new(NoteConsolidateStage),
            Box::new(NoteDriftStage),
            Box::new(NoteSynthesisStage),     // weekly-only: deep synthesis
            Box::new(NoteLintStage),
            Box::new(NoteDecayStage),
            Box::new(DailyDigestStage),
        ])
    }
}
```

### 4.4 DreamReport Schema

```rust
pub struct DreamReport {
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: u64,
    pub notes_consolidated: u32,
    pub contradictions_found: u32,
    pub notes_marked_stale: u32,
    pub synthesis_count: u32,
    pub format_fixed: u32,
    pub broken_links_found: u32,
    pub notes_archived: u32,
    pub errors: Option<String>,
}
```

## 5. note_manage Unified Tool

### 5.1 Tool Interface

```rust
pub enum NoteManageAction {
    Create,
    Update,
    Append,
    Query,
    List,
    Delete,
}

pub struct NoteManageArgs {
    pub action: NoteManageAction,
    pub category: Option<String>,      // preference/plan/learning/.../skill/wiki/other
    pub filename: Option<String>,      // kebab-case, no .md suffix
    pub title: Option<String>,         // required for create
    pub content: Option<String>,       // markdown body
    pub facts: Option<Vec<String>>,    // for append action
    pub links: Option<Vec<String>>,    // wikilinks
    pub tags: Option<Vec<String>>,
    pub query: Option<String>,         // for query action
    pub limit: Option<usize>,
}
```

### 5.2 Category-Specific Frontmatter Templates

```rust
fn frontmatter_template(category: &str, title: &str, tags: &[String]) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d").to_string();
    let tags_str = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());

    match category {
        "wiki" => format!(
            "---\ntitle: {title}\naliases: []\ntags: {tags_str}\nsources: []\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
        "skill" => format!(
            "---\ntitle: {title}\nscope: persona\ntags: {tags_str}\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
        _ => format!(
            "---\ncategory: {category}\ntags: {tags_str}\ncreated: \"{now}\"\nupdated: \"{now}\"\n---"
        ),
    }
}
```

### 5.3 Wiki-Specific Post-Write Hooks

Wiki retains git version management and index.md auto-generation as post-write hooks within `note_manage`:

```rust
if category == "wiki" {
    git_manager.commit_changes(agent_id, action, filename)?;
    regenerate_wiki_index(indexer, agent_id)?;
}
```

### 5.4 Migration Path

1. Implement `NoteManageTool`, register as `note_manage`
2. Mark `skill_manage` and `wiki_manage` as `#[deprecated]`, delegate internally to `note_manage`
3. Update LLM tool schema so `note_manage` description covers skill/wiki scenarios
4. Next release: remove `skill_manage` / `wiki_manage`

### 5.5 Skill Data Migration

Skill data currently exists only in the facts table (no markdown files). One-time migration:

```rust
let skill_facts = database.get_all_facts()
    .filter(|f| f.note_type == NoteType::Skill && f.is_valid);

for fact in skill_facts {
    let name = skill_name_from_path(&fact.path);
    let category_hint = skill_category_from_path(&fact.path);

    let note = KnowledgeNote {
        title: name,
        category: "skill".into(),
        tags: vec![category_hint],
        facts: parse_skill_content_to_facts(&fact.content),
        links: vec![],
        created_at: fact.created_at,
        updated_at: fact.updated_at,
        content_hash: String::new(),
    };
    indexer.write_note(agent_id, "skill", &note).await?;
}
```

## 6. Facts Table Elimination

### 6.1 Phased Migration (Order Must Not Be Reversed)

**Phase 1: Create Table + Dual Write**
- Create `raw_memories` table
- SessionCompactor dual-writes (facts + raw_memories)
- TranscriptIndexer dual-writes (facts + raw_memories)
- Verify: both tables have consistent data

**Phase 2: Switch Reads**
- CompressionService reads from raw_memories
- Verify: compression pipeline produces notes correctly
- Recall/retrieval switches to notes_fts + notes_vec

**Phase 3: Stop Writes + Migrate Legacy Data**
- SessionCompactor stops writing to facts (raw_memories only)
- TranscriptIndexer stops writing to facts
- Skill facts → skill/*.md (one-time migration script)
- Wiki facts already have markdown, just clean up redundant facts rows
- Verify: facts table has no active reads or writes

**Phase 4: Drop Table**
- `DROP TABLE facts`
- `DROP TABLE facts_vec_768, facts_vec_1024, facts_vec_1536`
- Clean up facts-related DDL in schema.rs
- Clean up all facts-related Rust code (MemoryFact write paths, FactSource, etc.)

### 6.2 Code Cleanup

| Module | Cleanup |
|--------|---------|
| `schema.rs` | Remove facts/facts_vec/facts_fts/graph_nodes/graph_edges DDL |
| `MemoryStore` trait | Remove `insert_fact`, `get_all_facts`, `invalidate_fact`, `update_fact`, etc. |
| `sqlite/mod.rs` | Remove all facts table SQL implementations |
| `MemoryFact` struct | Retain as DTO for CompressionService internal use; remove facts-table-specific fields (decay_score, tier, strength) |
| `skill_manage` / `wiki_manage` | Fully remove after Phase 4 |
| `ConflictDetector` | Remove facts-level conflict detection (replaced by Dream daemon NoteDrift) |
| `MemoryCommandHandler` | Remove facts-related event sourcing commands |
| `wiki_sync.rs` | Delete (replaced by note_manage + NoteLint) |

### 6.3 recall_signals Adaptation

```sql
-- Rename fact_id to note_path
ALTER TABLE recall_signals RENAME COLUMN fact_id TO note_path;
```

NoteDecay stage uses `recall_signals.note_path` to compute access frequency.

## 7. Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Data loss during migration | Dual-write phase: if raw_memories write fails, facts still has data | During Phase 1, failure in either write only warns (no panic), ensuring at least one side has data |
| CompressionService quality regression | New prompt may produce unexpected results | Retain `compress_in_workspace()` (legacy path) as fallback via config flag |
| Dream daemon false merge/delete | NoteConsolidate or NoteDecay misoperation | Decay moves to archive (not delete); Consolidate backs up originals; all writes logged to dream_reports |
| Skill migration content loss | Facts table skill content has inconsistent format | Migration script does dry-run first, outputs report for manual confirmation |
| Wiki git history break | Wiki files switch from facts pipeline to notes pipeline | Wiki markdown file location unchanged (`memory/note/{agent}/wiki/`), git history continues naturally |

## 8. Out of Scope

| Item | Reason |
|------|--------|
| Transcript embedding migration | Transcripts in raw_memories still need vector retrieval; belongs to retrieval layer refactoring |
| Multi-agent isolation changes | Existing agent_id mechanism unchanged; notes already have agent_id isolation |
| dream_reports historical data migration | Old reports keep old field names; not worth migrating |
| CompressionService LLM model optimization | Can use stronger models later; not an architectural concern |

## 9. Success Criteria

| Criterion | Verification |
|-----------|-------------|
| Facts table completely removed | `grep -r "facts" src/` returns zero hits (excluding comments and variable names) |
| CompressionService extracts from raw_memories and writes notes | Integration test: insert raw_memory → trigger compress → verify markdown file exists |
| Dream daemon 6 stages all pass | Each stage has at least 3 unit tests (happy path + empty input + edge case) |
| note_manage covers all CRUD operations | Tool tests: create/update/append/query/list/delete for each category |
| Attachment text flows into compression | Integration test: raw_memory with attachment_text → verify notes contain attachment content |
| No data loss | Pre/post migration: skill note count matches, wiki note count matches |
