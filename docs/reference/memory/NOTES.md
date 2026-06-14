# Knowledge Notes (L1)

> Markdown-first persistent knowledge. Each note is one `.md` file; SQLite tables are rebuildable indexes.

## Curated Hot Memory (MEMORY.md) — sibling concept

A separate, single-file *curated hot memory* lives at
`~/.aleph/agents/{agent_id}/MEMORY.md` alongside the L1 notes library. It is **not** a Knowledge Note — it is a small bounded "hot zone" rendered into the system prompt at session start.

- **Format:** entries separated by `\n§\n`. The `remember` tool is the only writer (LLM-driven add / replace / remove); direct edits via `self_config(write_file)` are rejected.
- **Char budget:** default 2,200 chars (configurable in `[memory.curated]`). Over-budget writes are rejected; the LLM must `replace` or `remove` first.
- **Frozen snapshot:** captured once per `(agent_id, session_key)` and reused for every prompt build in the session. Refreshes only on compression-run completion or session end (Hermes-inspired prefix-cache stability).
- **Threat scanning:** every write goes through `content_scanner` (prompt-injection / exfiltration / SSH access / invisible-unicode patterns).
- **Legacy compatibility:** existing free-format `MEMORY.md` is read as a single legacy entry; `add` is blocked until the LLM curates it via `replace` / `remove`.

Module: `src/memory/curated/`. Spec: `docs/superpowers/specs/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md`.

## 1. Overview

Notes are the L1 **persistent** layer of Aleph's memory stack. Three claims define the contract:

1. **Markdown is the source of truth.** Every note is a single `.md` file on disk at `~/.aleph/memory/note/{agent_id}/{category}/{filename}.md`. A human can read, diff, back up, and version-control these files without ever touching the database.
2. **SQLite is a rebuildable index.** The `notes_index`, `notes_links`, `notes_fts`, `notes_vec_map`, and `notes_vec_{768,1024,1536}` tables exist solely to make lookup, wikilink graph traversal, full-text search, and semantic search fast. `NoteIndexer::full_rebuild` can reconstruct every row of every index table from the markdown files alone.
3. **Per-agent isolation.** Every path, query, and index row is scoped by `agent_id`. Two agents running against the same database see disjoint note namespaces with no fallthrough.

## 2. Filesystem Layout

All note categories are enumerated by `CATEGORY_DIRS` in `src/memory/notes/indexer.rs`:

```text
~/.aleph/memory/note/
└── {agent_id}/
    ├── preference/*.md
    ├── plan/*.md
    ├── learning/*.md
    ├── project/*.md
    ├── personal/*.md
    ├── tool/*.md
    ├── lesson/*.md
    ├── skill/*.md
    ├── reference/*.md
    ├── transcript/*.md
    ├── subagent-run/*.md
    ├── subagent-session/*.md
    ├── subagent-checkpoint/*.md
    ├── subagent-transcript/*.md
    └── other/*.md
```

`NoteIndexer::ensure_dirs` creates every one of these directories lazily on first use. Filenames are sanitized by `sanitize_title` (see §4) to strip path separators and filesystem-unsafe characters, so a malicious note title like `../../etc/passwd` becomes the literal `etcpasswd` before ever touching disk.

## 3. Frontmatter Schema

### 3.1 Fields

Notes are read and written by `KnowledgeNote::from_markdown` / `KnowledgeNote::to_markdown` (`src/memory/notes/note/mod.rs`). The real `Frontmatter` struct (`src/memory/notes/note/parsing.rs`) declares:

```rust
pub(super) struct Frontmatter {
    pub(super) category: String,            // #[serde(default)]
    pub(super) tags: Vec<String>,           // #[serde(default)]
    pub(super) created: Option<String>,     // YYYY-MM-DD; #[serde(default)]
    pub(super) updated: Option<String>,     // YYYY-MM-DD; #[serde(default)]
    pub(super) confidence: f32,             // default 1.0
    pub(super) severity: Severity,          // #[serde(default)]
    pub(super) source_notes: Vec<String>,   // alias "source_facts"; #[serde(default)]
    pub(super) status: NoteStatus,          // #[serde(default)]
    pub(super) supersedes: Vec<String>,     // #[serde(default)]
    pub(super) superseded_by: Vec<String>,  // #[serde(default)]
    pub(super) permanent: bool,             // true → exempt from decay; #[serde(default)]
    pub(super) relations: Vec<Relation>,    // typed relation edges; #[serde(default)]
}
```

