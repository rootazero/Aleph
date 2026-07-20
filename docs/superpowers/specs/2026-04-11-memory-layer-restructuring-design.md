# Memory Layer Restructuring Design

> Date: 2026-04-11
> Status: Draft
> Scope: Restructure flat notes into `memory/{agent_id}/{category}/` hierarchy, add embedding index, complete read path, merge wiki, switch default compression, deprecate facts table
> Predecessor: `2026-04-11-knowledge-notes-system-design.md` (Phase 1 implemented)

## Summary

Evolve the Knowledge Notes system from a flat `~/.aleph/data/notes/` directory into a structured Memory Layer at `~/.aleph/data/memory/{agent_id}/{category}/`. Each FactType enum maps to a category subdirectory. Wikilinks use Obsidian-compatible resolution (shortest unique path, with category prefix for disambiguation). The design completes the missing read path (embedding-based retrieval), merges the standalone wiki module, and switches the default compression pipeline to write notes instead of facts.

## Concept Model

```
~/.aleph/data/
├── sources/{agent_id}/          Raw Sources（原始资料，按 agent 隔离，不分类）
│   ├── conversations/           对话历史
│   ├── attachments/             文件附件、图片、音频等
│   └── ...                      SQLite 做路径引用即可
│
└── memory/{agent_id}/           Memory Layer（记忆层，按 category 分类）
    ├── {category}/*.md          LLM 提取的知识笔记
    └── ...                      SQLite = 可重建索引 + Canvas = Obsidian 风格图谱

                sources ──LLM提取──→ memory
```

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Raw sources | `sources/{agent_id}/` flat, SQLite references | Agent-isolated, no category subdivision; SQLite stores path references |
| Directory structure | `memory/{agent_id}/{category}/` | Agent isolation + category organization at filesystem level |
| Category folders | 15 FactType enums as kebab-case dirs | Natural mapping, no new taxonomy needed |
| Note granularity | Knowledge note (grouped facts per topic) | LLM decides grouping within each category |
| Index primary key | Relative path (`wiki/rust-ownership`) | Unique within agent, maps directly to filesystem |
| Wikilink format | Obsidian-compatible shortest-path | `[[filename]]` for unique, `[[category/filename]]` for disambiguation |
| Obsidian compatibility | Full — agent dir is a valid vault | Users can open with Obsidian to browse/edit |
| Embedding index | `notes_vec_{dim}` in SQLite | Same pattern as existing `facts_vec_{dim}` |
| Wiki module | Merge into memory | Wiki is just `memory/{agent_id}/wiki/`, no separate system |
| Facts table | Deprecate after migration | Notes become the sole memory storage |

## Directory Structure

```
~/.aleph/data/memory/
├── default/                        # agent_id
│   ├── preference/                 # FactType::Preference
│   │   ├── editor.md
│   │   └── coding-style.md
│   ├── plan/                       # FactType::Plan
│   ├── learning/                   # FactType::Learning
│   │   └── rust-basics.md
│   ├── project/                    # FactType::Project
│   │   └── aleph.md
│   ├── personal/                   # FactType::Personal
│   ├── tool/                       # FactType::Tool
│   ├── lesson/                     # FactType::Lesson
│   ├── skill/                      # FactType::Skill
│   │   └── rust-coding.md
│   ├── wiki/                       # FactType::Wiki
│   │   └── rust-ownership.md
│   ├── transcript/                 # FactType::Transcript
│   ├── subagent-run/               # FactType::SubagentRun
│   ├── subagent-session/           # FactType::SubagentSession
│   ├── subagent-checkpoint/        # FactType::SubagentCheckpoint
│   ├── subagent-transcript/        # FactType::SubagentTranscript
│   └── other/                      # FactType::Other
└── work-agent/                     # another agent_id
    ├── preference/
    └── ...
```

### Category ↔ FactType Mapping

| FactType | Directory name |
|----------|---------------|
| Preference | `preference` |
| Plan | `plan` |
| Learning | `learning` |
| Project | `project` |
| Personal | `personal` |
| Tool | `tool` |
| Lesson | `lesson` |
| Skill | `skill` |
| Wiki | `wiki` |
| Transcript | `transcript` |
| SubagentRun | `subagent-run` |
| SubagentSession | `subagent-session` |
| SubagentCheckpoint | `subagent-checkpoint` |
| SubagentTranscript | `subagent-transcript` |
| Other | `other` |

