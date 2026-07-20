# Memory Documentation Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure `docs/reference/MEMORY_SYSTEM.md` (899 lines, ~60% obsolete) into a 5-document hierarchy matching the post-facts-table architecture (L0 raw_memories + L1 markdown notes + Dream pipeline + NoteFactRetrieval).

**Architecture:** Overview entry (`MEMORY_SYSTEM.md`) + four domain-scoped subdocs under `docs/reference/memory/`. Fact-extraction-and-reorganize approach (spec §6 B): verify every trait signature, schema column, and path against `src/memory/` at write time; port still-valid fragments (embedding presets, TOML config, troubleshooting) from the old doc; discard obsolete architecture (facts table, ACMA tier/strength, VFS, old Dream stage names).

**Tech Stack:** Markdown documentation. No code changes. Verification via `grep` + source reading.

**Spec:** `docs/superpowers/specs/2026-04-12-memory-docs-restructure-design.md`

---

## File Structure

Files this plan creates or modifies:

- **Modify:** `docs/reference/MEMORY_SYSTEM.md` — replace 899-line old doc with ~220-line overview + navigation
- **Create:** `docs/reference/memory/NOTES.md` — L1 notes layer (~420 lines)
- **Create:** `docs/reference/memory/RAW_MEMORY.md` — L0 ephemeral layer (~210 lines)
- **Create:** `docs/reference/memory/DREAM_DAEMON.md` — offline consolidation (~360 lines)
- **Create:** `docs/reference/memory/RETRIEVAL.md` — retrieval + scoring + tools (~420 lines)

All five documents share the style in spec §5:
- First line: one-sentence purpose.
- Architecture diagram near top (ASCII box-drawing).
- Code references in `src/path:line` format only for stable anchors (trait/struct definitions).
- No prose repetition — forward-link to the doc that owns a topic.
- English narrative (`docs/reference/` convention).

---

## Task Order Rationale

Subdocs are written **before** the overview so the overview can link to real files, not placeholders. Within subdocs, order is bottom-up by dependency:

1. RAW_MEMORY (no cross-refs to other subdocs)
2. NOTES (links to RAW_MEMORY)
3. DREAM_DAEMON (links to NOTES)
4. RETRIEVAL (links to NOTES, DREAM_DAEMON)
5. MEMORY_SYSTEM overview (links to all four)
6. Cross-link + fact audit
7. Remove orphan content from old doc already covered

---

## Task 1: Scaffold Skeleton Files

Create the 5 files with top-level section headers only. This locks the structure in one atomic commit and surfaces naming issues before any prose is written.

**Files:**
- Modify: `docs/reference/MEMORY_SYSTEM.md` (full replace)
- Create: `docs/reference/memory/NOTES.md`
- Create: `docs/reference/memory/RAW_MEMORY.md`
- Create: `docs/reference/memory/DREAM_DAEMON.md`
- Create: `docs/reference/memory/RETRIEVAL.md`

- [ ] **Step 1: Back up old doc as a working reference**

```bash
cp docs/reference/MEMORY_SYSTEM.md /tmp/MEMORY_SYSTEM.old.md
```

The old doc is the source for "KEEP" fragments in spec §4. Keep `/tmp/MEMORY_SYSTEM.old.md` readable for the whole plan; we delete the actual repo copy in Task 6.

- [ ] **Step 2: Create `docs/reference/memory/` directory and scaffold RAW_MEMORY.md**

```bash
mkdir -p docs/reference/memory
```

Write `docs/reference/memory/RAW_MEMORY.md`:

```markdown
# Raw Memory (L0)

> Short-lived, high-volume conversation and attachment data consumed by the compression pipeline and session-context restore.

## 1. Role

## 2. When to Use raw_memories vs Notes

## 3. `raw_memories` Schema

## 4. `RawMemory` + `RawMemorySource`

## 5. `RawMemoryStore` Trait

## 6. Writers

### 6.1 SessionCompactor

### 6.2 TranscriptIndexer

### 6.3 Gateway Media Pipeline (attachment_text)

## 7. Readers

### 7.1 CompressionService

### 7.2 recall_context

### 7.3 session_summary_source

## 8. Lifecycle

## See Also
```

- [ ] **Step 3: Scaffold NOTES.md**

Write `docs/reference/memory/NOTES.md`:

```markdown
# Knowledge Notes (L1)

> Markdown-first persistent knowledge. Each note is one `.md` file; SQLite tables are rebuildable indexes.

## 1. Overview

## 2. Filesystem Layout

## 3. Frontmatter Schema

## 4. `KnowledgeNote` Data Model

## 5. Wikilinks

## 6. `NoteIndexer` and the Write Pipeline

### 6.1 Write Flow

### 6.2 Compression Scheduler

### 6.3 Cold-Start `full_rebuild()`

## 7. `NoteStore` Trait

## 8. SQLite Schema

## 9. Wiki Post-Write Hooks

## 10. Skills as Notes

## 11. `note_manage` Tool

## 12. Event Sourcing

## 13. Namespace Scoping

## See Also
```

- [ ] **Step 4: Scaffold DREAM_DAEMON.md**

Write `docs/reference/memory/DREAM_DAEMON.md`:

```markdown
# Dream Daemon

> Offline consolidation of the notes layer during user-idle windows.

## 1. Purpose

## 2. Scheduling

## 3. `DreamGate`

## 4. Core Types

### 4.1 `DreamContext`

### 4.2 `DreamPipeline`

### 4.3 `DreamStage` Trait

## 5. Stages

### 5.1 NoteConsolidate

### 5.2 NoteDrift

### 5.3 NoteSynthesis (Weekly)

### 5.4 NoteLint

### 5.5 NoteDecay

### 5.6 DailyDigest

## 6. Pipelines

### 6.1 Daily (5 Stages)

### 6.2 Weekly (6 Stages)

## 7. `DreamReport` Schema

## 8. Persistence

## 9. Safety

## 10. Configuration

## See Also
```

- [ ] **Step 5: Scaffold RETRIEVAL.md**

Write `docs/reference/memory/RETRIEVAL.md`:

```markdown
# Memory Retrieval

> `NoteFactRetrieval` — hybrid search over notes, scoring, context assembly, tools, and audit.

## 1. Entry Points

## 2. Hybrid Search Algorithm

## 3. Bridge to Legacy Types

## 4. Scoring Pipeline

### 4.1 Stages Overview

### 4.2 `importance_weight` (and ValueEstimator)

### 4.3 `cosine_rerank`

### 4.4 `mmr_diversity`

### 4.5 `time_decay`

### 4.6 `recency_boost`

### 4.7 `length_normalization`

### 4.8 `hard_min_score`

## 5. Reranker (Optional, Not Wired)

## 6. Query Expander (Optional, Not Wired)

## 7. Embedding Provider

## 8. Context Assembly

### 8.1 `ContextComposer`

### 8.2 `ContextComptroller`

## 9. `AiMemoryRetriever`

## 10. RippleTask

## 11. Memory Tools

### 11.1 `memory_search`

### 11.2 `memory_browse`

### 11.3 `memory_explore`

### 11.4 `recall_context`

## 12. Audit and Explainability

## 13. Cortex (Independent Subsystem)

## Appendix: Retrieval Tuning Tips

## See Also
```

- [ ] **Step 6: Scaffold the new `MEMORY_SYSTEM.md` overview (full replace)**

Overwrite `docs/reference/MEMORY_SYSTEM.md`:

```markdown
# Memory System

> Persistent knowledge for LLM conversations via markdown-first notes, ephemeral raw memories, and offline consolidation.

## 1. Purpose

## 2. Design Principles

## 3. Two-Layer Data Model

## 4. Storage Traits

## 5. Scratchpad

## 6. Interfaces

## 7. TOML Configuration

## 8. Subdocument Navigation

## 9. Troubleshooting
```

- [ ] **Step 7: Verify files compile as valid markdown**

Run: `find docs/reference -name "*.md" -newer /tmp -print`

Expected: the 5 edited files listed.

- [ ] **Step 8: Commit scaffold**

```bash
git add docs/reference/MEMORY_SYSTEM.md docs/reference/memory/
git commit -m "docs(memory): scaffold 5-document restructure skeletons

Section-header-only scaffolds for the new memory documentation
hierarchy. Content in subsequent commits.

Spec: docs/superpowers/specs/2026-04-12-memory-docs-restructure-design.md"
```

---

## Task 2: Write `memory/RAW_MEMORY.md`

The simplest of the four subdocs. No cross-refs to other new docs. Written first to establish style.

**Files:**
- Modify: `docs/reference/memory/RAW_MEMORY.md`
- Read (for verification): `src/memory/store/raw_memory.rs`, `src/memory/store/sqlite/raw_memories.rs`, `src/memory/session_compactor/mod.rs`, `src/memory/transcript_indexer/mod.rs`, `src/memory/compression/service.rs`, `src/builtin_tools/recall_context.rs`

- [ ] **Step 1: Verify the schema**

Run: `grep -n "CREATE TABLE.*raw_memories\|CREATE INDEX.*raw" src/memory/store/sqlite/**/*.rs`

Read the exact DDL from the matched file(s). Write the §3 `raw_memories` schema subsection using the real column list, types, indexes, and `WHERE is_processed = 0` partial-index clause. Do not paraphrase — copy faithfully.

- [ ] **Step 2: Verify `RawMemory` and `RawMemorySource`**

Read `src/memory/store/raw_memory.rs`. Extract the struct + enum definitions verbatim into §4 as a fenced rust block. Include doc comments. Do not trim fields.

- [ ] **Step 3: Verify `RawMemoryStore` trait**

From the same file, extract every `async fn` signature. Write §5 as a method table:

| Method | Purpose |
|---|---|

One row per signature. Keep the method column as the full Rust signature.

- [ ] **Step 4: Verify writers**

For each of §6.1–§6.3:

- §6.1 `SessionCompactor`: read `src/memory/session_compactor/mod.rs`. Document the d0/d1/d2 layer semantics (what each represents), when `insert_raw_memory` is called, which `source` value is written (`"session_compressed"`).
- §6.2 `TranscriptIndexer`: read `src/memory/transcript_indexer/mod.rs`. Document the semantic-chunking inputs, chunk size/overlap defaults (read actual values from `TranscriptIndexerConfig`), `source = "transcript"`.
- §6.3 Gateway media pipeline: grep `grep -rn "attachment_text" src/gateway/ src/memory/` to find the exact write site. Document where the field is populated and what kinds of attachments (PDF/Word/image text) flow in.

Each subsection: 1 short paragraph + the concrete config keys or constants.

- [ ] **Step 5: Verify readers**

- §7.1 `CompressionService`: grep `grep -n "get_unprocessed_raw_memories\|mark_as_processed" src/memory/compression/`. Document the batch consumption loop and the marking behavior.
- §7.2 `recall_context`: grep `grep -n "get_raw_by_path_prefix" src/builtin_tools/`. Document the path-prefix query and the `aleph://session/{id}/raw/` path convention.
- §7.3 `session_summary_source`: grep `grep -rn "session_summary_source" src/`. Document the session-context-restore read path.

- [ ] **Step 6: Write §1, §2, §8**

§1 Role: one paragraph describing "raw" = short-lived, session-scoped, high-volume. The anti-example is also useful: "why not a markdown file per row? filesystem I/O + human-unreadable noise".

§2 Decision table:

| Data kind | Storage | Why |
|---|---|---|
| Conversation turn transcripts | raw_memories | … |
| Session d0/d1/d2 summaries | raw_memories | … |
| Attachment extracted text | raw_memories | … |
| User preferences / decisions | notes | … |
| Synthesized insights | notes | … |

§8 Lifecycle: read the config + any TTL constants. If no hard TTL exists, document the actual: "rows persist until `is_processed=1` flip; there is no time-based eviction currently; cleanup is manual/external". Verify by `grep -n "DELETE FROM raw_memories\|TTL\|retention" src/memory/`.

- [ ] **Step 7: Add "See Also" and forward-links**

Point to: `NOTES.md` for where processed raw data lands, `RETRIEVAL.md` §11.4 for `recall_context` tool.

- [ ] **Step 8: Fact-check scan**

Run:

```bash
grep -n "facts table\|MemoryStore::\|aleph://\|graph_nodes\|FactRetrieval" docs/reference/memory/RAW_MEMORY.md
```