Every field is `#[serde(default)]`, so missing values fall through to sane defaults rather than erroring. Dates are parsed as `YYYY-MM-DD` (UTC midnight) by `parse_date_to_unix`; empty / missing dates yield `0`. `to_markdown` serializes all non-default fields back to YAML frontmatter.

The minimum on-disk shape for a note created via `NoteManageTool` is:

```yaml
---
category: {category}
tags: {tags_json}
created: "{YYYY-MM-DD}"
updated: "{YYYY-MM-DD}"
---
```

Additional fields (`confidence`, `severity`, `status`, `supersedes`, `superseded_by`, `permanent`, `relations`, `source_notes`) are emitted when non-default. Forward-compatible: unknown fields are ignored by the parser.

## 4. `KnowledgeNote` Data Model

Verbatim from `src/memory/notes/note.rs`:

```rust
/// A knowledge note — the primary memory unit.
///
/// Parsed from (and serializable back to) a markdown file with YAML frontmatter.
#[derive(Debug, Clone)]
pub struct KnowledgeNote {
    /// Filename without `.md` extension
    pub title: String,
    /// From frontmatter `category` field
    pub category: String,
    /// From frontmatter `tags` field
    pub tags: Vec<String>,
    /// Bullet points from the body (lines starting with `- `)
    pub facts: Vec<String>,
    /// Extracted `[[wikilinks]]` from the body
    pub links: Vec<String>,
    /// Unix timestamp (seconds) — from frontmatter `created` date
    pub created_at: i64,
    /// Unix timestamp (seconds) — from frontmatter `updated` date
    pub updated_at: i64,
    /// SHA-256 hex digest of the full file content
    pub content_hash: String,
}
```

Parsing splits frontmatter and body at the first pair of `---` fences; the body contributes `facts` (every line after a trimmed `- ` prefix) and `links` (via `extract_wikilinks`; see §5). `content_hash` is computed over the entire file content and is how the indexer decides whether a re-scanned file needs to be re-indexed.

`sanitize_title` guards every filename before it reaches the filesystem:

```rust
pub fn sanitize_title(title: &str) -> String
```

It strips `/ \ \0 : * ? " < > |`, removes every occurrence of `..`, and trims surrounding whitespace. This is applied in `NoteIndexer::write_note`, `append_to_note`, `rename_note`, and every action handler in `NoteManageTool` — LLM-generated titles cannot escape the agent's category directory.

## 5. Wikilinks

### 5.1 Supported syntax

`src/memory/notes/wikilink.rs` defines:

```rust
static WIKILINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\|]+)(?:\|([^\]]*))?\]\]").unwrap());
```

Both `[[target]]` and `[[target|alias]]` are matched. `extract_wikilinks` returns only the target part (capture group 1); `extract_wikilinks_with_alias` returns `(target, Option<alias>)` pairs. `rewrite_wikilinks(text, old, new)` replaces `[[old]]` → `[[new]]` and `[[old|alias]]` → `[[new|alias]]`, leaving unrelated links intact.

`extract_wikilinks(text: &str) -> Vec<String>` returns every bracketed target in document order. `rewrite_wikilinks(text, old, new) -> String` replaces every `[[old]]` (and `[[old|alias]]`) with `[[new]]` (preserving alias), leaving unrelated bracketed text alone.

### 5.2 Resolution algorithm

`resolve_wikilink` performs exact-match first, then unique-filename fallback:

1. **Contains `/` → exact path match.** If the link text contains `/`, it is treated as a full `category/filename` path. `NoteStore::get_note_index` is queried directly; hit returns the same string, miss returns `None`.
2. **No `/` → global filename search.** `NoteStore::find_by_filename` scans every category for notes whose filename equals the link text. If exactly one note matches, its full path is returned. If zero or more than one match, resolution returns `None` (ambiguous links are deliberately not guessed).

