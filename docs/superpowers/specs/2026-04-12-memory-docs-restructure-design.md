# Memory Documentation Restructure Design

**Date:** 2026-04-12
**Status:** Draft
**Scope:** Restructure `docs/reference/MEMORY_SYSTEM.md` (899 lines, ~60% obsolete) into a 5-document hierarchy reflecting the post–facts-table architecture.

---

## 1. Problem

`docs/reference/MEMORY_SYSTEM.md` was written for the pre-refactor architecture. After the 2026-04-11 → 2026-04-12 migrations, the real code diverges substantially:

| Area | Doc says | Code is |
|------|----------|---------|
| Primary storage | `facts` table with `facts_vec_*` / `facts_fts` | `facts` table + DDL fully deleted |
| Storage trait | `MemoryStore` (17 fact methods) + `GraphStore` | `MemoryStore` deleted. Replaced by `NoteStore` / `RawMemoryStore` / `DreamStore` / `CompressionStore` |
| Knowledge graph | `graph_nodes` / `graph_edges` SQLite tables | Tables dropped. Graph = wikilinks in markdown + `notes_links` index |
| Path model | VFS (`aleph://user/preferences/...`) | VFS deleted. Real filesystem (`~/.aleph/memory/note/{agent_id}/{category}/*.md`) |
| Dream stages | `SummarizeStage` / `DriftDetectStage` / `WikiLintStage` / `DecayStage` / `ConsolidateStage` + unregistered `WikiIngestStage` / `TunnelDiscoveryStage` | `NoteConsolidate` / `NoteDrift` / `NoteSynthesis` / `NoteLint` / `NoteDecay` / `DailyDigest` (daily=5, weekly=6) |
| ACMA dimensions | `layer` / `tier` / `strength` on `MemoryFact` | Notes have no tier/strength. `decay.rs` + `MemoryStrength` are dead code |
| Retrieval | `FactRetrieval` / `HybridRetrieval` | `NoteFactRetrieval` on `NoteStore::hybrid_search_notes` |
| Paths referenced | `src/memory/augmentation.rs`, `graph.rs`, `retention.rs`, `compression_daemon/` | None of these exist |

A single 899-line doc cannot be fixed by patching — the architectural framing is wrong. It needs restructuring to match the two-layer model the code now embodies.

---

## 2. Goals