Expected: zero matches. If any appear, rewrite — raw_memories is post-facts-table, and session data uses real filesystem paths (none of these concepts apply).

- [ ] **Step 9: Line count check**

Run: `wc -l docs/reference/memory/RAW_MEMORY.md`

Expected: 160–260 lines (target 210 ±50). If over, cut examples or merge subsections.

- [ ] **Step 10: Commit**

```bash
git add docs/reference/memory/RAW_MEMORY.md
git commit -m "docs(memory): write RAW_MEMORY.md — L0 ephemeral layer

raw_memories schema, RawMemoryStore trait, writers (SessionCompactor
/ TranscriptIndexer / media pipeline), readers (CompressionService
/ recall_context / session_summary_source), lifecycle."
```

---

## Task 3: Write `memory/NOTES.md`

Largest of the four subdocs. Depends on RAW_MEMORY (forward-links for the write-side input).

**Files:**
- Modify: `docs/reference/memory/NOTES.md`
- Read: `src/memory/notes/note.rs`, `src/memory/notes/indexer.rs`, `src/memory/notes/store.rs`, `src/memory/notes/wikilink.rs`, `src/memory/notes/extractor.rs`, `src/memory/store/sqlite/notes.rs`, `src/memory/store/sqlite/schema.rs`, `src/memory/compression/service.rs`, `src/memory/compression/scheduler.rs`, `src/memory/events/handler.rs`, `src/memory/events/commands.rs`, `src/memory/namespace.rs`, `src/builtin_tools/note_manage.rs` (verify exact filename first)

- [ ] **Step 1: Inventory categories**

Run: `grep -n "CATEGORY_DIRS\|pub const.*CATEGORY" src/memory/notes/indexer.rs`

Copy the exact category list into §2 as a directory tree. Example format:

```
~/.aleph/memory/note/
└── {agent_id}/
    ├── preference/*.md
    ├── plan/*.md
    ├── learning/*.md
    ├── skill/*.md
    ├── wiki/*.md
    ├── tool/*.md
    ├── synthesis/*.md
    ├── archive/*.md
    └── other/*.md
```

Adjust to the real list emitted by grep.

- [ ] **Step 2: Verify frontmatter schema**

Read `src/memory/notes/extractor.rs` for parse logic and `src/memory/notes/indexer.rs` for write-side templates. Document §3 as three subsections:

- Common fields (category/tags/created/updated)
- Wiki-specific (title/aliases/sources)
- Skill-specific (title/scope)

Pull the literal YAML template strings from the code.

- [ ] **Step 3: Verify `KnowledgeNote` struct**

Read `src/memory/notes/note.rs`. Extract the `KnowledgeNote` struct and `sanitize_title` function signature into §4. Include every field with its doc comment.

- [ ] **Step 4: Verify wikilinks**

Read `src/memory/notes/wikilink.rs`. Document in §5:

- Supported syntaxes: `[[target]]`, `[[target|alias]]` (confirm against `extract_wikilinks`).
- Resolution algorithm: read `resolve_wikilink` line-by-line and describe the precedence (exact filename → agent-scoped → fuzzy?). If the code's actual precedence differs from the initial spec description, document the code's truth.
- Persistence: resolved links go into `notes_links` table.

- [ ] **Step 5: Verify `NoteIndexer`**

Read `src/memory/notes/indexer.rs`. Document in §6:

- §6.1 Write Flow: write markdown → parse frontmatter → extract wikilinks → upsert index/links/fts/vec. Include a simple ASCII flow diagram.
- §6.2 Compression Scheduler: read `src/memory/compression/scheduler.rs`. One paragraph on idle-trigger + batch consumption from raw_memories → CompressionService → NoteIndexer.
- §6.3 `full_rebuild()`: one paragraph on cold-start scan of `memory/note/` and index repopulation.

- [ ] **Step 6: Verify `NoteStore` trait**

Read `src/memory/notes/store.rs`. Write §7 as a method table (signature + purpose). Include every method:
`index_note`, `remove_note_index`, `get_note_index`, `list_notes`, `get_outgoing_links`, `get_incoming_links`, `search_notes_fts`, `hybrid_search_notes`, `vector_search_notes_with_content`, `get_notes_by_category`, `count_all_notes`, `find_by_filename`, `get_graph_data`, `get_neighbors`.

If the actual trait has more/fewer methods, match the code. Do not invent.

- [ ] **Step 7: Verify SQLite schema**

Read `src/memory/store/sqlite/schema.rs` and `src/memory/store/sqlite/notes.rs`. In §8, include DDL for each table as a single code block per table:

- `notes_index`
- `notes_links`
- `notes_fts`
- `notes_vec_map`
- `notes_vec_768`, `notes_vec_1024`, `notes_vec_1536`
- `recall_signals` (note: after the rename, column is `note_path`)

Copy the real CREATE statements. If the DDL lives in a migration, use that source.

- [ ] **Step 8: Verify wiki hooks**

Grep: `grep -rn "category == \"wiki\"\|wiki_git\|index\\.md" src/memory/ src/builtin_tools/`

Document in §9: git commit on change, `index.md` regeneration. Include the actual function names/file paths that implement this.

If wiki hooks live in `note_manage` / `wiki_manage`, cite that location.

- [ ] **Step 9: Write §10 Skills as notes**

One paragraph: the `skill/` category uses `scope: persona` in frontmatter, is loaded at agent init as persona-scoped context. Point at the actual consumer (grep `grep -rn "category.*skill" src/`).

- [ ] **Step 10: Verify `note_manage` tool**

Run: `find src/builtin_tools -name "note_manage*" -o -name "*note_manage*"` and `grep -rn "note_manage" src/tools/ src/builtin_tools/`.

Read the tool definition. In §11:

- The action enum (Create / Update / Append / Query / List / Delete — verify match)
- The args struct
- Category-specific frontmatter templates (point back to §3)
- Deprecation note on `skill_manage` / `wiki_manage` if they are still in-tree

- [ ] **Step 11: Verify event sourcing**

Read `src/memory/events/commands.rs` and `src/memory/events/handler.rs`. In §12:

- Command types (`CreateFactCommand`, `UpdateContentCommand`, `InvalidateFactCommand`, `DeleteFactCommand`, `RecordAccessCommand`, `RestoreFactCommand`, `ApplyDecayCommand`, `ConsolidateCommand`)
- Handler behavior: writes notes + emits events. Note: command names still contain "Fact" for historical reasons but targets are notes.
- Forward-link to `RETRIEVAL.md` §12 for audit/explain usage.

- [ ] **Step 12: Verify namespace**

Read `src/memory/namespace.rs`. In §13, one paragraph: `NamespaceScope` enum + `agent_id` isolation. Quote the enum variants.

- [ ] **Step 13: Add §1 overview + See Also**

§1: one paragraph referencing the three claims: markdown is source of truth, SQLite is rebuildable index, per-agent isolation.

See Also: link to RAW_MEMORY.md §7.1 (CompressionService reads raw, writes notes), DREAM_DAEMON.md (notes are the dream's subject), RETRIEVAL.md §1 (how notes are queried).

- [ ] **Step 14: Fact-check scan**

```bash
grep -n "facts table\|MemoryStore::\|aleph://\|graph_nodes\|graph_edges\|VFS\|l1_generator" docs/reference/memory/NOTES.md
```

Expected: zero matches in production-code claims. Migration-history mentions are fine only if clearly labeled ("formerly…").

Also:

```bash
grep -n "WikiIngestStage\|TunnelDiscoveryStage" docs/reference/memory/NOTES.md
```

Expected: zero matches.

- [ ] **Step 15: Line count check**

Run: `wc -l docs/reference/memory/NOTES.md`

Expected: 370–470 lines (target 420 ±50).

- [ ] **Step 16: Commit**

```bash
git add docs/reference/memory/NOTES.md
git commit -m "docs(memory): write NOTES.md — L1 markdown-first knowledge

Filesystem layout, frontmatter, KnowledgeNote, wikilinks, NoteIndexer,
NoteStore trait, SQLite schema, wiki/skill specialization, note_manage
tool, event sourcing writers, namespace scoping."
```

---

## Task 4: Write `memory/DREAM_DAEMON.md`

Depends on NOTES. Stage descriptions cite notes-layer operations.

**Files:**
- Modify: `docs/reference/memory/DREAM_DAEMON.md`
- Read: `src/memory/dreaming/mod.rs`, `src/memory/dreaming/gate.rs`, `src/memory/dreaming/report.rs`, `src/memory/dreaming/stages/mod.rs`, `src/memory/dreaming/stages/note_consolidate.rs`, `src/memory/dreaming/stages/note_drift.rs`, `src/memory/dreaming/stages/note_synthesis.rs`, `src/memory/dreaming/stages/note_lint.rs`, `src/memory/dreaming/stages/note_decay.rs`, `src/memory/dreaming/stages/daily_digest.rs`, `src/memory/dreaming/stages/types.rs`, `src/memory/store/sqlite/dream_reports.rs`, `src/memory/store/sqlite/recall_signals.rs`, `src/config/memory.rs` (for dreaming config block)

- [ ] **Step 1: Verify scheduling constants**

Read `src/memory/dreaming/mod.rs`. Document in §2:

- `DEFAULT_CHECK_INTERVAL_SECONDS` (literal value)
- `ensure_dream_daemon` singleton pattern
- idle threshold + time window + weekly cadence from `DreamingConfig`

- [ ] **Step 2: Verify `DreamGate`**

Read `src/memory/dreaming/gate.rs`. Document in §3:

- `DreamGate` struct
- `BlockReason` enum variants (exact)
- `GateResult` enum
- When gate fires

- [ ] **Step 3: Verify core types**

§4 subsections with code blocks:

- §4.1 `DreamContext` — paste the struct from `src/memory/dreaming/mod.rs` with doc comments, including `NoteEntry`
- §4.2 `DreamPipeline` — constructor pattern, `daily()` / `weekly()` builders, `run()` method
- §4.3 `DreamStage` trait — read `src/memory/dreaming/stages/mod.rs`, paste trait signature including `name`, `should_run`, `execute`

- [ ] **Step 4: Write §5.1 NoteConsolidate**

Read `src/memory/dreaming/stages/note_consolidate.rs`. Document:

- Input source (notes_index scan + notes_vec similarity)
- Similarity threshold (verify the actual constant; spec said 0.85)
- LLM decision types (merge / coexist / absorb) — quote the exact prompt labels
- Outputs (report fields updated)
- Safety: backup mechanism

Target: 40–60 lines for the subsection.

- [ ] **Step 5: Write §5.2 NoteDrift**

Read `src/memory/dreaming/stages/note_drift.rs`. Document:

- 7-day window (verify constant)
- Wikilink-connected retrieval
- LLM verdict types (consistent / contradictory / stale — verify)
- Contradiction marking (strikethrough or `## Superseded` section — verify actual behavior)
- `stale: true` frontmatter flag
- Output fields

- [ ] **Step 6: Write §5.3 NoteSynthesis (Weekly)**

Read `src/memory/dreaming/stages/note_synthesis.rs`. Document:

- DBSCAN clustering (verify parameters — epsilon, min_samples)
- Per-category scope
- Synthesis output path (`synthesis/{category}-insights.md` — verify)
- Wikilinks added to source notes
- Trigger: weekly pipeline only (restate)

- [ ] **Step 7: Write §5.4 NoteLint**

Read `src/memory/dreaming/stages/note_lint.rs`. Document:

- Frontmatter completeness check (which fields are required)
- Broken link detection algorithm
- Auto-fix cases: missing defaults, fuzzy-match repair
- FTS/embedding index rebuild trigger (content_hash change)
- Output fields

- [ ] **Step 8: Write §5.5 NoteDecay**

Read `src/memory/dreaming/stages/note_decay.rs`. Document:

- Activity score formula (paste the actual expression — if it matches spec's `access_count*0.4 + recency*0.3 + links*0.3` then good; if not, use the code's)
- Recency weight formula
- Bottom-10% cleanup
- Protection rules (category-specific thresholds, incoming-link count, age gate)
- Archive destination (`archive/{category}/`)
- recall_signals dependency — forward-link to §8

- [ ] **Step 9: Write §5.6 DailyDigest**

Read `src/memory/dreaming/stages/daily_digest.rs`. Document:

- 24h window
- Note body reading
- LLM summary generation
- Output: `daily_insights` table row

- [ ] **Step 10: Write §6 Pipelines**

Paste the exact `DreamPipeline::daily()` and `DreamPipeline::weekly()` constructors as Rust. Describe:

- Daily order and why merge-first
- Weekly = Daily + NoteSynthesis between NoteDrift and NoteLint (verify exact position)

Include a small ASCII pipeline flow.

- [ ] **Step 11: Write §7 DreamReport**

Read `src/memory/dreaming/report.rs`. Paste the full struct with doc comments. Enumerate `DreamReportStatus` variants. Explain `stages_executed` tracking and interruption semantics.

- [ ] **Step 12: Write §8 Persistence**

For each table read the DDL:

- `dream_status` — schema + lifecycle
- `dream_reports` — one row per run
- `daily_insights` — one row per date
- `recall_signals` — feeds NoteDecay. Note rename: `fact_id → note_path`. Forward-link to NOTES.md §8.

- [ ] **Step 13: Write §9 Safety + §10 Configuration**

§9: bulleted list — archive over delete, backup before merge, interruption on activity, `dream_reports` audit trail.

§10: paste the `[memory.dreaming]` TOML block from the actual config source (`src/config/memory.rs` or `config.example.toml` if it exists). Defaults and one-line semantics per key.

- [ ] **Step 14: Write §1 Purpose + See Also**

§1: one paragraph — dream = offline integrity maintenance for notes (not memory writing), runs during user-idle windows, interruptible.

See Also: NOTES.md §7 (what the dream operates on), RAW_MEMORY.md §7.1 (note: CompressionService is the realtime path, Dream is the offline path), RETRIEVAL.md §1 (better retrieval thanks to dream consolidation).

- [ ] **Step 15: Fact-check scan**

```bash
grep -n "SummarizeStage\|DriftDetectStage\|WikiLintStage\|DecayStage\|ConsolidateStage\|WikiIngestStage\|TunnelDiscoveryStage" docs/reference/memory/DREAM_DAEMON.md
```

Expected: zero matches. Only the new stage names should appear.

```bash
grep -n "graph_nodes\|graph_edges\|MemoryStore::\|MemoryStrength" docs/reference/memory/DREAM_DAEMON.md
```

Expected: zero matches.

- [ ] **Step 16: Line count check**

Run: `wc -l docs/reference/memory/DREAM_DAEMON.md`

Expected: 310–410 lines (target 360 ±50).

- [ ] **Step 17: Commit**

```bash
git add docs/reference/memory/DREAM_DAEMON.md
git commit -m "docs(memory): write DREAM_DAEMON.md — 6-stage notes consolidation

Scheduling, DreamGate, pipeline types, stages (NoteConsolidate /
NoteDrift / NoteSynthesis / NoteLint / NoteDecay / DailyDigest),
daily vs weekly pipelines, DreamReport, persistence tables,
safety invariants, configuration."
```

---

## Task 5: Write `memory/RETRIEVAL.md`

Depends on NOTES and DREAM_DAEMON.

**Files:**
- Modify: `docs/reference/memory/RETRIEVAL.md`
- Read: `src/memory/note_retrieval/mod.rs`, `src/memory/note_retrieval/hybrid.rs`, `src/memory/notes/search_result.rs`, `src/memory/notes/retrieval.rs`, `src/memory/notes/store.rs` (hybrid_search_notes impl), `src/memory/scoring_pipeline/mod.rs`, `src/memory/scoring_pipeline/config.rs`, every file in `src/memory/scoring_pipeline/stages/`, `src/memory/rerank/`, `src/memory/query_expander.rs`, `src/memory/embedding_provider.rs`, `src/memory/composer.rs`, `src/memory/context_comptroller/`, `src/memory/ai_retrieval.rs`, `src/memory/ripple/mod.rs`, `src/memory/ripple/task.rs`, `src/builtin_tools/memory_search.rs`, `src/builtin_tools/memory_browse.rs`, `src/builtin_tools/memory_explore.rs`, `src/builtin_tools/recall_context.rs`, `src/memory/audit.rs`, `src/memory/cortex/mod.rs` (read top-level to confirm non-integration)

- [ ] **Step 1: Verify entry points**

Read `src/memory/note_retrieval/mod.rs` and `hybrid.rs`. In §1, paste the `NoteFactRetrieval` struct and the signatures of `retrieve()` and `vector_retrieve()`. Include the `ScoredFact` return-type note.

- [ ] **Step 2: Document hybrid algorithm**

Read `src/memory/notes/store.rs` `hybrid_search_notes` implementation. In §2:

- Step 1: parallel vector + FTS search
- Step 2: RRF fusion (paste the k constant from code)
- Step 3: sort + top-k
- Step 4: disk content load from `memory/note/{agent}/{category}/{filename}.md`
- ASCII flow diagram

- [ ] **Step 3: Document type bridge**

Read `src/memory/notes/search_result.rs`. In §3 paste `NoteSearchResult` + `to_memory_fact()` + `to_scored_fact()`. Explain: "downstream consumers still receive `ScoredFact<MemoryFact>`, but path is now `note://...` and tier/strength fields are defaults."

- [ ] **Step 4: Write §4 Scoring Pipeline overview table**

§4.1: for each of the 7 stages, one row:

| Stage | Purpose | Key config key |
|---|---|---|
| `importance_weight` | … | … |
| `cosine_rerank` | … | … |
| `mmr_diversity` | … | … |
| `time_decay` | … | … |
| `recency_boost` | … | … |
| `length_normalization` | … | … |
| `hard_min_score` | … | … |

Source the config keys from `src/memory/scoring_pipeline/config.rs`.

- [ ] **Step 5: Write §4.2–§4.8 per-stage**

Each subsection: 15–25 lines. Read the stage file. Document:

- What input field it reads
- What transformation it applies (formula if mathematical)
- Which config key controls it
- Ordering semantics within the pipeline

For §4.2, include a sub-paragraph "ValueEstimator feeds this stage" that cites `src/memory/value_estimator/mod.rs` and lists the 8 signal types (UserPreference / FactualInfo / Decision / PersonalInfo / Question / Answer / Greeting / SmallTalk) with base scores verified from code.

- [ ] **Step 6: Write §5 Reranker (not wired)**

Read `src/memory/rerank/`. List the 5 provider implementations (Jina / Voyage / SiliconFlow / vLLM / Pinecone). For each, one line:

- Model/endpoint the provider targets
- Config key to enable

Clearly mark: "Implemented but not wired into `NoteFactRetrieval` as of this doc; see `src/memory/note_retrieval/hybrid.rs` for the expected integration point."

- [ ] **Step 7: Write §6 Query Expander (not wired)**

Read `src/memory/query_expander.rs`. One paragraph:

- What it does (Chinese synonym expansion)
- Config key
- "Not wired" status

- [ ] **Step 8: Write §7 Embedding Provider**

Read `src/memory/embedding_provider.rs`. §7 has three parts:

- `EmbeddingProvider` trait signature (paste)
- `RemoteEmbeddingProvider` struct + how `create_provider()` selects implementation
- Preset table (SiliconFlow / OpenAI / Ollama / Custom) with api_base, model, dimensions — pull from code, not the old doc
- Multi-dimension rationale: `notes_vec_{768,1024,1536}` let provider swaps not lose data

- [ ] **Step 9: Write §8 Context Assembly**

- §8.1 `ContextComposer`: read `src/memory/composer.rs`. Document `CompositionRequest`, `ComposedContext`, the core/global/workspace/persona filter-build helpers. Paste key struct fields with comments.
- §8.2 `ContextComptroller`: read `src/memory/context_comptroller/`. Document `ComptrollerConfig` (redundancy_threshold, token_budget), `RetentionMode` variants, and the redundancy-detection algorithm (cosine similarity ≥ threshold). Include the `ArbitratedContext` return type.

- [ ] **Step 10: Write §9 `AiMemoryRetriever`**

Read `src/memory/ai_retrieval.rs`. One subsection (30–50 lines):

- `AiMemoryRequest` / `AiMemoryResult` / `MemoryCandidate`
- Optional LLM-in-the-loop candidate selection
- When it's used (via config flag)

- [ ] **Step 11: Write §10 RippleTask**

Read `src/memory/ripple/`. Document:

- `RippleConfig` fields (BFS depth, similarity floor, branching limit)
- `RippleTask::run` algorithm — BFS over `NoteStore::vector_search_notes_with_content`
- `RippleResult` output

- [ ] **Step 12: Write §11 Memory Tools**

For each of §11.1–§11.4, read the builtin tool file:

- `memory_search` (§11.1): params, output, uses `NoteFactRetrieval` + `ContextComptroller`
- `memory_browse` (§11.2): params (list / read actions), filesystem-backed (not SQL). Point at the post-VFS rewrite.
- `memory_explore` (§11.3): wraps `RippleTask`, params (query, depth, limit)
- `recall_context` (§11.4): params, uses `RawMemoryStore::get_raw_by_path_prefix`. Forward-link to RAW_MEMORY.md §7.2.

One subsection per tool, 15–25 lines each.

- [ ] **Step 13: Write §12 Audit**

Read `src/memory/audit.rs`. Document:

- `AuditEntry`, `AuditAction`, `AuditActor`, `AuditDetails`
- `ExplainedEvent`, `FactExplanation`, `ForgettingExplanation`
- The explain-path: how to trace why a note was returned or dropped

Forward-link to NOTES.md §12 (event sourcing is the write-side counterpart).

- [ ] **Step 14: Write §13 Cortex note**

Read `src/memory/cortex/mod.rs` (just the top-level re-exports and integration.rs). One short subsection:

- Lists the submodules (distillation, meta_cognition, pattern_extractor, clustering)
- States: "Self-contained experimental subsystem. Not invoked by `NoteFactRetrieval`, `ContextComposer`, or the main agent loop as of this doc. Retained for future integration paths in `src/memory/cortex/integration.rs`."
- Says where a curious reader should start (`integration.rs`)

Keep under 25 lines. This is a placeholder, not a feature doc.

- [ ] **Step 15: Write Appendix tuning tips**

Absorb the still-valid retrieval portions of the old doc's "Best Practices" (§3.1–§3.3 of old doc lines 790–815). Reframe as:

- Retrieval precision: threshold tuning, rerank enablement when it lands
- Context budget: ContextComptroller knobs
- Deduplication: redundancy_threshold

5–10 bullet points, no long prose.

- [ ] **Step 16: Fact-check scan**

```bash
grep -n "FactRetrieval\b\|HybridRetrieval\b\|MemoryStore::\|VFS\|aleph://user\|graph_nodes" docs/reference/memory/RETRIEVAL.md
```

Expected: zero in production-code claims. Historical references must be labeled ("replaced by…").

- [ ] **Step 17: Line count check**

Run: `wc -l docs/reference/memory/RETRIEVAL.md`

Expected: 370–470 lines (target 420 ±50).

- [ ] **Step 18: Commit**

```bash
git add docs/reference/memory/RETRIEVAL.md
git commit -m "docs(memory): write RETRIEVAL.md — hybrid search, scoring, tools

NoteFactRetrieval entry points, RRF hybrid algorithm, NoteSearchResult
type bridge, 7-stage ScoringPipeline, optional rerank/query expander,
embedding providers, ContextComposer + ContextComptroller, RippleTask,
memory tools (search / browse / explore / recall_context), audit and
explainability, Cortex note."
```

---

## Task 6: Write `MEMORY_SYSTEM.md` Overview

Final subdoc. Written last so every subdoc reference is to a real file.

**Files:**
- Modify: `docs/reference/MEMORY_SYSTEM.md`
- Read: `src/config/memory.rs`, `src/memory/scratchpad/mod.rs`, `src/memory/store/mod.rs`, `src/memory/mod.rs` (for top-level re-exports), `/tmp/MEMORY_SYSTEM.old.md` (for Troubleshooting and Config fragments)

- [ ] **Step 1: Write §1 Purpose**

One paragraph. Spec §3.1 row 1. Keep under 60 words.

- [ ] **Step 2: Write §2 Design Principles**

Five bullets:

- L0 (raw, ephemeral) → L1 (notes, persistent) separation
- Markdown is source of truth; SQLite is rebuildable index
- One trait per storage concern (no monolithic MemoryStore)
- LLM sovereignty — classification/merge/synthesis decisions go to the model, not regex
- Real filesystem over VFS abstractions

- [ ] **Step 3: Write §3 Two-Layer Data Model diagram**

ASCII box-drawing:

```
Gateway / Agent Loop
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│ raw_memories (SQLite)                                        │
│   sessions · transcripts · attachment_text                   │
│   consumed by CompressionService (is_processed flag)         │
└─────────────────────────────────────────────────────────────┘
    │  CompressionService (realtime)
    ▼
┌─────────────────────────────────────────────────────────────┐
│ notes (Markdown files + SQLite index)                        │
│   ~/.aleph/memory/note/{agent}/{category}/*.md               │
│   notes_index · notes_links · notes_fts · notes_vec_{dim}    │
└─────────────────────────────────────────────────────────────┘
    │  Dream Daemon (offline, idle-only)
    ▼
   consolidate / drift / synthesis / lint / decay / digest
    │
    ▼
 queries: NoteFactRetrieval.retrieve() → ScoredFact<MemoryFact>
```

Verify every path and table name exists. One paragraph after the diagram summarizing the flow.

- [ ] **Step 4: Write §4 Storage Traits table**

| Trait | File | Purpose | Primary caller |
|---|---|---|---|
| `NoteStore` | `src/memory/notes/store.rs` | Notes index, wikilinks, FTS, vector search | `NoteFactRetrieval`, `NoteIndexer` |
| `RawMemoryStore` | `src/memory/store/raw_memory.rs` | Raw memory CRUD + is_processed flag | `CompressionService`, `SessionCompactor` |
| `DreamStore` | `src/memory/store/mod.rs` | Dream status + daily insights | `DreamDaemon` |
| `CompressionStore` | `src/memory/store/mod.rs` | Compression-run audit metadata | `CompressionService` |

Verify against current code. Add a closing sentence: "All four are implemented by `SqliteMemoryBackend`, wrapped in `MemoryBackend = Arc<SqliteMemoryBackend>`."

- [ ] **Step 5: Write §5 Scratchpad**

Read `src/memory/scratchpad/mod.rs`. One short subsection:

- Purpose: in-session conversation history buffer, non-persistent
- `ScratchpadConfig`, `SessionHistory`, `ScratchpadManager`
- Orthogonal to L0/L1 (session-level, not session-archive)

15–30 lines.

- [ ] **Step 6: Write §6 Interfaces**

One-line table of tools with forward-links:

| Tool | Purpose | Doc |
|---|---|---|
| `note_manage` | CRUD on notes (unified skill/wiki/other) | NOTES.md §11 |
| `memory_search` | Hybrid retrieval | RETRIEVAL.md §11.1 |
| `memory_browse` | Filesystem browser over notes | RETRIEVAL.md §11.2 |
| `memory_explore` | Multi-hop (Ripple) exploration | RETRIEVAL.md §11.3 |
| `recall_context` | Session raw-data restore | RETRIEVAL.md §11.4 |

- [ ] **Step 7: Write §7 TOML Configuration**

Read `src/config/memory.rs`. Paste the current `MemoryConfig` structure as a TOML block. Include subsections:

```toml
[memory]
...

[memory.context_comptroller]
...

[memory.value_estimator]
...

[memory.dreaming]
...
```

Each key gets an inline `# comment` with the default value and one-line semantics. Discard any keys from the old doc that no longer exist in code (e.g., `graph_decay`, `memory_decay` — verify).

- [ ] **Step 8: Write §8 Subdocument Navigation**

```markdown
## 8. Subdocument Navigation

- [Notes (L1)](memory/NOTES.md) — markdown-first persistent knowledge, indexing, `note_manage` tool.
- [Raw Memory (L0)](memory/RAW_MEMORY.md) — ephemeral session data, compression input.
- [Dream Daemon](memory/DREAM_DAEMON.md) — 6-stage offline notes consolidation.
- [Retrieval](memory/RETRIEVAL.md) — hybrid search, scoring, tools, audit.
```

- [ ] **Step 9: Write §9 Troubleshooting**

Port the old doc's "Troubleshooting" section (lines ~847–874). For each issue, re-verify the suggested fix references a real config key:

- High memory usage → which config keys actually exist now?
- Slow search → similarity threshold, max_context_items — still in config?
- Missing results → threshold lowering

Keep 3 issues, 3–5 solutions each. If any suggested fix points at a removed key, drop or rewrite.

Absorb the still-valid config/retention tips from the old "Best Practices" here (spec §4 row).

- [ ] **Step 10: Fact-check scan**

```bash
grep -n "facts table\|MemoryStore::\|aleph://\|graph_nodes\|graph_edges\|FactRetrieval\|HybridRetrieval\|MemoryStrength\|WikiIngestStage\|layer.*tier" docs/reference/MEMORY_SYSTEM.md
```

Expected: zero matches (historical/migration prose disallowed in overview — this is a clean entry doc).

- [ ] **Step 11: Line count check**

Run: `wc -l docs/reference/MEMORY_SYSTEM.md`

Expected: 170–270 lines (target 220 ±50).

- [ ] **Step 12: Commit**

```bash
git add docs/reference/MEMORY_SYSTEM.md
git commit -m "docs(memory): rewrite MEMORY_SYSTEM.md as 5-doc overview

Purpose, design principles, two-layer model diagram, storage trait
table, scratchpad, memory tools (forward-linked), TOML config,
subdocument navigation, troubleshooting. Replaces the 899-line
pre-refactor document."
```

---

## Task 7: Cross-Link and Fact Audit

Global pass across all 5 files.

**Files:** All 5 docs (read-only verification + targeted edits if failures).

- [ ] **Step 1: Cross-link walk**

Run:

```bash
grep -rn "\](\.\|\]\(memory/\|\]\(\\.\\./" docs/reference/MEMORY_SYSTEM.md docs/reference/memory/
```

For each matched link, verify the target file + anchor exists:

```bash
for link in $(grep -rhoE '\]\([^)]+\)' docs/reference/MEMORY_SYSTEM.md docs/reference/memory/ | sed 's/[])]//g; s/\[//; s/(//'); do
  # Manual per-link resolution: strip anchor, check file exists
  file="${link%#*}"
  # Use the doc's directory as the base for relative paths
  [ -f "docs/reference/$file" ] || [ -f "docs/reference/memory/$file" ] || echo "BROKEN: $link"
done
```

Expected: no "BROKEN" lines. If broken, fix the link or add the target section anchor.

- [ ] **Step 2: Comprehensive obsolete-reference audit**

```bash
grep -rnE "facts table|MemoryStore::(insert_fact|get_all_facts|invalidate_fact|update_fact|get_compressed_facts|count_raw_memories|text_search|vector_search)|FactRetrieval\b|HybridRetrieval\b|graph_nodes\b|graph_edges\b|memory_entities\b|facts_vec_\w+|facts_fts\b|VFS|aleph://|l1_generator|l1_overview|WikiIngestStage|TunnelDiscoveryStage|SummarizeStage|DriftDetectStage|WikiLintStage|DecayStage|ConsolidateStage|MemoryStrength" docs/reference/MEMORY_SYSTEM.md docs/reference/memory/
```

Expected: zero matches. Any hit is a bug — fix by either removing the mention or clearly labeling it as "historical (pre-2026-04-12)".

- [ ] **Step 3: Path-existence audit**

For every `src/...` path referenced in any of the 5 docs, confirm it exists:

```bash
grep -rhoE "src/[^ )\`\"']+" docs/reference/MEMORY_SYSTEM.md docs/reference/memory/ | sort -u | while read p; do
  [ -e "$p" ] || echo "MISSING: $p"
done
```

Expected: no "MISSING" lines.

- [ ] **Step 4: Trait/struct name audit**

Spot-check 10 names:

```bash
for name in NoteStore RawMemoryStore DreamStore CompressionStore NoteFactRetrieval KnowledgeNote NoteIndexer DreamPipeline DreamStage NoteSearchResult; do
  grep -rl "$name" src/ > /dev/null || echo "MISSING TYPE: $name"
done
```

Expected: no "MISSING TYPE" lines.

- [ ] **Step 5: Line count summary**

Run: `wc -l docs/reference/MEMORY_SYSTEM.md docs/reference/memory/*.md`

Expected totals (each ±50):

- MEMORY_SYSTEM.md: ~220
- NOTES.md: ~420
- RAW_MEMORY.md: ~210
- DREAM_DAEMON.md: ~360
- RETRIEVAL.md: ~420

- [ ] **Step 6: If audits revealed fixes, commit them together**

```bash
git add docs/reference/
git commit -m "docs(memory): cross-link and fact-audit fixes"
```

If no fixes needed, skip the commit.

---

## Task 8: Update Upstream Pointers

Other docs may still reference the old memory doc. Add forward-links only; do not rewrite their bodies.

**Files:**
- Modify (if stale ref found): `docs/reference/ARCHITECTURE.md`, `docs/reference/AGENT_SYSTEM.md`, `docs/reference/TOOL_SYSTEM.md`, `docs/reference/GATEWAY.md`, `CLAUDE.md`

- [ ] **Step 1: Find stale memory references**

```bash
grep -rn "MEMORY_SYSTEM\.md\|facts\.db\|FactRetrieval\|MemoryStore" docs/reference/ CLAUDE.md | grep -v "docs/reference/memory/" | grep -v "docs/reference/MEMORY_SYSTEM.md"
```

- [ ] **Step 2: For each hit, apply minimal update**

Replace stale claims with a forward-link. Example rewrite:

- Before: `"See [Memory System](MEMORY_SYSTEM.md) for Facts DB details."`
- After:  `"See [Memory System](MEMORY_SYSTEM.md) for the data-layer overview; drill into [Notes](memory/NOTES.md), [Raw Memory](memory/RAW_MEMORY.md), [Dream Daemon](memory/DREAM_DAEMON.md), or [Retrieval](memory/RETRIEVAL.md)."`

Do not rewrite body content of upstream docs. Only update the memory citation.

- [ ] **Step 3: Verify CLAUDE.md link table still valid**

The `📚 文档索引` table in `CLAUDE.md` has a row `MEMORY_SYSTEM.md`. The row should remain (file still exists at that path), but consider adding 4 child rows for the subdocs if the table's grain permits. Non-blocking — can be deferred.

- [ ] **Step 4: Commit if any upstream edits were made**

```bash
git add docs/reference/*.md CLAUDE.md
git commit -m "docs: update memory-system forward links in upstream docs"
```

If no edits needed, skip.

---

## Self-Review Results

**Spec coverage:**

- Spec §3.1 (MEMORY_SYSTEM.md 9 sections) → Task 6 steps 1–10
- Spec §3.2 (NOTES.md 13 sections) → Task 3 steps 1–13
- Spec §3.3 (RAW_MEMORY.md 8 sections) → Task 2 steps 1–7
- Spec §3.4 (DREAM_DAEMON.md 10 sections) → Task 4 steps 1–14
- Spec §3.5 (RETRIEVAL.md 13 sections + appendix) → Task 5 steps 1–15
- Spec §4 fact-extraction matrix → Tasks 2–6 individual verification steps + Task 7 step 2 audit
- Spec §5 writing constraints → enforced by every "verify … from code" step + Task 7 audits
- Spec §6 migration strategy → Task 1 (scaffold) → Tasks 2–5 (subdocs) → Task 6 (overview) → Task 7 (audit)
- Spec §7 success criteria → Task 7 steps 1–5 check each row
- Spec §8 risks → mitigated by verify-before-write pattern in every subdoc task
- Spec §9 out-of-scope → Task 8 adds pointers only, no body rewrites

No gaps detected.

**Placeholder scan:** No "TBD" or "fill in" remain. Every step either has concrete grep commands, literal paths, or "read `src/foo.rs` and document X" with a specific deliverable.

**Type consistency:** Trait and struct names (`NoteStore`, `RawMemoryStore`, `DreamStore`, `CompressionStore`, `NoteFactRetrieval`, `KnowledgeNote`, `NoteIndexer`, `DreamPipeline`, `DreamStage`, `NoteSearchResult`) are used identically across all tasks. No naming drift.

---

Plan complete and saved to `docs/superpowers/plans/2026-04-12-memory-docs-restructure.md`.