There is no fuzzy fallback and no case folding — matches are exact against the stored filename string. Cross-agent resolution is also disabled: every query is scoped by `agent_id`.

### 5.3 Persistence

Resolved links (really: the raw target strings extracted at index time) are persisted in the `notes_links` table (see §8) as `(agent_id, from_note, to_note)` triples, where `from_note` is the `category/filename` path of the source note and `to_note` is the raw wikilink target as it appeared in the source (before resolution).

## 6. `NoteIndexer` and the Write Pipeline

`NoteIndexer<S: NoteStore>` (`src/memory/notes/indexer.rs`) is generic over the store trait and owns both the filesystem root (`memory_dir: PathBuf`) and an `Arc<S>` handle. It is the only module that writes markdown files.

### 6.1 Write Flow

`index_file(agent_id, category, path)` is the per-file write path:

```text
+--------------------+     +---------------------+     +-------------------+
| read file contents | --> | sha256 → content    | --> | skip if existing  |
| (tokio::fs)        |     | hash compared to    |     | hash matches      |
+--------------------+     | notes_index row     |     +-------------------+
                           +---------------------+              |
                                                                v
                           +-----------------------------+   +-------+
                           | KnowledgeNote::from_markdown|-->| parse |
                           |   • split_frontmatter       |   | body  |
                           |   • extract_facts (- lines) |   +-------+
                           |   • extract_wikilinks       |       |
                           +-----------------------------+       v
                                                        +------------------+
                                                        | store.index_note |
                                                        |  upserts:        |
                                                        |   • notes_index  |
                                                        |   • notes_links  |
                                                        |   • notes_fts    |
                                                        +------------------+
```

Write entry points on `NoteIndexer`:

- `write_note(agent_id, category, &note)` — serializes `KnowledgeNote::to_markdown` and writes `{category}/{title}.md`. Used by `CompressionService` for fresh notes.
- `append_to_note(agent_id, "category/filename", &facts, &links)` — reads the existing file (or synthesizes an empty `KnowledgeNote`), extends `facts`, deduplicates `links`, bumps `updated_at`, writes, and re-indexes.
- `rename_note(agent_id, old_title, new_title)` — renames the file, scans every category dir for `[[old_title]]` references, calls `rewrite_wikilinks` on each match, and re-indexes every changed file.

#### Write-time semantic dedup (mem0-style) / 写入期语义去重