1. Documentation accurately reflects current code (traits, modules, tables, file paths).
2. Each document scoped by domain, sized 200–420 lines (project's CODE_ORGANIZATION principle).
3. Clear navigation: high-level readers land on overview, implementers drill into subdocs.
4. Preserve still-valid content from old doc (embedding providers, TOML config, troubleshooting) — do not re-research known facts.
5. Zero documentation lies: every trait signature, schema column, and path must be verified against `src/memory/` at write time.

**Non-goals:**

- Redesigning the memory system. This is a documentation task; code stays as-is.
- Documenting `src/memory/cortex/` or `src/memory/decay.rs` internals — these are standalone/dead subsystems, acknowledged briefly but not expanded.
- Rewriting `ARCHITECTURE.md` / `TOOL_SYSTEM.md` / `GATEWAY.md` memory sections. Those stay; we add forward-links only.

---

## 3. Target Structure

```
docs/reference/
├── MEMORY_SYSTEM.md                   Overview + navigation (entry point)
└── memory/
    ├── NOTES.md                        L1 notes layer
    ├── RAW_MEMORY.md                   L0 ephemeral layer
    ├── DREAM_DAEMON.md                 Offline consolidation pipeline
    └── RETRIEVAL.md                    Retrieval, scoring, tools
```

All five documents are written in the same style:

- First line: one-sentence purpose.
- Architecture diagram near top (ASCII, box-drawing).
- Code references use `src/path:line` format where line is stable (struct definitions, trait signatures).
- No prose repetition across docs — overlapping topics link out.

### 3.1 `MEMORY_SYSTEM.md` (Overview) — target ~220 lines

| Section | Content |
|---------|---------|
| 1. Purpose | One paragraph: persistent knowledge for LLM conversations via markdown-first notes + ephemeral raw memories + offline consolidation. |
| 2. Design principles | L0→L1 separation; markdown is source of truth, SQLite is rebuildable index; LLM sovereignty (no hand-coded classification); one trait per concern. |
| 3. Two-layer data model | Diagram: Gateway → raw_memories → CompressionService → notes (markdown + index) → Dream daemon → wiki/skill/preference/... |
| 4. Storage traits | Table: trait → file → purpose → primary caller. Covers `NoteStore`, `RawMemoryStore`, `DreamStore`, `CompressionStore`. |
| 5. Scratchpad | In-session history buffer (`src/memory/scratchpad/`), non-persistent, complements L0/L1. |
| 6. Interfaces | Memory tools exposed to LLM: `note_manage`, `memory_search`, `memory_browse`, `memory_explore`, `recall_context`. One-line each, forward-link to RETRIEVAL.md §11. |
| 7. TOML configuration | `[memory]` / `[memory.context_comptroller]` / `[memory.value_estimator]` / `[memory.dreaming]` — preserved from old doc, verified against `src/config/memory.rs`. |
| 8. Subdocument navigation | Links to NOTES / RAW_MEMORY / DREAM_DAEMON / RETRIEVAL with one-line summary each. |
| 9. Troubleshooting | Common issues + resolutions — memory usage, search latency, missing results — config-key-level fixes. Absorbs the still-valid portions of the old "Best Practices" (config/retention tips). |

### 3.2 `memory/NOTES.md` — target ~420 lines

| Section | Content |
|---------|---------|
| 1. Overview | Markdown-first persistent knowledge. Each note = one file under `memory/note/{agent_id}/{category}/*.md`. |
| 2. Filesystem layout | Directory tree example with 8–10 categories: preference / plan / learning / skill / wiki / tool / synthesis / archive / other. |
| 3. Frontmatter schema | YAML fields: `category` / `tags` / `created` / `updated`. Category-specific fields (wiki: `title` + `aliases` + `sources`; skill: `title` + `scope`). |
| 4. `KnowledgeNote` data model | Struct definition (`src/memory/notes/note.rs`), title sanitization rules. |
| 5. Wikilinks | `[[target]]` and `[[target\|alias]]` syntax, resolution algorithm (filename-exact → category-prefix → fuzzy). Stored in `notes_links`. |
| 6. `NoteIndexer` | Write pipeline: write markdown → parse frontmatter + wikilinks → upsert notes_index / notes_links / notes_fts / notes_vec. `full_rebuild()` for cold-start. Sub-paragraph "Compression scheduler" covers `src/memory/compression/scheduler.rs` — the idle-triggered driver that calls `CompressionService → NoteIndexer`. |
| 7. `NoteStore` trait | Method-by-method table: `index_note` / `remove_note_index` / `get_note_index` / `list_notes` / `get_outgoing_links` / `get_incoming_links` / `search_notes_fts` / `hybrid_search_notes` / `vector_search_notes_with_content` / `get_notes_by_category` / `count_all_notes` / `find_by_filename` / `get_graph_data` / `get_neighbors`. |
| 8. SQLite schema | DDL for `notes_index` / `notes_links` / `notes_fts` / `notes_vec_map` / `notes_vec_{768,1024,1536}` / `recall_signals`. |
| 9. Wiki post-write hooks | Git commit per change, `index.md` regeneration. Only active for `category == "wiki"`. |
| 10. Skills as notes | `skill/` category = persona-scoped notes. Frontmatter `scope: persona`. |
| 11. `note_manage` tool | Action enum + args, category-specific frontmatter templates, migration from `skill_manage` / `wiki_manage`. |
| 12. Event sourcing | `MemoryCommandHandler` writes notes via commands (`CreateFact` / `UpdateContent` / `InvalidateFact` / …); events stored for audit + replay. Forward-link to audit section in RETRIEVAL.md. |
| 13. Namespace scoping | `NamespaceScope` (`src/memory/namespace.rs`) — agent_id-based isolation. One paragraph. |

### 3.3 `memory/RAW_MEMORY.md` — target ~210 lines

| Section | Content |
|---------|---------|
| 1. Role | Short-lived, high-volume, session-scoped raw data. Not worth a markdown file per row. |
| 2. When to use raw vs notes | Decision table: conversation turn transcripts, session summaries, attachment text → raw; synthesized knowledge → notes. |
| 3. `raw_memories` schema | DDL with column purpose: `id` / `content` / `source` / `agent_id` / `session_id` / `path` (traceability) / `layer` (d0/d1/d2 session summaries) / `attachment_text` / `is_processed` / `created_at`. Indexes. |
| 4. `RawMemory` + `RawMemorySource` enums | Struct + enum definitions from `src/memory/store/raw_memory.rs`. |
| 5. `RawMemoryStore` trait | Method table: `insert_raw_memory` / `get_unprocessed_raw_memories` / `mark_as_processed` / `get_raw_by_path_prefix` / …. |
| 6. Writers | `SessionCompactor` (d0/d1/d2 summaries), `TranscriptIndexer` (semantic chunks), Gateway media pipeline (attachment_text). One subsection each with config knobs. |
| 7. Readers | `CompressionService` (consume + mark_as_processed), `recall_context` tool (path-prefix scan), `session_summary_source` (session context restore). |
| 8. Lifecycle | Retention = until `is_processed=1` + 24h TTL (configurable). No decay, no scoring — raw data is transient. |

### 3.4 `memory/DREAM_DAEMON.md` — target ~360 lines

| Section | Content |
|---------|---------|
| 1. Purpose | Offline consolidation of the notes layer during user idle. |
| 2. Scheduling | `ensure_dream_daemon()` global singleton, 60 s check interval, idle threshold + time window + weekly cadence. |
| 3. `DreamGate` | Block reasons (activity / dim resources / config disabled). |
| 4. `DreamContext` + `DreamPipeline` + `DreamStage` trait | Core types from `src/memory/dreaming/mod.rs`. Stage execution with interruption on activity. |
| 5. Stages (one subsection each) | **NoteConsolidate** — pairwise similarity > 0.85 → LLM merge/coexist/absorb. **NoteDrift** — 7-day window, wikilink-connected consistency check. **NoteSynthesis** (weekly) — DBSCAN per category → insight notes under `synthesis/`. **NoteLint** — frontmatter + broken wikilink repair + index rebuild. **NoteDecay** — activity score, bottom 10% → archive (not delete). **DailyDigest** — 24h window → `daily_insights` table. |
| 6. Pipelines | Daily (5 stages, no Synthesis); Weekly (6 stages). Mermaid-style textual flow. |
| 7. `DreamReport` schema | Field-by-field, status enum (Completed / Interrupted / Failed), `stages_executed` tracking. |
| 8. Persistence | `dream_status` / `dream_reports` / `daily_insights` / `recall_signals` tables. |
| 9. Safety | Archive over delete, original backup before merge, all writes logged. |
| 10. Configuration | `[memory.dreaming]` TOML keys with defaults and semantics. |

### 3.5 `memory/RETRIEVAL.md` — target ~420 lines

| Section | Content |
|---------|---------|
| 1. Entry points | `NoteFactRetrieval::retrieve()` / `vector_retrieve()` — signatures and intended use sites. |
| 2. Hybrid search algorithm | Vector (`notes_vec_{dim}`) + FTS (`notes_fts`) → RRF fusion (k=60) → disk content load → `Vec<NoteSearchResult>`. |
| 3. Bridge to legacy types | `NoteSearchResult::to_memory_fact()` / `to_scored_fact()` — why this bridge exists, what zero-downstream-change means. |
| 4. Scoring Pipeline | Table of 7 stages: `importance_weight` / `cosine_rerank` / `mmr_diversity` / `time_decay` / `recency_boost` / `length_normalization` / `hard_min_score` — purpose + key config key for each. One short subsection per stage. |
| 5. Reranker (optional, not wired) | Providers: Jina / Voyage / SiliconFlow / vLLM / Pinecone. Integration path if enabled. |
| 6. Query expander (optional, not wired) | Chinese synonym expansion, integration status. |
| 7. Embedding provider | `EmbeddingProvider` trait, `RemoteEmbeddingProvider`, preset table (SiliconFlow / OpenAI / Ollama / Custom), multi-dimension storage rationale. |
| 8. Context assembly | `ContextComposer` (global + workspace + persona union), `ContextComptroller` (redundancy + token budget + `RetentionMode`). |
| 9. `AiMemoryRetriever` | Optional LLM-in-the-loop candidate picker. |
| 10. RippleTask | Multi-hop vector traversal over notes, BFS bounded by similarity + depth. |
| 11. Memory tools | `memory_search` / `memory_browse` (filesystem browser) / `memory_explore` (RippleTask wrapper) / `recall_context` — params + backing implementation. |
| 12. Audit + explainability | `AuditEntry`, `FactExplanation`, `ForgettingExplanation` from `src/memory/audit.rs` — how to trace why a retrieval returned a given note. |
| 13. Cortex (independent subsystem) | **One short note.** `src/memory/cortex/` — pattern extraction, distillation, meta_cognition. Currently self-contained, not consumed by the main retrieval/agent loop. Flagged as experimental / not-wired. |

---

## 4. Fact Extraction Matrix (Old Doc → New Docs)

For each major section of the current `MEMORY_SYSTEM.md`, the disposition:

| Old section (line range) | Disposition | New location |
|---|---|---|
| Facts Database + Storage Traits (69–139) | **DELETE** — factually wrong | — |
| Embedding + Provider Presets (141–179) | **KEEP** (update paths) | RETRIEVAL.md §7 |
| Hybrid Retrieval + RRF (180–249) | **UPDATE** — retarget at notes | RETRIEVAL.md §1–2 |
| Context Augmentation (253–298) | **UPDATE** — merge into ContextComposer description | RETRIEVAL.md §8 |
| Session Compression (300–357) | **UPDATE** — retarget at raw_memories → notes flow | RAW_MEMORY.md §7 + NOTES.md (write side) |
| Memory Decay (359–384) | **DELETE** — decay.rs is dead; notes use NoteDecay stage | DREAM_DAEMON.md §5 (NoteDecay) |
| Cognitive Memory Architecture (ACMA) (388–450) | **DELETE** — tier/strength/layer not on notes | — |
| Retention Policies (452–473) | **DELETE** — struct doesn't exist | — |
| Memory Graph (475–487) | **UPDATE** — graph = wikilinks + notes_links | NOTES.md §5 + §8 |
| DreamDaemon (489–516) | **UPDATE** — new stage names and pipelines | DREAM_DAEMON.md §4–6 |
| ContextComptroller (520–560) | **KEEP** (update paths) | RETRIEVAL.md §8 |
| ValueEstimator (562–621) | **KEEP** (verify still wired) | RETRIEVAL.md §4 — described as the input to the `importance_weight` scoring stage |
| CompressionDaemon (623–651) | **UPDATE** — actual path is `src/memory/compression/scheduler.rs` | NOTES.md §6 (NoteIndexer write pipeline) — added paragraph "Compression scheduler" |
| memory_search tool (653–696) | **UPDATE** — now on NoteFactRetrieval | RETRIEVAL.md §11 |
| Configuration (699–752) | **KEEP** (verify keys) | MEMORY_SYSTEM.md §7 |
| Manual Test Checklist (756–762) | **DROP** | — (manual QA is not doc content; out of scope) |
| Usage Examples (766–786) | **DROP** | — (too shallow to be useful) |
| Best Practices (790–815) | **SPLIT** | Config/retention tips → MEMORY_SYSTEM.md §9 (merged with Troubleshooting); retrieval tuning tips → RETRIEVAL.md appendix |
| Performance Metrics (819–843) | **KEEP** but mark "approximate, 2026-Q2 measurements" | RETRIEVAL.md appendix |
| Troubleshooting (847–874) | **KEEP** (update config keys) | MEMORY_SYSTEM.md §9 |
| Pending Connection (878–890) | **KEEP** — still valid list | RETRIEVAL.md §5 + §6 + cross-refs |

Every **KEEP** / **UPDATE** entry must be re-verified against current code at write time (read the Rust source, not the old doc, when extracting).

---

## 5. Writing Constraints

Applied to every new doc:

1. **Verify before write.** Each trait signature, field name, path, and DDL must be read from `src/memory/` in the same editing session. No copy-from-memory.
2. **One purpose per section.** If a section grows beyond 50 lines, split it.
3. **Link, don't repeat.** Any topic owned by another doc gets a one-line summary + forward link.
4. **Diagrams as ASCII box-drawing** (consistent with existing reference docs).
5. **No "TODO" / "TBD" placeholders** — if we can't verify, the section is cut.
6. **Code snippets show struct/trait signatures + doc comments** — no full implementation bodies (point to the file instead).
7. **No version-specific claims** (e.g., "as of v2026.04.12"). Docs describe current code; git history is the timeline.
8. **English prose in code samples; Chinese or English narrative OK** (project convention). For this doc set we default to **English narrative** — matches every other file under `docs/reference/`.

---

## 6. Migration Strategy

Follow approach **B (Fact extraction + new architecture)**, executed in order:

1. **Write spec** (this document) + commit.
2. **Draft outline files** — create the 5 files with section headers only, no content. Verify structure is navigable.
3. **Port KEEP/UPDATE fragments** — for each row in §4 marked KEEP/UPDATE, verify facts against code, rewrite into target location.
4. **Fill in DELETE-replacements** — new content for sections that replace deleted architecture (e.g., `NoteStore` trait, `raw_memories` schema, new Dream stages).
5. **Cross-link pass** — every forward reference verified, no dangling `[...](...)` links.
6. **Fact audit** — final pass: grep each new doc for `FactRetrieval|graph_nodes|graph_edges|MemoryStore::|aleph://|layer.*tier` → expected zero hits in production-code claims. References to "old design" in migration notes are fine if labeled.
7. **Delete old doc** — remove original `MEMORY_SYSTEM.md` and replace with the new overview.
8. **Commit each file individually** with scope-specific commit messages.

No production code changes. No test additions. This is a pure docs PR.

---

## 7. Success Criteria

| Criterion | Verification |
|---|---|
| 5 new docs exist at target paths | `ls docs/reference/MEMORY_SYSTEM.md docs/reference/memory/*.md` |
| Each doc within size budget | `wc -l` for each, within ±50 of target |
| Zero obsolete references | `grep -rn "MemoryStore::\|FactRetrieval\|graph_nodes\|VFS\|aleph://\|WikiIngestStage\|TunnelDiscoveryStage\|SummarizeStage" docs/reference/memory docs/reference/MEMORY_SYSTEM.md` → 0 production-code claims |
| All trait signatures match code | Spot-check 10 signatures against `src/memory/` source |
| Cross-links resolve | Manual walk: every `[...](path)` points to an existing file |
| Old sections accounted for | Every section of the original 899-line doc has a row in §4 of this spec |

---

## 8. Risks

| Risk | Mitigation |
|---|---|
| Partial re-verify — a "KEEP" fragment turns out to be stale | Write mandates §5.1: every KEEP is re-read from code, not copied from old doc |
| Scope creep: someone wants to add Cortex detail mid-flight | Non-goals §2 is explicit; defer to a separate spec |
| Circular links between the 5 docs | Plan designates one owner per topic (§4). Other docs forward-link only |
| Old doc deleted before new ones ready | Keep old doc present until all 5 new docs are landed and reviewed |
| Code drifts again during writing (active refactor branch) | Pin a commit SHA in each new doc's front-matter note or accept drift — reviewer catches at merge time |

---

## 9. Out of Scope

- Updating `ARCHITECTURE.md` / `TOOL_SYSTEM.md` / `GATEWAY.md` memory sections beyond adding a pointer to the new doc tree.
- Removing dead `src/memory/decay.rs` or `MemoryStrength` — separate cleanup task.
- Documenting `src/memory/cortex/` internals — brief mention only.
- Translating to Chinese — new docs remain English (reference/* convention).
- Adding usage walkthroughs beyond what the old doc provided.