## Markdown File Format

Unchanged from Phase 1:

```markdown
---
category: wiki
tags: [rust, ownership, borrow-checker]
created: 2026-04-01
updated: 2026-04-10
---

- Rust uses ownership model for memory safety without GC
- Each value has exactly one owner at a time
- Borrowing allows temporary references without transferring ownership

Related: [[skill/rust-coding]] [[learning/rust-basics]]
```

## Wikilink Resolution (Obsidian-Compatible)

Resolution priority:

1. **Exact path match**: `[[wiki/rust-ownership]]` → look up `wiki/rust-ownership` in `notes_index.path`
2. **Global filename match**: `[[rust-ownership]]` → search `notes_index.filename = 'rust-ownership'`; if exactly one result, resolve to it
3. **Ambiguous**: If multiple files share the same filename across categories, return no match (require path prefix)
4. **Not found**: Create a placeholder note in `other/` category (LLM fills later)

```rust
pub async fn resolve_wikilink(
    store: &impl NoteStore,
    link: &str,
    agent_id: &str,
) -> Option<String> {
    // 1. Try exact path match
    if link.contains('/') {
        if store.get_note_index(link).await.ok()?.is_some() {
            return Some(link.to_string());
        }
    }
    // 2. Try global filename match
    let matches = store.find_by_filename(link, agent_id).await.ok()?;
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    None // Ambiguous or not found
}
```

## SQLite Index Schema Changes

### notes_index — add agent_id, category, filename columns

```sql
CREATE TABLE IF NOT EXISTS notes_index (
    path            TEXT PRIMARY KEY,       -- "wiki/rust-ownership" (relative within agent)
    filename        TEXT NOT NULL,           -- "rust-ownership" (for global wikilink resolution)
    agent_id        TEXT NOT NULL DEFAULT 'default',
    category        TEXT NOT NULL,           -- "wiki" (maps to FactType)
    tags_json       TEXT NOT NULL DEFAULT '[]',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    last_accessed_at INTEGER,
    content_hash    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notes_filename ON notes_index(filename);
CREATE INDEX IF NOT EXISTS idx_notes_agent ON notes_index(agent_id);
CREATE INDEX IF NOT EXISTS idx_notes_category ON notes_index(category);
```

### notes_links — use relative paths

```sql
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    from_note   TEXT NOT NULL,       -- "wiki/rust-ownership"
    to_note     TEXT NOT NULL,       -- "skill/rust-coding"
    UNIQUE(from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(to_note);
```

### notes_vec — new embedding index

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS notes_vec_{dim} USING vec0(
    path TEXT PRIMARY KEY,
    embedding float[{dim}]
);
```

One table per embedding dimension (768, 1024, 1536), matching the existing `facts_vec_{dim}` pattern.

### notes_fts — add filename column

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path,
    filename,
    content,
    tokenize='unicode61'
);
```

## NoteStore Trait Changes

```rust
#[async_trait]
pub trait NoteStore: Send + Sync {
    // Existing methods — update signatures to use path (not title)
    async fn index_note(&self, note: &KnowledgeNote, agent_id: &str) -> Result<(), AlephError>;
    async fn remove_note_index(&self, path: &str) -> Result<(), AlephError>;
    async fn get_note_index(&self, path: &str) -> Result<Option<NoteIndexEntry>, AlephError>;
    async fn list_notes(&self, agent_id: Option<&str>) -> Result<Vec<NoteIndexEntry>, AlephError>;
    async fn get_outgoing_links(&self, path: &str) -> Result<Vec<String>, AlephError>;
    async fn get_incoming_links(&self, path: &str) -> Result<Vec<String>, AlephError>;
    async fn search_notes_fts(&self, query: &str, limit: usize) -> Result<Vec<NoteIndexEntry>, AlephError>;
    async fn get_graph_data(&self, agent_id: &str, limit: usize) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;
    async fn get_neighbors(&self, center: &str, depth: u8, limit: usize) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>), AlephError>;

    // New methods
    async fn find_by_filename(&self, filename: &str, agent_id: &str) -> Result<Vec<String>, AlephError>;
    async fn upsert_embedding(&self, path: &str, embedding: &[f32], dim: u32) -> Result<(), AlephError>;
    async fn vector_search(&self, embedding: &[f32], dim: u32, agent_id: &str, limit: usize) -> Result<Vec<(String, f32)>, AlephError>;
}
```