The compound ingestor (`DefaultCompoundIngestor::dedup_redirect_creates`) runs an
optional admission gate between planning and apply. For each planned
`PageOp::Create`, it embeds the candidate note's text and compares it — by
**exact cosine** (metric-independent of the store's vec0 distance) — against the
stored embeddings of the already-gathered related pages. When the nearest related
page meets `dedup_similarity_threshold`, the `Create` is rewritten into an
`Append` onto that page, so the genuinely-new facts merge into the existing note
instead of spawning a near-duplicate. The probe reuses the related set already
fetched for the planner (no extra search) and batch-embeds all candidates in a
single round-trip; it is purely additive and adds no LLM call (R7/R10-safe).

This mirrors mem0's additive-dedup strategy but **surpasses** it: mem0 drops the
duplicate, whereas Aleph absorbs its facts into the keeper and still layers the
richer offline dream-consolidation (`note_consolidate`) on top. Controlled by
`memory.compound_ingest.dedup_enabled` (default **false** → byte-identical
ingest) and `dedup_similarity_threshold` (default `0.92`); both are threaded
into `RelatedBudget`. Disabled, missing embeddings, or an empty related set all
degrade gracefully to the legacy create-everything behaviour (P7).

写入期语义去重：在规划与落盘之间增设可选准入闸门。对每个待建 `Create`，将候选笔记文本嵌入后与已召回的相关页面存量向量做**精确余弦**比较；当最近邻超过阈值时，把 `Create` 改写为对该页的 `Append`，让新事实并入既有笔记而非新建近似重复页。复用规划阶段已取的相关集、单次批量嵌入、零额外 LLM 调用。默认关闭（字节级一致），由 `dedup_enabled` / `dedup_similarity_threshold` 控制。相比 mem0 仅丢弃重复，Aleph 吸收其事实并叠加离线 dream 整合，能力更强。

### 6.2 Compression Scheduler

`CompressionScheduler` in `src/memory/compression/scheduler.rs` decides when to promote raw memories into notes. It tracks `pending_turns: AtomicU32` and `last_activity: Mutex<Instant>`, and `should_trigger_compression()` returns a `CompressionTrigger` enum variant (`None` | `IdleTimeout` | `TurnThreshold` | `SessionEnd` | `ManualRequest` | `BackgroundSchedule`). Turn-threshold trigger has priority over idle-timeout; idle trigger fires only when `pending_turns > 0`. Defaults: `idle_timeout_seconds = 300`, `turn_threshold = 20`, `background_interval_seconds = 3600`. When the scheduler fires, `CompressionService` (`src/memory/compression/service.rs`) consumes a batch from `raw_memories` (see `RAW_MEMORY.md` §7.1), extracts `NoteUpdate`s via the LLM extractor, and dispatches them into `NoteIndexer::write_note` (for `NoteAction::Create`) or `NoteIndexer::append_to_note` (for `Append` / `Update`). The scheduler implements `PostCompactCleanup` to reset its turn counter when compaction completes.

### 6.3 Cold-Start `full_rebuild()`

`full_rebuild(agent_id) -> IndexStats` scans `memory_dir/{agent_id}/{category}/*.md` for every category in `CATEGORY_DIRS`, parses each file through `KnowledgeNote::from_markdown`, and calls `NoteStore::index_note`. Files whose SHA-256 matches the existing `notes_index.content_hash` are skipped, yielding a cheap no-op on warm databases. The returned `IndexStats { indexed, skipped, errors }` makes the operation observable; parse failures are logged and counted rather than aborting the whole rebuild. This is the repair path if the SQLite index is deleted or goes out of sync with the markdown files.

## 7. `NoteStore` Trait

`src/memory/notes/store.rs` defines the persistence contract. Every method is scoped by `agent_id`:

| Method | Purpose |
|---|---|
| `index_note(&self, note: &KnowledgeNote, agent_id: &str, category: &str) -> Result<()>` | Upsert `notes_index` row, replace `notes_links` rows, and rebuild `notes_fts` content. |
| `remove_note_index(&self, path: &str, agent_id: &str) -> Result<()>` | Remove index / links / FTS entries by `category/filename` path. |
| `get_note_index(&self, path: &str, agent_id: &str) -> Result<Option<NoteIndexEntry>>` | Single-row lookup by path. |
| `list_notes(&self, agent_id: &str) -> Result<Vec<NoteIndexEntry>>` | All notes for an agent, most-recently-updated first. |
| `get_outgoing_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>>` | Raw wikilink targets emitted by this note. |
| `get_incoming_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>>` | Paths of notes that link to this filename. |
| `search_notes_fts(&self, query, agent_id, limit) -> Result<Vec<NoteIndexEntry>>` | FTS5 full-text search. |
| `get_graph_data(&self, agent_id, limit) -> Result<(Vec<NoteIndexEntry>, Vec<(String,String)>)>` | Top nodes + edges for graph visualization. |
| `get_neighbors(&self, center, agent_id, depth, limit) -> Result<(Vec<NoteIndexEntry>, Vec<(String,String)>)>` | BFS neighborhood around a node. |
| `count_all_notes(&self) -> Result<i64>` | Cross-agent note count for diagnostics. |
| `find_by_filename(&self, filename, agent_id) -> Result<Vec<String>>` | Used by wikilink resolution (§5.2) to find exact filename matches. |
| `upsert_embedding(&self, path, agent_id, embedding, dim) -> Result<()>` | Write or replace the embedding vector for a note. |
| `vector_search(&self, embedding, dim, agent_id, limit) -> Result<Vec<(String, f32)>>` | Vector similarity returning paths + scores. |
| `hybrid_search_notes(&self, embedding, query_text, agent_id, dim_hint, limit) -> Result<Vec<NoteSearchResult>>` | Vector + FTS fusion via RRF, returns full content. |
| `vector_search_notes_with_content(&self, embedding, agent_id, dim_hint, limit) -> Result<Vec<NoteSearchResult>>` | Vector-only search returning full content. |
| `get_notes_by_category(&self, agent_id, category, limit) -> Result<Vec<NoteIndexEntry>>` | Paginated category listing. |
| `get_embedding(&self, path, agent_id, dim_hint) -> Result<Option<Vec<f32>>>` | Read back a stored embedding. |

`NoteIndexEntry` is the lightweight row returned everywhere index metadata is enough:

```rust
pub struct NoteIndexEntry {
    pub path: String,        // "reference/rust-ownership"
    pub filename: String,    // "rust-ownership"
    pub agent_id: String,
    pub category: String,
    pub tags: Vec<String>,
    pub link_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
    pub content_hash: String,
}
```

## 8. SQLite Schema

DDL is defined in `src/memory/store/sqlite/schema.rs`. All statements use `CREATE ... IF NOT EXISTS` and `init_schema` is idempotent.

`notes_index`:

```sql
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
```

`notes_links`:

```sql
CREATE TABLE IF NOT EXISTS notes_links (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    from_note   TEXT NOT NULL,
    to_note     TEXT NOT NULL,
    to_raw      TEXT NOT NULL,
    relation    TEXT,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
```

`to_raw` stores the raw wikilink text as written in the source note (before resolution), and `relation` carries an optional typed relation label (from the `Relation` frontmatter field). Both were added when the note graph subsystem landed.

`notes_fts`:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    path,
    filename,
    content,
    agent_id UNINDEXED,
    tokenize='unicode61'
);
```

`notes_vec_map`:

```sql
CREATE TABLE IF NOT EXISTS notes_vec_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT NOT NULL,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    UNIQUE(agent_id, path)
);
CREATE INDEX IF NOT EXISTS idx_notes_vec_map_agent ON notes_vec_map(agent_id);
```

`notes_vec_768`, `notes_vec_1024`, `notes_vec_1536` — one `sqlite-vec` virtual table per embedding dimension, emitted by `init_notes_vec_tables` via:

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS notes_vec_{dim} USING vec0(
    rowid   INTEGER PRIMARY KEY,
    embedding float[{dim}]
);
```

