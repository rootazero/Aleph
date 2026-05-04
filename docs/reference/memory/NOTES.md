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

### 3.1 Common fields (default template)

Notes parsed by `KnowledgeNote::from_markdown` (`src/memory/notes/note.rs`) use this frontmatter shape, emitted by the default branch of `frontmatter_template` in `src/builtin_tools/note_manage.rs`:

```yaml
---
category: {category}
tags: {tags_json}
created: "{YYYY-MM-DD}"
updated: "{YYYY-MM-DD}"
---
```

The parser's `Frontmatter` struct declares every field `#[serde(default)]`, so missing values fall through to empty strings / empty vectors rather than erroring. Dates are parsed as `YYYY-MM-DD` (UTC midnight) by `parse_date_to_unix`; empty / missing dates yield `0`.

### 3.2 Reference-specific (`category = "reference"`)

```yaml
---
title: {title}
aliases: []
tags: {tags_json}
sources: []
created: "{YYYY-MM-DD}"
updated: "{YYYY-MM-DD}"
---
```

`title`, `aliases`, and `sources` are reference-specific extensions emitted by `frontmatter_template("reference", ...)`. The current `KnowledgeNote` parser does **not** bind `title`, `aliases`, or `sources` into struct fields — they are preserved on disk but not surfaced in the in-memory `KnowledgeNote` beyond the filename-derived `title` field and the common `tags` vector. Treat them as forward-compatible metadata.

### 3.3 Skill-specific (`category = "skill"`)

```yaml
---
title: {title}
scope: persona
tags: {tags_json}
created: "{YYYY-MM-DD}"
updated: "{YYYY-MM-DD}"
---
```

The literal string `scope: persona` is emitted verbatim by `frontmatter_template("skill", ...)`. This marks skill notes as agent-persona-scoped content so downstream consumers can distinguish them from regular knowledge entries. As with reference, the `scope` field is preserved in the file but not parsed into the `KnowledgeNote` struct.

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
    LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());
```

Only `[[target]]` is matched. The `[[target|alias]]` pipe-alias form is **not** recognized by the regex — if a note body contains `[[foo|Foo]]`, the entire string `foo|Foo` becomes the link target. Documented here as the code's truth, not as a limitation; if piped aliases matter, fix the regex before relying on them.

`extract_wikilinks(text: &str) -> Vec<String>` returns every bracketed target in document order. `rewrite_wikilinks(text, old, new) -> String` replaces every `[[old]]` with `[[new]]`, leaving unrelated bracketed text alone.

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
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
```

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

## 9. Reference Post-Write Hooks

`src/wiki/git.rs` defines `WikiGitManager` — `ensure_repo` runs `git init`, `ensure_agent_dir` creates per-agent directories, and `commit_changes` auto-commits on page changes. `src/wiki/index.rs` defines `generate_index_content` (builds a markdown table of every wiki page) and `write_index` (writes `index.md` into the agent's wiki directory).

**However**, nothing in the current write path invokes either module. A codebase-wide grep for `WikiGitManager`, `wiki_git`, `generate_index_content`, and `write_index` returns only `src/wiki/git.rs` and `src/wiki/index.rs` themselves — there are no call sites in `src/builtin_tools/note_manage.rs`, `src/memory/notes/indexer.rs`, or `src/memory/compression/service.rs`. When a `reference`-category note is created or updated through `note_manage` or the compression pipeline, the markdown file is written and indexed normally, but **no git commit is produced and no `index.md` is regenerated**. The wiki-specific machinery exists as a dormant module; wiring it to the note write path is future work.

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

Category-specific frontmatter comes from `frontmatter_template(category, title, tags)` at the bottom of `note_manage.rs`. The three branches (`reference`, `skill`, default) produce the YAML shown in §3.

**Deprecation status (verified by grep):**

- `src/builtin_tools/skill_manage.rs` is still present and active — but it configures **extension skills** (§10), not skill-category notes. The module exists; it does not overlap with `note_manage`.
- `wiki_manage` is **removed**. `src/executor/builtin_registry/registry.rs` line 928 contains the single remaining reference, a redirect: `"wiki_manage has been removed. Use note_manage instead."`. The argument-builder struct at `src/wiki/tools/manage.rs` is vestigial scaffolding with no live callers.

## 12. Event Sourcing

Commands in `src/memory/events/commands.rs`:

- `CreateNoteCommand` — emits `MemoryEvent::NoteCreated` at seq 1.
- `UpdateContentCommand` — rebuilds current content via `EventProjector::fold_events_to_note`, then emits `NoteContentUpdated { old_content, new_content, reason }`.
- `InvalidateNoteCommand` — soft delete; emits `NoteInvalidated { reason }`.
- `RestoreNoteCommand` — revives an invalidated note; emits `NoteRestored { new_strength }`.
- `RecordNoteAccessCommand` — emits `NoteAccessed { query, relevance_score, used_in_response, new_access_count }` with `EventActor::Agent`.
- `ConsolidateCommand` — emits `NoteConsolidated { source_note_paths, consolidated_content }`.
- `DeleteNoteCommand` — hard delete; emits `NoteDeleted { reason }`.

> The former `ApplyDecayCommand` (bulk `StrengthDecayed` batch) and `TierTransitioned` event were removed as part of the memory sovereignty cleanup. Strength/tier/confidence are no longer part of the note model; aging and salience are expressed through retrieval scoring stages and prompt-layer judgement instead of persisted per-note fields.

Pre-Phase-R2 events written with the legacy `Fact*` variant names and the
`fact_id` payload field still deserialize correctly because every variant
carries `#[serde(alias = "Fact...")]` and every `note_path` field carries
`#[serde(alias = "fact_id")]`. Likewise `source_note_paths` carries
`#[serde(alias = "source_fact_ids")]`. Writes only emit the new names.

The SQL column `fact_id` on the `memory_events` table is preserved as schema
metadata for audit-row stability — `MemoryEventEnvelope.fact_id` mirrors the
inner event's `note_path` and only exists at the storage edge. Likewise the
`MemoryEvent::fact_id()` accessor is retained as public API and returns the
underlying `note_path` value.

`MemoryCommandHandler` in `src/memory/events/handler.rs` projects each event
into the notes layer via `project_to_notes`:

1. Append the `MemoryEventEnvelope` to the SQLite event log (`append_memory_event`).
2. Fold all events for the affected note path into a projected note via `EventProjector::fold_events_to_note`.
3. On a present projection, write a `KnowledgeNote { title: sanitize_title(note_path), category: note.note_type.to_category_dir(), facts: [note.content], ... }` via `NoteIndexer::write_note`, then `index_note`.
4. On a `None` projection (note deleted), scan `CATEGORY_DIRS` for a file named `{note_path}.md` and remove both file and index entry.

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

## See Also

- [Raw Memory (L0)](RAW_MEMORY.md) §7.1 — `CompressionService` reads unprocessed `raw_memories` and writes notes through the §6 pipeline.
- [Dream Daemon](DREAM_DAEMON.md) — the dream pipeline's subject is the note corpus: drift detection, decay, and lint stages all operate on the markdown + index layer described here.
- [Retrieval](RETRIEVAL.md) §1 — how notes are queried (FTS, vector, hybrid, graph) by the retrieval pipeline.