### NoteIndexEntry — add fields

```rust
pub struct NoteIndexEntry {
    pub path: String,           // "wiki/rust-ownership"
    pub filename: String,       // "rust-ownership"
    pub agent_id: String,       // "default"
    pub category: String,       // "wiki"
    pub tags: Vec<String>,
    pub link_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    pub content_hash: String,
}
```

## NoteIndexer Changes

The indexer now scans `memory/{agent_id}/` recursively (one level of category subdirs):

```rust
impl NoteIndexer {
    pub fn new(memory_dir: PathBuf, store: Arc<S>) -> Self;

    // Scans memory_dir/{agent_id}/{category}/*.md
    pub async fn full_rebuild(&self, agent_id: &str) -> Result<IndexStats, AlephError>;

    // Path is relative: "wiki/rust-ownership"
    pub async fn index_file(&self, agent_id: &str, path: &Path) -> Result<bool, AlephError>;

    // Write to memory_dir/{agent_id}/{category}/{filename}.md
    pub async fn write_note(&self, agent_id: &str, note: &KnowledgeNote) -> Result<PathBuf, AlephError>;

    // Append facts to memory_dir/{agent_id}/{note_path}.md
    pub async fn append_to_note(&self, agent_id: &str, note_path: &str, new_facts: &[String], new_links: &[String]) -> Result<(), AlephError>;

    // Ensures all 15 category dirs exist
    pub async fn ensure_dirs(&self, agent_id: &str) -> Result<(), AlephError>;
}
```

## Write Path (LLM Extraction)

### NoteUpdate changes

```rust
pub struct NoteUpdate {
    pub note_path: String,          // "preference/editor" (category/filename)
    pub action: NoteAction,
    pub new_facts: Vec<String>,
    pub links: Vec<String>,
    pub tags: Option<Vec<String>>,
}
```

Note: `category` is derived from `note_path` (first segment). No separate field needed.

### Compression flow

```
1. compress_to_notes() called by Dream pipeline
2. LLM extracts NoteExtractionResponse with note_path (category/filename)
3. For each update:
   a. Write/append markdown file
   b. Generate embedding for note body
   c. Index note + upsert embedding
4. Update compression timestamp
```

### Extraction prompt changes

The prompt tells LLM the available categories and instructs it to include category in `note_path`:

```
CATEGORIES (use as path prefix):
preference, plan, learning, project, personal, tool, lesson, skill, wiki, transcript, other

OUTPUT FORMAT:
{
  "updates": [{
    "note_path": "preference/editor",
    "action": "append",
    "new_facts": ["..."],
    "links": ["wiki/rust-ownership"]
  }]
}
```

## Read Path (Memory Injection)

### NoteRetrieval

New retrieval service replacing the facts-based path:

```rust
pub struct NoteRetrieval {
    memory_dir: PathBuf,
    store: Arc<dyn NoteStore>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl NoteRetrieval {
    pub async fn retrieve(&self, query: &str, agent_id: &str, limit: usize) -> Result<Vec<NoteContent>, AlephError> {
        // 1. Embed query
        let embedding = self.embedder.embed(query).await?;

        // 2. Vector search in notes_vec
        let results = self.store.vector_search(&embedding, dim, agent_id, limit).await?;

        // 3. Read markdown files for top-K paths
        let mut notes = Vec::new();
        for (path, score) in results {
            let file_path = self.memory_dir.join(agent_id).join(format!("{path}.md"));
            if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
                notes.push(NoteContent { path, content, score });
            }
        }

        // 4. Optional: 1-hop wikilink expansion
        // Follow links from top notes to pull related context

        Ok(notes)
    }
}
```

### Integration with MemoryRetrieval

`src/memory/retrieval.rs` delegates to `NoteRetrieval` instead of `FactRetrieval`:

```rust
pub async fn retrieve_memories(&self, context: &ContextAnchor, query: &str) -> Result<Vec<MemoryEntry>, AlephError> {
    let note_retrieval = NoteRetrieval::new(memory_dir, store, embedder);
    let notes = note_retrieval.retrieve(query, agent_id, limit).await?;
    Ok(notes.into_iter().map(note_to_entry).collect())
}
```

## Wiki Module Merger

| Component | Action |
|-----------|--------|
| `src/wiki/mod.rs` | Remove — path helpers replaced by NoteIndexer |
| `src/wiki/tools.rs` (WikiManageTool) | Rewrite to call NoteIndexer for `memory/{agent_id}/wiki/` |
| `src/wiki/wikilink.rs` | Already covered by `src/memory/notes/wikilink.rs` |
| `src/wiki/git.rs` | Optional: keep as `memory/notes/git.rs` for git tracking |
| `src/wiki/index.rs` | Remove — auto-index generation is optional |
| Existing wiki files at `~/.aleph/data/wiki/` | Migrate to `~/.aleph/data/memory/{agent_id}/wiki/` |
| `WikiIngestStage` | Already no-op (Task 10) — remove entirely |
| `WikiLintStage` | Remove — replaced by wikilink resolution |

## Canvas Adjustments

Minimal changes from current implementation:

- Node ID = `path` (e.g., `wiki/rust-ownership`) instead of `title`
- Node display name = `filename` (e.g., `rust-ownership`)
- `get_graph_data()` now filters by `agent_id`
- Everything else (Obsidian-style rendering, interactions) stays the same

## Data Migration

### Existing facts → notes

Reuse the `migration.rs` from Phase 1, updated to:
1. Group facts by `agent_id` + `fact_type`
2. Map `fact_type` → category directory name
3. Derive filename from path (same logic as before)
4. Write to `memory/{agent_id}/{category}/{filename}.md`
5. Generate embeddings and index

### Existing wiki files → memory

```
~/.aleph/data/wiki/{agent_id}/{slug}.md
→ ~/.aleph/data/memory/{agent_id}/wiki/{slug}.md
```

Simple file move + re-index.

## Implementation Phases

```
Phase 1: Schema + NoteStore restructure
  - Update notes_index DDL (add path, filename, agent_id, category)
  - Update NoteStore trait signatures
  - Update SQLite implementation
  - Add find_by_filename()

Phase 2: NoteIndexer restructure
  - Scan {agent_id}/{category}/ directory structure
  - ensure_dirs() creates all 15 category folders
  - Update write_note/append_to_note for category paths
  - Update wikilink resolution with resolve_wikilink()

Phase 3: Embedding index
  - Add notes_vec_{dim} DDL
  - Add upsert_embedding() and vector_search() to NoteStore
  - Generate embeddings on index_note()

Phase 4: Read path
  - Create NoteRetrieval service
  - Wire into MemoryRetrieval (replace FactRetrieval delegation)
  - Optional: 1-hop wikilink expansion

Phase 5: Extraction prompt update
  - Update NoteUpdate to use note_path (category/filename)
  - Update extraction prompt with category list
  - Switch Dream pipeline to compress_to_notes() as default

Phase 6: Wiki merger + data migration
  - Move wiki files to memory/{agent_id}/wiki/
  - Rewrite WikiManageTool to use NoteIndexer
  - Migrate existing facts to notes (updated migration.rs)
  - Remove src/wiki/ module

Phase 7: Cleanup
  - Remove FactRetrieval, old compress_in_workspace path
  - Deprecate facts/facts_fts/facts_vec tables
  - Update gateway handlers for agent_id filtering
```

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Embedding generation slow on full rebuild | Batch embedding with rate limiting; incremental-only after first rebuild |
| Category mismatch (LLM puts note in wrong category) | Non-critical — note content is correct, category is organizational only |
| Filename collisions within category | Sanitize + append numeric suffix |
| Wikilink ambiguity (same filename in 2+ categories) | Require path prefix; warn user via Canvas detail panel |
| Migration data loss | Keep old facts table until verified; migration includes verification step |

## Non-Goals

- Real-time file watching (inotify/FSEvents) — scan on demand
- Obsidian plugin API compatibility — file format compatible, not plugin compatible
- Multi-user concurrent access — single user, single writer
- Cross-agent wikilinks — links are scoped within one agent