`recall_signals` (note: `fact_id` was renamed to `note_path` by `migrate_recall_signals_note_path`):

```sql
CREATE TABLE IF NOT EXISTS recall_signals (
    id          TEXT PRIMARY KEY,
    note_path   TEXT NOT NULL,
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
    ON recall_signals(note_path, query_hash, day_bucket, channel);
CREATE INDEX IF NOT EXISTS idx_recall_note_path
    ON recall_signals(note_path);
CREATE INDEX IF NOT EXISTS idx_recall_day_bucket
    ON recall_signals(day_bucket);
```

`init_schema` also calls `drop_obsolete_facts_tables`, which runs `DROP TABLE IF EXISTS facts / facts_fts / facts_vec_768 / facts_vec_1024 / facts_vec_1536 / graph_nodes / graph_edges / memory_entities` as a one-time cleanup on existing databases.

## 9. Orientation Layer (`index.md` / `log.md` / `SCHEMA.md`)

Three generated files live in each agent's note directory and are managed by `src/memory/notes/orientation/`:

| File | Generator | Purpose |
|---|---|---|
| `SCHEMA.md` | `SchemaStore` (`orientation/schema.rs`) | Per-agent memory schema: tag taxonomy, page thresholds, update policy |
| `index.md` | `IndexMdGenerator` (`orientation/index_md.rs`) | Category-grouped note listing, rebuilt by `FsNoteOrientation::rebuild_index` |
| `log.md` | `LogMdWriter` (`orientation/log_md.rs`) | Append-only audit log of ingest, query, lint, and session events |

`FsNoteOrientation::bootstrap` creates all three on first use. `IndexRefresherStage` (wired into the ingest path) calls `refresh_index_after_ingest` after each batch so `index.md` stays current.

`read_snapshot` assembles these three files into an `OrientationSnapshot` that is injected into agent prompts: `schema_text` comes from `SchemaDoc::compact_for_prompt()` (Tag Taxonomy + Page Thresholds + Update Policy sections only, to reduce prompt tokens); `index_text` is the full `index.md`; `recent_log_tail` is the last 20 log entries.

> The `src/wiki/` directory and its `WikiGitManager` / `generate_index_content` were removed. All orientation functionality now lives in `src/memory/notes/orientation/`.

## 10. Skills as Notes

The `skill/` directory under `memory/note/{agent_id}/` receives skill-category notes whose frontmatter carries `scope: persona` (§3.3). These markdown files are the distilled, human-readable form of persona-scoped skill knowledge and travel through the same indexer, wikilink graph, FTS, and vector search as every other note category.

A distinct subsystem lives in `src/skill/` — `SkillSystem`, `SkillId`, `PromptScope`, and the `skill_manage` tool (`src/builtin_tools/skill_manage.rs`) — that deals with **extension skills** (external, installable skill manifests, toggled via `skill_manage(skill_id, enabled, scope)` with scope values `system` | `tool` | `standalone` | `disabled`). That system is orthogonal to skill-category notes: `skill_manage` never touches the notes filesystem, and `note_manage(category='skill', ...)` never touches the extension registry. Keeping the distinction sharp avoids confusing persona notes (markdown under `memory/note/{agent}/skill/`) with installed skill extensions (configuration under `~/.aleph/skills/`).

## 11. `note_manage` Tool

`NoteManageTool` (`src/builtin_tools/note_manage.rs`) is the unified LLM-facing CRUD surface for every note category. Action enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NoteManageAction {
    Create,   // fails if filename already exists
    Update,   // replace body of existing note
    Append,   // extend facts + links on existing or new note
    Query,    // FTS5 search across indexed notes
    List,     // list notes, optionally filtered by category
    Delete,   // remove file + index entry
}
```

Args:

```rust
pub struct NoteManageArgs {
    pub action: NoteManageAction,
    pub category: Option<String>,   // required for create/update/append/delete
    pub filename: Option<String>,   // required for create/update/append/delete
    pub title: Option<String>,      // required for create
    pub content: Option<String>,    // required for create/update body
    pub facts: Option<Vec<String>>, // for append
    pub links: Option<Vec<String>>, // wikilink targets
    pub tags: Option<Vec<String>>,
    pub query: Option<String>,      // required for query
    pub limit: Option<usize>,       // default: 20 (query), 100 (list)
}
```

Every action runs `validate_category(category)` against the tool's own copy of the category list (which matches `CATEGORY_DIRS` plus the four `subagent-*` categories, see lines 22–38 of `note_manage.rs`). Unknown categories are rejected with a listing of valid values. Create and update both run input through `sanitize_title` before the filename is ever joined into a path.

Frontmatter is produced by `KnowledgeNote::to_markdown` (the single real write path). The YAML fields written are those described in §3.

**Deprecation status (verified by grep):**

- `src/builtin_tools/skill_manage.rs` is still present and active — but it configures **extension skills** (§10), not skill-category notes. The module exists; it does not overlap with `note_manage`.
- `wiki_manage` is **removed**. The `src/wiki/` directory has been deleted; orientation functionality moved to `src/memory/notes/orientation/` (§9).

## 12. Event Sourcing

Every mutation to a note is captured as an immutable `MemoryEvent` wrapped in a `MemoryEventEnvelope`. This provides an audit trail, enables time-travel queries, and powers `explain_fact`.

### 12.1 Event Types

Events are classified as **Skeleton** (structural mutations, persisted immediately) or **Pulse** (high-frequency observations, buffered before persist):

| Event | Type | Payload |
|---|---|---|
| `NoteCreated` | Skeleton | `note_path, content, note_type, path, namespace, agent, source, source_memory_ids` |
| `NoteContentUpdated` | Skeleton | `note_path, old_content, new_content, reason` |
| `NoteMetadataUpdated` | Skeleton | `note_path, field, old_value, new_value` |
| `NoteAccessed` | Pulse | `note_path, query, relevance_score, used_in_response, new_access_count` |
| `NoteInvalidated` | Skeleton | `note_path, reason, actor` |
| `NoteRestored` | Skeleton | `note_path` |
| `NoteDeleted` | Skeleton | `note_path, reason` |
| `NoteConsolidated` | Skeleton | `note_path, source_note_paths, consolidated_content` |
| `NoteMigrated` | Skeleton | `note_path, snapshot` |

Pre-Phase-R2 events written with the legacy `Fact*` variant names and the
`fact_id` payload field still deserialize correctly because every variant
carries `#[serde(alias = "Fact...")]` and every `note_path` field carries
`#[serde(alias = "fact_id")]`.

### 12.2 Commands

Commands in `src/memory/events/commands.rs`:

- `CreateNoteCommand` — emits `MemoryEvent::NoteCreated` at seq 1.
- `UpdateContentCommand` — rebuilds current content via `EventProjector::fold_events_to_note`, then emits `NoteContentUpdated`.
- `InvalidateNoteCommand` — soft delete; emits `NoteInvalidated`.
- `RestoreNoteCommand` — revives an invalidated note; emits `NoteRestored`.
- `RecordNoteAccessCommand` — emits `NoteAccessed` with `EventActor::Agent`.
- `ConsolidateCommand` — emits `NoteConsolidated`.
- `DeleteNoteCommand` — hard delete; emits `NoteDeleted`.

> The former `ApplyDecayCommand` (bulk `StrengthDecayed` batch) and `TierTransitioned` event were removed as part of the memory sovereignty cleanup. Strength/tier/confidence are no longer part of the note model; aging and salience are expressed through retrieval scoring stages and prompt-layer judgement instead of persisted per-note fields.

### 12.3 Projection

`MemoryCommandHandler` in `src/memory/events/handler.rs` projects each event into the notes layer:

1. Append the `MemoryEventEnvelope` to the SQLite event log (`append_memory_event`).
2. Fold all events for the affected note path into a projected note via `EventProjector::fold_events_to_note`.
3. On a present projection, write a `KnowledgeNote` via `NoteIndexer::write_note`, then `index_note`.
4. On a `None` projection (note deleted), scan `CATEGORY_DIRS` for the file and remove both file and index entry.

This is the "notes dual-write": the event log remains the audit-and-explain source of truth while markdown files are the primary read surface. See `RETRIEVAL.md` §12 for how the event log powers `explain_fact` and time-travel queries.

## 13. Namespace Scoping

`src/memory/namespace.rs` declares:

```rust
pub enum NamespaceScope {
    Owner,
    Guest(String),
    Shared,
}
```

`NamespaceScope::to_sql_filter` returns the WHERE clause fragment (`1=1` for `Owner`, `namespace = ?` with bind for `Guest` and `Shared`), and `to_namespace_value` returns the column value written on insert (`"owner"`, `"guest:{id}"`, or `"shared"`). `from_auth_context(role, guest_id)` maps `DeviceRole::Operator` → `Owner` and `DeviceRole::Node` → `Guest(guest_id)` (required; returns an error if missing). The `recall_signals` table carries a `namespace` column defaulting to `'owner'`; per-user memory access filtering happens through this enum.

Note scoping itself is orthogonal: notes are keyed by `agent_id` in the filesystem path and the `notes_index` primary key. `agent_id` and `namespace` are independent axes — the former partitions the markdown filesystem, the latter partitions rows visible to a given authenticated caller.

## 14. Graph Subsystem (upcoming)

`src/memory/notes/graph/` is the planned home for the note protocol-graph subsystem — a typed, queryable knowledge graph built on top of the existing `notes_links` table (§8). The subsystem will provide structured graph traversal (BFS/DFS), relation-typed edge queries, and graph-aware retrieval to complement the existing FTS and vector search paths. Design documentation lives in `docs/reference/memory/notes-graph-spec.md` and `notes-graph-plan.md` (Phase 0–3 implementation roadmap).

## See Also

- [Raw Memory (L0)](RAW_MEMORY.md) §7.1 — `CompressionService` reads unprocessed `raw_memories` and writes notes through the §6 pipeline.
- [Dream Daemon](DREAM_DAEMON.md) — the dream pipeline's subject is the note corpus: drift detection, decay, and lint stages all operate on the markdown + index layer described here.
- [Retrieval](RETRIEVAL.md) §1 — how notes are queried (FTS, vector, hybrid, graph) by the retrieval pipeline.
