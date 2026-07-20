# Aleph Note Layer — LLM-wiki Optimization Design

**Date:** 2026-05-03
**Author:** AI co-authored brainstorming session
**Status:** Design (awaiting implementation plans)

## 0. Background

Karpathy's *llm-wiki* (April 2026) describes a memory pattern where an LLM incrementally compiles raw sources into a persistent, interlinked markdown wiki rather than re-deriving knowledge per query. Aleph's L1 note layer is already structured along these lines (`~/.aleph/memory/note/{agent_id}/{category}/*.md` + rebuildable SQLite indexes; index.md + log.md control plane via `notes/orientation/`; two-pass compound ingest via `notes/ingest/`; lint pipeline via `dreaming/stages/`).

A deeper analysis (`/Volumes/TBU4/Workspace/llm-wiki2.md`) surfaces five risks the bare pattern leaves unaddressed:

1. **Hallucination feedback loop** — the LLM reads its own past wiki pages as background and re-promotes its own claims to "fact" on the next ingest (llm-wiki2 §31).
2. **Partial-context updates** — ingests see only a few related pages, producing locally consistent but globally contradictory output (llm-wiki2 §32).
3. **Irreversible information loss via summarization** — nuance and provenance dropped at compile time can't be reconstructed (llm-wiki2 §33).
4. **Loss of provenance** — generated pages without source citations can't be audited or reverified (llm-wiki2 §34).
5. **Silent staleness** — old claims continue to appear as current facts; supersession is implicit (llm-wiki2 §29-30).

This spec defines a focused optimization of Aleph's existing note layer that addresses these risks without destructive refactoring. The work is decomposed into four sequential phases with phase-level verification gates, packaged as one design document and four implementation plans.

### 0.1 Conceptual mapping (LLM-wiki ↔ Aleph)

| LLM-wiki concept | Aleph module |
|---|---|
| **Raw sources** (immutable) | `raw_memories` SQLite table + `compression/` |
| **Wiki** (derived knowledge) | `~/.aleph/memory/note/{agent_id}/{category}/*.md` + `notes_*` SQLite indexes |
| **Schema** (behavior contract) | `note_schema` builtin tool + `notes/orientation/schema.rs` |
| **Operational control plane** (`index.md`, `log.md`) | `notes/orientation/` (`FsNoteOrientation`) |
| **Lint / health-check** | `dreaming/stages/` (note_lint, note_drift, note_consolidate, note_decay, note_synthesis, daily_digest, index_refresher) |
| **Governance / review** (added by C2) | `notes/governance/` (gate, supersession), `notes_review_queue`, `note_review` dream stage |

### 0.2 Scope decisions (locked)

- **Phase order:** A (bug fixes / correctness) → B (performance / cadence) → C2 (governance / anti-feedback / supersession) → R2 (`fact` → `note` naming cleanup).
- **C2 depth:** review queue + supersession lifecycle + paragraph-level provenance + ingest source-tagging + recall-signal-driven confidence decay + new `contradiction` category. Adversarial ingest (C3 in brainstorming) is deferred to a separate spec.
- **R2 depth:** rename event-sourcing layer (`*Fact*` → `*Note*` enums + `fact_id` → `note_path` payload field) and frontmatter `source_facts` → `source_notes` (with `#[serde(alias)]`). Keep `KnowledgeNote.facts: Vec<String>` (the page-level "claims" abstraction) intact — it is the carrier for paragraph-level provenance in C2.
- **Packaging:** one spec covering A+B+C2+R2; four implementation plans (one per phase) with explicit verification gates between phases.
- **R3 / R8 / R11 redlines:** no new heavyweight dependencies; all governance LLM judgment lives in the existing `dreaming` cadence; no new logic added to `src/harness/`.

### 0.3 Architectural redline check

- **R3 Core minimalism:** three new SQLite tables added (`notes_provenance`, `notes_review_queue`, `notes_review_archive`); all are rebuildable indexes or audit logs, not new truth sources.
- **R8 LLM sovereignty:** `note_review` dream stage performs LLM second-pass review — preserved. `governance::gate` thresholds are simple comparisons (not pretending to be reasoning), routing decisions only.
- **R11 Thin harness:** all changes land in `src/memory/`, `src/builtin_tools/note_*`, `src/memory/store/`. `src/harness/` not touched.

## 1. Architecture overview

### 1.1 Existing modules (reused, unchanged or lightly extended)

```
src/memory/
├── notes/
│   ├── note.rs              <- KnowledgeNote + Frontmatter (A/C2/R2 add fields)
│   ├── wikilink.rs          <- regex (A1 fixes pipe-alias)
│   ├── indexer.rs           <- NoteIndexer (A2/B1/B3/B5 here)
│   ├── store.rs             <- NoteStore trait (B1 adds diff-upsert)
│   ├── retrieval.rs / note_retrieval/hybrid.rs  <- unchanged
│   ├── orientation/         <- index.md / log.md (B4 adjusts cadence)
│   ├── ingest/              <- CompoundIngestor (C2 adds gate hook)
│   ├── dedup.rs / extractor.rs / search_result.rs   <- unchanged
│   ├── profile/, query_filer/                       <- unchanged
│   └── governance/  (NEW)   <- gate.rs, supersession.rs (C2)
├── compression/             <- Scheduler / Service (unchanged)
├── dreaming/stages/         <- adds note_review.rs (C2); extends note_decay.rs (C2.7)
├── events/                  <- R2 renames commands & enum
└── store/sqlite/            <- schema.rs adds 3 tables (C2.9)
```

### 1.2 Two raw → note write paths (both must be governed)

A consequence of the dreaming subsystem is that `feedback_distill.rs` reads `RawMemorySource::Correction` from `raw_memories` and writes notes directly via `NoteIndexer`. C2 governance (gate + provenance) MUST cover both paths:

1. **Compound ingest path:** `compression/CompoundIngestor` → `notes/ingest/apply.rs` → `governance::gate.evaluate` → `NoteIndexer::write_note`
2. **Feedback distill path:** `dreaming/stages/feedback_distill.rs` → `governance::gate.evaluate` → `NoteIndexer::index_file`

Both call `governance::gate.evaluate` before any write. The third path (`note_manage` LLM tool for create/update/append/delete) also routes through the gate.

### 1.3 Per-phase change inventory

**Phase A (bug fixes)**
- `notes/wikilink.rs` — pipe-alias regex
- `notes/note.rs` — YAML quoting, fact parser sub-bullet support, `sanitize_title` empty-result guard, frontmatter date round-trip
- `notes/indexer.rs` + `store/sqlite/schema.rs` — `notes_links` migration: add `to_raw` column; persist resolved `to_note`
- `dreaming/stages/note_lint.rs` — late-resolution lint rule

**Phase B (performance & cadence)**
- `store/sqlite/notes.rs` — set-diff upsert for `notes_links` and `notes_fts`
- `store/sqlite/schema.rs` — composite index `(agent_id, filename)` on `notes_index`
- `notes/indexer.rs` — parallel `full_rebuild` per category
- `embedding_manager.rs` + `notes/indexer.rs` — pending-embedding queue with batch flush
- `notes/orientation/` + `notes/ingest/apply.rs` + `dreaming/stages/feedback_distill.rs` — `index.md` refresh on each ingest commit

**Phase C2 (governance)**
- `notes/note.rs` — frontmatter fields `status`, `supersedes`, `superseded_by`; per-fact provenance HTML comment parser
- `notes/governance/gate.rs` (new) — `NoteWriteGate` trait, `DefaultNoteWriteGate`, `CandidateNote`, `GateOutcome`
- `notes/governance/supersession.rs` (new) — frontmatter ↔ body `## Superseded` section bidirectional sync
- `dreaming/stages/note_review.rs` (new) — async LLM review consumer of `notes_review_queue`
- `dreaming/stages/note_decay.rs` — recall-signal-driven confidence decay with severity floor
- `notes/ingest/retrieve.rs` + `notes/ingest/prompts.rs` — origin tagging in prompts and post-processing
- `notes/indexer.rs` + `builtin_tools/note_manage.rs` — `contradiction` category registered
- `store/sqlite/schema.rs` — `notes_provenance`, `notes_review_queue`, `notes_review_archive` tables

**Phase R2 (naming cleanup)**
- `events/commands.rs` — rename `*FactCommand` → `*NoteCommand`; param `fact_id` → `note_path`
- `events/types.rs` — rename `MemoryEvent::Fact*` → `Note*` with `#[serde(alias)]`; payload `fact_id` → `note_path` and `source_fact_ids` → `source_note_paths` with aliases
- `events/handler.rs` + `events/projector.rs` — `fold_events_to_fact` → `fold_events_to_note`; downstream call-site updates
- `notes/note.rs` — frontmatter `source_facts` → `source_notes` with `#[serde(alias)]`; `KnowledgeNote.source_facts` field renamed
- Tracing fields and module doc strings — `fact_id` → `note_path`
- `docs/reference/memory/NOTES.md` §12 documentation update

## 2. Phase A — Bug fixes / correctness

Six items, each with location, current behavior, fix sketch, and new tests.

### A1. Wikilink pipe-alias regex broken

- **Location:** `src/memory/notes/wikilink.rs:10`
- **Current:** `r"\[\[([^\]]+)\]\]"` captures `[[rust|Rust 学习]]` whole as target `"rust|Rust 学习"`.
- **Fix:**
  - Regex becomes `r"\[\[([^\]\|]+)(?:\|([^\]]*))?\]\]"`.
  - `extract_wikilinks(text) -> Vec<String>` returns targets only (callers unchanged).
  - New `extract_wikilinks_with_alias(text) -> Vec<(String, Option<String>)>` for future use.
  - `rewrite_wikilinks(text, old, new)` preserves alias: `[[old|alias]]` → `[[new|alias]]`.
  - `remove_wikilink(text, name)` removes the entire `[[name|alias]]` token.
- **Tests:** four new — extract pipe-alias forms; rewrite preserves alias; remove drops full token; mixed pipe + plain forms in same body.

### A2. `notes_links.to_note` stores raw unresolved targets

- **Location:** `src/memory/notes/indexer.rs` (write) + `src/memory/store/sqlite/schema.rs:notes_links` (DDL) + `src/memory/store/sqlite/notes.rs` (read)
- **Current:** `to_note` is the verbatim wikilink target; `[[rust]]` and `[[reference/rust]]` produce different rows.
- **Fix:**
  - Schema migration: add column `to_raw TEXT NOT NULL`. Backfill `to_raw = to_note` for existing rows.
  - On `index_note`, run `resolve_wikilink` on every raw target. Resolved success: write `(to_raw=raw, to_note=resolved)`. Resolution failure: write `(to_raw=raw, to_note=raw)` and let lint resolve later.
  - Add lint rule in `dreaming/stages/note_lint.rs`: scan rows where `to_raw != to_note` only at `to_note=raw_form`, retry resolution, update `to_note` on success.
  - All graph queries (`get_incoming_links`, `get_neighbors`) use `to_note`.
- **Tests:** incoming-links resolves mixed link forms (both `[[rust]]` and `[[reference/rust]]` hit the same target); late resolution test (link first, target note created later, lint resolves).

### A3. YAML inline-array tags / source_notes lack quoting

- **Location:** `src/memory/notes/note.rs:148, 159-166` (writer)
- **Current:** `tags: [editor, vim]` is flow-style without quoting; tags containing `,`, `:`, `[`, `]`, `#`, etc. break round-trip.
- **Fix:** Add helper `yaml_inline_array(items: &[String]) -> String`:
  - If element contains none of `'`, `"`, `,`, `:`, `[`, `]`, `{`, `}`, `#`, `&`, `*`, `!`, `|`, `>`, `%`, `@`, `` ` `` — emit as-is.
  - Otherwise wrap in single quotes; double internal `'` to `''`.
  - Output form: `[a, 'b: with colon', 'c''quote']`.
- Apply to `tags`, `supersedes`, `superseded_by`, and `source_notes` (post-R2).
- **Tests:** round-trip a tag containing each special character; empty array stays `[]`.

### A4. Frontmatter date round-trip is fragile

- **Location:** `src/memory/notes/note.rs:36-38, 149-150`
- **Current:** `created: Option<String>` — but YAML may parse `2026-04-01` (unquoted) as a native date type, deserializing into `String` is parser-version-dependent and fragile.
- **Fix:**
  - Writer always quotes: `created: "2026-04-01"`.
  - Reader uses a custom deserializer that accepts String, native YAML date, and Null; converts each to `Option<String>` in `YYYY-MM-DD` form.
- **Tests:** legacy unquoted, new quoted, native YAML date — all round-trip identically.

### A5. `extract_facts` drops sub-bullets and continuation lines

- **Location:** `src/memory/notes/note.rs:240-247`
- **Current:** `trimmed.strip_prefix("- ")` extracts every line starting with `- ` as a top-level fact; indented sub-bullets are flattened to peer facts; non-`-`-prefixed continuation lines are silently dropped.
- **Fix:** state-machine parser:
  - A top-level fact starts at indentation 0 with `- `.
  - Lines indented >= 2 columns (space or tab) attach to the current fact, preserving original indentation in the appended text.
  - Empty line ends the current fact.
  - Output remains `Vec<String>` but each `String` may now contain `\n` for nested structure.
- **Side effect:** `body_text()`, embedding text, FTS body all reflect richer fact text. This is intentional — the multi-line fact is the carrier for paragraph-level provenance (C2.2).
- **Tests:** single-level, nested, mixed continuation, empty-line boundary.

### A6. `sanitize_title` empty result is silently accepted

- **Location:** `src/memory/notes/note.rs:253-259`
- **Current:** Inputs like `..`, `///`, `||||` all sanitize to `""`; the empty string then becomes filename `"".md`.
- **Fix:** Change signature to `pub fn sanitize_title(&str) -> Result<String, AlephError>`. Return `Err(AlephError::Validation { message: "note title sanitizes to empty" })` for empty / all-dot / all-whitespace results. Callers (`indexer.rs`, `note_manage.rs`) already use `?` and naturally propagate.
- **Tests:** `""`, `".."`, `"///"`, `" "` all return `Err`; normal inputs still `Ok`.

### A7. Phase A verification gate

1. `cargo test -p alephcore --lib memory::notes::wikilink memory::notes::note` green with at least 12 new tests.
2. `cargo test -p alephcore --lib memory::store::sqlite::notes` green with `incoming_links_resolves_mixed_link_forms` passing.
3. `cargo test -p alephcore --lib memory::dreaming::stages::note_lint` green with `lint_resolves_pending_links` passing.
4. Manual smoke: `full_rebuild` against the author's `~/.aleph/memory/note/` produces no new errors.
5. A is independently shippable before R2 (A and R2 do not share file-level changes).

## 3. Phase B — Performance & cadence

### B1. `notes_links` / `notes_fts` set-diff upsert

- **Location:** `src/memory/store/sqlite/notes.rs:92, 107, 114, 132-138`
- **Current:** Every `index_note` deletes all `(from_note=?, agent_id=?)` rows and inserts the new set; FTS row deleted and inserted whole.
- **Fix:**
  - Read existing `(from_note=?)` rows into a `HashSet<(to_raw, to_note)>`. Compare with the new computed set. INSERT only added rows; DELETE only removed rows; no-op on intersection.
  - For FTS: compare `KnowledgeNote.content_hash`; if unchanged skip rebuild. If changed, hash `body_text()` separately; if body identical (frontmatter-only change) skip FTS rebuild.
  - Wrap both steps in a single SQLite transaction.
- **Expected gain:** ~80% reduction in steady-state writes during dream lint sweeps.
- **Tests:** unit set-diff (add only / remove only / mixed / no-op); benchmark `notes_index_writes` shows >=70% write reduction.

### B2. `find_by_filename` composite index

- **Location:** `src/memory/store/sqlite/schema.rs`
- **Fix:** Add `CREATE INDEX IF NOT EXISTS idx_notes_filename_agent ON notes_index(agent_id, filename)`. Mark legacy `idx_notes_filename` deprecated; remove in a later release.
- **Verify:** `EXPLAIN QUERY PLAN SELECT path FROM notes_index WHERE agent_id=? AND filename=?` reports `USING INDEX idx_notes_filename_agent`.

### B3. Parallel `full_rebuild` per category

- **Location:** `src/memory/notes/indexer.rs:116`
- **Fix:**
  - Use `tokio::task::JoinSet`. For each `category` in `CATEGORY_DIRS`, spawn one task that walks `memory_dir/{agent_id}/{category}/` and parses every `.md` file.
  - Limit concurrency with `Semaphore::new(num_cpus::get())` to avoid FD / WAL thrash.
  - Parsed `KnowledgeNote` records are forwarded to a single SQLite-writer task (sqlite stays serial).
  - `IndexStats { indexed, skipped, errors }` is aggregated across tasks.
- **Tests:** `full_rebuild_parallel_matches_serial_results` (1000 fake notes, byte-equal output); benchmark records absolute time.

### B4. `index.md` refresh becomes ingest-driven

- **Location:** `src/memory/notes/orientation/` + `src/memory/notes/ingest/apply.rs` + `src/memory/dreaming/stages/feedback_distill.rs` + `src/memory/dreaming/stages/index_refresher.rs`
- **Current:** `index_refresher` runs only inside `dream` (Conserve / Synthesize / default strategies). Between dreams, `index.md` is stale up to one hour.
- **Fix:**
  - New trait method `NoteOrientation::refresh_index_after_ingest(agent_id, batch_summary)` — partial refresh of categories touched by this ingest only.
  - Called at the tail of `apply.rs::ingest_batch`.
  - Called at the tail of `feedback_distill.rs::execute` (the second raw → note path).
  - `index_refresher.rs` retained for whole-index health rewrites during dream — at lower frequency (one whole-rewrite per dream cycle).
  - `log.md` already ingest-driven (`profile/synthesizer.rs`, `query_filer/filer.rs`); only the `LogAction` enum is extended in C2 to include `LogAction::ReviewQueued`, `LogAction::Superseded`.
- **Tests:** `apply_triggers_partial_index_refresh_on_each_ingest`; integration test using `tokio::time::pause()` to simulate two ingests with no dream between, verifying `index.md` is current after the second ingest.

### B5. Embedding generation batched via pending queue

- **Location:** `src/memory/embedding_manager.rs` + `src/memory/notes/indexer.rs`
- **Current:** Each `index_note` blocks on a synchronous embedding provider call; eight notes in one ingest take eight serial network round-trips.
- **Fix:**
  - `NoteIndexer::index_file` no longer generates the embedding inline; it pushes `(agent_id, path, body_text)` onto an in-memory `pending_embeddings: Mutex<Vec<...>>`.
  - `EmbeddingManager::flush_pending(batch_size)` batches embed calls and writes to `notes_vec_*`.
  - Flush is triggered: (a) at `apply.rs::ingest_batch` tail; (b) at any dream stage tail; (c) every 60 seconds via background tick.
  - Retrieval gracefully degrades: if a path is in `pending_embeddings` and not yet in `notes_vec_*`, hybrid search returns FTS-only results for that note. No retrieval errors.
- **Crash recovery:** On startup, `pending_embeddings` is empty; `full_rebuild` regenerates any missing embeddings.
- **Tests:** `pending_embeddings_flush_writes_all`; integration test verifying retrieval works before and after flush.

### B6. Phase B verification gate

1. `cargo bench -p alephcore --bench notes_index_writes` shows >=70% write reduction post-fix.
2. `cargo bench -p alephcore --bench full_rebuild_1000_notes` shows >=4x speedup on an 8-core machine.
3. `EXPLAIN QUERY PLAN` confirms composite index usage.
4. Integration: ingest triggers partial `index.md` refresh; `feedback_distill` does the same.
5. Integration: embedding flush leaves no notes without vectors.
6. `cargo test -p alephcore --lib memory::notes` remains green (A regressions blocked).

### B7. Phase ordering note

- A must ship before B: B1's diff-upsert key includes `to_raw`, which A2 introduces.
- C2 schedules ingest-tail review queue inserts immediately before B4's `refresh_index_after_ingest`. Order is `gate → apply → refresh_index → log.md append`.

## 4. Phase C2 — Governance / anti-feedback / supersession

### C2.1 Frontmatter schema extension (additive, fully backward-compatible)

- **Location:** `src/memory/notes/note.rs:30-45` (`Frontmatter`) + `note.rs:144-167` (writer)
- **New fields** (each `#[serde(default)]`):
  ```yaml
  status: active            # active | deprecated | contradicted (default active)
  supersedes: []            # paths this note replaces
  superseded_by: []         # paths replacing this note (maintained by governance)
  ```
- **`KnowledgeNote::default()`** updated to include `status: Active`, `supersedes: Vec::new()`, `superseded_by: Vec::new()` so all existing `..Default::default()` call sites continue to compile.
- **Writer** uses A3's `yaml_inline_array` for the two list fields; status emitted lowercase.
- **Tests:** legacy note without new fields parses to defaults; new fields round-trip; `status: contradicted` deserializes correctly.

### C2.2 Paragraph-level provenance via inline HTML comments + rebuildable index

- **Format:** Each fact bullet may carry an inline trailing comment:
  ```markdown
  - The user prefers Vim. <!-- src: raw/abc-123, origin: raw_source, inferred: false -->
  - Synthesized: keyboard-centric workflow. <!-- origin: inferred, inferred: true -->
  - Recall from prior note. <!-- src: note/preference/editor-vim, origin: prior_note, inferred: false -->
  ```
- **`origin` values:** `raw_source` | `prior_note` | `inferred` | `legacy`. `legacy` is auto-assigned to facts in pre-existing notes lacking comments.
- **Parser:** `extract_provenance_markers(body) -> Vec<FactProvenance>` runs after A5's `extract_facts` and aligns by index. Regex: `<!-- src:\s*([^,]+),\s*origin:\s*(raw_source|prior_note|inferred|legacy),\s*inferred:\s*(true|false)\s*-->`.
- **`KnowledgeNote.fact_provenance: Vec<FactProvenance>`** — new field, length matches `facts`. Default is `legacy` for missing comments.
- **FTS body sanitation:** comments stripped before insertion into `notes_fts.content` (do not pollute search).
- **Persistence:** Mirrored into `notes_provenance` SQLite table (DDL in C2.9). Fully rebuildable from markdown.
- **Tests:** every origin round-trips; mixed (commented + uncommented) facts in one note; FTS body free of comment artifacts.

### C2.3 `notes/governance/gate.rs` — unified raw → note write gate

- **New files:** `src/memory/notes/governance/mod.rs`, `src/memory/notes/governance/gate.rs`
- **Trait:**
  ```rust
  pub trait NoteWriteGate: Send + Sync {
      // evaluate has the side effect of writing into notes_review_queue when it
      // returns Defer, and into notes_review_archive when it returns Reject.
      // This concentrates write logic in one place; callers (apply.rs,
      // feedback_distill.rs, note_manage.rs) only inspect the outcome.
      async fn evaluate(&self, candidate: &CandidateNote) -> Result<GateOutcome, AlephError>;
  }

  pub enum GateOutcome {
      Accept(CandidateNote),
      Defer { queue_id: String, reason: String },   // queue_id is the freshly-inserted row id
      Reject { archive_id: String, reason: String },// archive_id is the freshly-inserted row id
  }

  pub struct CandidateNote {
      pub agent_id: String,
      pub category: String,
      pub note: KnowledgeNote,
      pub source_path: Option<String>,
      pub fact_provenance: Vec<FactProvenance>,
      pub action: NoteWriteAction, // Create | Update | Append | Delete
      pub bypass_review: bool,     // true when re-applying from review queue
      pub contradicts_existing: bool, // set by ingest plan
  }
  ```
- **`DefaultNoteWriteGate` triggers `Defer`** on (configurable thresholds shown):
  - `confidence < 0.5`
  - `severity >= High` AND `confidence < 0.8`
  - `contradicts_existing == true`
  - `bypass_review == true` always overrides — already-reviewed candidates are admitted unconditionally (prevents review loops).
- **Delete action:** `gate.evaluate` for `NoteWriteAction::Delete` always returns `Accept` for `severity < Critical` notes; for `Critical` notes returns `Defer` so an LLM-driven delete cannot silently remove safety-relevant claims.
- **Mount points:**
  - `src/memory/notes/ingest/apply.rs::write_note` — gate before `NoteIndexer::write_note`.
  - `src/memory/dreaming/stages/feedback_distill.rs` — gate before `NoteIndexer::index_file`.
  - `src/builtin_tools/note_manage.rs::create/update/append` — gate before `NoteIndexer` calls.
- **Fail-closed:** Gate's SQLite path failure returns `Defer { reason: "gate unavailable" }` — never `Accept`. `Reject` is only returned when an explicit policy rule applies.
- **Tests:** four `Defer` triggers + bypass admit + fail-closed (mock SQLite error); confirm `Accept` path passes through correctly.

### C2.4 `notes/governance/supersession.rs` — frontmatter ↔ body sync

- **New file:** `src/memory/notes/governance/supersession.rs`
- **Existing precedent:** `dreaming/stages/note_drift.rs:8` already appends `## Superseded by [[X]]` to the older note's body when verdict is CONTRADICTORY.
- **Sync rules:**
  - **Body → frontmatter:** After parsing markdown, scan body for `^## Superseded by \[\[(.+)\]\]$` headings. Union targets into `superseded_by` field.
  - **Frontmatter → body:** Before writing markdown, if `superseded_by` non-empty and body lacks the corresponding section, append it.
  - Idempotent — repeated reindex never produces duplicates.
- **`supersedes` maintenance:** When a new note A declares `supersedes: [B]`, `apply.rs` writes A and concurrently calls `NoteIndexer::append_to_note(B, superseded_by += [A])`.
- **Tests:** drift body section → frontmatter sync; frontmatter → body section sync; both sides already populated → no-op.

### C2.5 `dreaming/stages/note_review.rs` — async review consumer

- **New file:** `src/memory/dreaming/stages/note_review.rs`
- **Position in dream pipeline:** Inserted after `note_lint` and before `note_consolidate` in all strategies (`Conserve`, `Synthesize`, default).
- **Algorithm:**
  1. `SELECT * FROM notes_review_queue WHERE agent_id=? AND status='pending' AND created_at < now - dwell_seconds` (default `dwell_seconds = 300`).
  2. For each candidate:
     - Deserialize `candidate_json` into `CandidateNote`.
     - Retrieve 3-5 nearest existing notes in the same category as comparison context.
     - LLM prompt: `"Approve / Reject / Rewrite this candidate. Output JSON {verdict, reason, [rewritten_content]}."`
     - `approve`: set `bypass_review = true`, hand to `apply.rs` for write; mark queue row `approved`.
     - `reject`: write to `notes_review_archive`, delete from queue.
     - `rewrite`: replace candidate content, set `bypass_review = true`, write via apply; mark queue row `rewritten`.
  3. On LLM call failure: `retry_count++`. After 3 failures, archive as `final_status = 'timeout'`.
- **Tests:** approve / reject / rewrite paths; timeout archival; retries do not duplicate writes.

### C2.6 New `contradiction` category

- **Locations:** `src/memory/notes/indexer.rs::CATEGORY_DIRS`, `src/builtin_tools/note_manage.rs:22-38` category list
- **Purpose:** Used by `note_drift.rs` when two existing notes contradict in a way not resolvable by simple supersedes (e.g., conflicting source claims). Each contradiction note records both sides via wikilinks.
- **Frontmatter convention:** `tags: [contradiction]`, `status: active` for live conflicts, `status: deprecated` once resolved.
- **Tests:** `validate_category("contradiction")` succeeds; round-trip a contradiction note.

### C2.7 Recall-signal-driven confidence decay with severity floor

- **Location:** `src/memory/dreaming/stages/note_decay.rs` (extend existing)
- **Algorithm per note `p`:**
  ```
  days_since_hit = days_since_last_recall_signal(p)   // ∞ if never hit
  decayed        = old_conf * exp(-days_since_hit / 90)
  new_conf       = max(decayed, severity_floor(p.severity))
  ```
- **Severity floors (default values, configurable):**
  | Severity | Floor |
  |---|---|
  | Low | 0.0 |
  | Med | 0.5 |
  | High | 0.7 |
  | Critical | 0.85 |
- **Write back:** Only if `|new_conf - old_conf| > 0.02` (epsilon to avoid micro-writes). Updates `confidence` frontmatter field. Does not touch `updated_at` (decay is not user-meaningful update).
- **Tests:** cold note's confidence drops; high-severity note never falls below floor; recently-hit note decays minimally; epsilon prevents spurious writes.

### C2.8 Ingest-time origin tagging (anti-feedback-loop)

- **Locations:** `src/memory/notes/ingest/retrieve.rs` (context assembly), `src/memory/notes/ingest/prompts.rs` (prompt template), `src/memory/notes/ingest/apply.rs` (post-process)
- **Behavior:**
  - `retrieve.rs` annotates context blocks: `[RAW src=raw/<id>] ...` for raw_memory excerpts; `[PRIOR_NOTE src=note/<path>] ...` for existing-note excerpts.
  - `prompts.rs` instructs the LLM: emit each fact bullet with an inline `<!-- src: ..., origin: ..., inferred: ... -->` comment. The origin selection rule given in the prompt: completely from RAW → `raw_source`; completely from PRIOR_NOTE → `prior_note`; cross-source synthesis → `origin: inferred, inferred: true`.
  - `apply.rs` post-processes the LLM draft **during `CandidateNote` construction, before `gate.evaluate`**: any fact line lacking a comment is patched to `<!-- origin: inferred, inferred: true -->` (lenient — graceful degradation, not hard rejection). This guarantees the gate always sees a consistent `fact_provenance` vector.
- **Strict retrieval mode:** `note_retrieval/hybrid.rs` config gains `strict_origin_filter: bool`. When true, hybrid search joins `notes_provenance` and excludes facts with `origin = prior_note` from cross-document synthesis answers (default false).
- **Tests:** prompt fixture contains origin rule string; apply post-process patches missing comments; strict-mode retrieval excludes `prior_note` facts.

### C2.9 SQLite schema additions

- **Location:** `src/memory/store/sqlite/schema.rs`
- **DDL** (all `IF NOT EXISTS`):
  ```sql
  -- paragraph-level provenance (rebuildable index)
  CREATE TABLE IF NOT EXISTS notes_provenance (
      id          INTEGER PRIMARY KEY AUTOINCREMENT,
      agent_id    TEXT NOT NULL,
      note_path   TEXT NOT NULL,
      fact_idx    INTEGER NOT NULL,
      origin      TEXT NOT NULL,  -- raw_source | prior_note | inferred | legacy
      source_kind TEXT,           -- raw | note | NULL
      source_id   TEXT,
      inferred    INTEGER NOT NULL DEFAULT 0,
      created_at  INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS idx_prov_path ON notes_provenance(agent_id, note_path);
  CREATE INDEX IF NOT EXISTS idx_prov_source ON notes_provenance(source_kind, source_id);

  -- review queue (state, not audit log; not rebuildable from markdown by design)
  CREATE TABLE IF NOT EXISTS notes_review_queue (
      id              TEXT PRIMARY KEY,
      agent_id        TEXT NOT NULL,
      candidate_json  TEXT NOT NULL,
      severity        TEXT NOT NULL,
      confidence      REAL NOT NULL,
      reason          TEXT NOT NULL,
      status          TEXT NOT NULL DEFAULT 'pending',
      retry_count     INTEGER NOT NULL DEFAULT 0,
      created_at      INTEGER NOT NULL,
      decided_at      INTEGER,
      decision_actor  TEXT
  );
  CREATE INDEX IF NOT EXISTS idx_review_pending
      ON notes_review_queue(agent_id, status, created_at);

  -- review archive (immutable audit log)
  CREATE TABLE IF NOT EXISTS notes_review_archive (
      id              TEXT PRIMARY KEY,
      agent_id        TEXT NOT NULL,
      candidate_json  TEXT NOT NULL,
      final_status    TEXT NOT NULL,
      reason          TEXT NOT NULL,
      created_at      INTEGER NOT NULL,
      archived_at     INTEGER NOT NULL
  );
  CREATE INDEX IF NOT EXISTS idx_archive_age
      ON notes_review_archive(archived_at);
  ```
- **Rebuildability:** `notes_provenance` is reproducible from markdown HTML comments. `notes_review_queue` is not — pending content is intentionally not in the markdown layer until reviewed. `notes_review_archive` is an audit log; its contents are not promotable to the wiki layer except by explicit user action.

### C2.10 Phase C2 verification gate

1. Frontmatter new-field round-trip green; legacy notes parse with defaults.
2. `notes_provenance` rebuilt by `full_rebuild` after table drop.
3. `governance::gate` unit tests cover four `Defer` triggers + bypass admit + fail-closed.
4. `governance::supersession` bidirectional sync green.
5. `note_review` dream stage: approve / reject / rewrite / timeout all covered.
6. `note_decay` recall-driven: cold/hot/floor cases covered.
7. Ingest origin-tagging: prompt fixture, post-process patching, strict retrieval mode all covered.
8. Full `cargo test -p alephcore --lib memory::notes memory::dreaming memory::store::sqlite` green.
9. `validate_category("contradiction")` succeeds.

### C2.11 Constraint on event sourcing

- **C2 introduces no new `MemoryEvent` variants.** Supersession, review queue, and provenance are state tables and frontmatter mutations only; downstream events (`NoteContentUpdated` after R2) implicitly cover them.
- This constraint enables the A → B → C2 → R2 ordering. If a future spec wants to surface supersession or review as first-class events, R2 must ship first.

## 5. Phase R2 — Naming cleanup (`fact` → `note`)

### R2.0 Sequencing prerequisite

R2 ships after C2 because of the C2.11 constraint (no new event variants in C2). If a later spec adds `NoteSuperseded` / `NoteReviewApproved` events, R2 must land first. Spec authors adding new events MUST verify R2 has shipped or reorder.

### R2.1 Rust symbol renames (compile-time)

- **Location:** `src/memory/events/commands.rs`, `src/memory/events/handler.rs`, `src/memory/events/projector.rs`, all callers
- **Renames:**
  | Old | New |
  |---|---|
  | `CreateFactCommand` | `CreateNoteCommand` |
  | `InvalidateFactCommand` | `InvalidateNoteCommand` |
  | `RestoreFactCommand` | `RestoreNoteCommand` |
  | `RecordAccessCommand` | `RecordNoteAccessCommand` |
  | `DeleteFactCommand` | `DeleteNoteCommand` |
  | `EventProjector::fold_events_to_fact` | `fold_events_to_note` |
  | function param `fact_id: &str` | `note_path: &str` |
- **Unchanged:** `UpdateContentCommand`, `ConsolidateCommand` (no `Fact` substring).
- **No deprecated aliases:** Rust symbols are compile-time; rename in one pass; old symbols removed.

### R2.2 `MemoryEvent` enum variant renames + serde aliases (event-log compatibility)

- **Location:** `src/memory/events/types.rs`
- **Constraint:** The event log is append-only persistent storage. Pre-R2 events serialized as JSON `{"event": {"FactCreated": {...}}}` MUST still deserialize.
- **Implementation:**
  ```rust
  #[derive(Serialize, Deserialize, ...)]
  pub enum MemoryEvent {
      #[serde(rename = "NoteCreated", alias = "FactCreated")]
      NoteCreated { note_path: String, ... },

      #[serde(rename = "NoteContentUpdated", alias = "FactContentUpdated")]
      NoteContentUpdated { note_path: String, old_content: String, new_content: String, reason: String },

      #[serde(rename = "NoteInvalidated", alias = "FactInvalidated")]
      NoteInvalidated { note_path: String, reason: String },

      #[serde(rename = "NoteRestored", alias = "FactRestored")]
      NoteRestored { note_path: String, new_strength: f32 },

      #[serde(rename = "NoteAccessed", alias = "FactAccessed")]
      NoteAccessed { note_path: String, query: String, relevance_score: f32, used_in_response: bool, new_access_count: u32 },

      #[serde(rename = "NoteConsolidated", alias = "FactConsolidated")]
      NoteConsolidated { source_note_paths: Vec<String>, consolidated_content: String },

      #[serde(rename = "NoteDeleted", alias = "FactDeleted")]
      NoteDeleted { note_path: String, reason: String },
  }
  ```
- **Behavior:** New writes use new names only; reads accept both via alias.
- **Alias retention:** At least one minor release. Removal requires a one-shot event-log migration tool to be present and verified first.

### R2.3 Payload field renames

- **Affected fields:**
  - `fact_id: String` → `note_path: String` with `#[serde(alias = "fact_id")]` on every `MemoryEvent::*` payload.
  - In `NoteConsolidated`: `source_fact_ids: Vec<String>` → `source_note_paths: Vec<String>` with `#[serde(alias = "source_fact_ids")]`.
- **Test fixture:** Old envelope JSON `{"FactCreated": {"fact_id": "x", ...}}` must deserialize as `NoteCreated { note_path: "x", ... }`.

### R2.4 Frontmatter `source_facts` → `source_notes`

- **Location:** `src/memory/notes/note.rs:43, 78-80`
- **Implementation:**
  - `Frontmatter::source_facts` → `Frontmatter::source_notes` with `#[serde(alias = "source_facts")]`.
  - `KnowledgeNote::source_facts` → `KnowledgeNote::source_notes` (Rust public field rename — call sites updated).
  - Writer emits `source_notes: [...]` only.
- **Migration:** Legacy markdown files retain `source_facts:` until next reindex/write of that file, at which point the writer emits `source_notes:` and the legacy form disappears naturally.
- **Tests:** legacy markdown round-trips via alias; rewrite emits new form; `source_notes` populated from either YAML key.

### R2.5 Tracing / log / doc string cleanup

- `tracing::*!(fact_id = ?p, ...)` → `note_path`.
- `event!(target = "memory.fact", ...)` → `memory.note`.
- Module doc comments referring to "fact" updated to "note".
- `docs/reference/memory/NOTES.md` §12 updated to describe `*Note*` events with alias compatibility note.
- **Verification:** `rg -n 'fact_id =' --no-heading src/` returns zero hits.

### R2.6 Migration policy

- **Database schema:** zero changes. `memory_events` keeps JSON envelope, alias handles old/new.
- **Optional CLI:** `aleph-server admin events migrate-naming` rewrites old-named envelopes in-place for `grep` friendliness. Not required for correctness; default not run.
- **Markdown files:** legacy `source_facts:` not actively rewritten; replaced on next natural write.
- **Rollback:** alias enables a downgrade to a pre-R2 build to read post-R2 envelopes (serde reads new but writes nothing the old code can't parse, since the old code wrote `FactCreated` and the new code writes `NoteCreated` which old code with `FactCreated` only would treat as unknown variant — see R2.7 caveat).

### R2.7 Phase R2 verification gate

1. Old envelope fixture (containing `FactCreated` / `fact_id`) deserializes into the new variant + field.
2. New writes produce only `NoteCreated` / `note_path` (verifiable via SQLite `SELECT event FROM memory_events ORDER BY id DESC LIMIT 1`).
3. Legacy markdown `source_facts: [...]` parses with `source_notes` populated.
4. Legacy markdown after one rewrite → `source_notes: [...]` present, `source_facts:` absent.
5. `cargo test -p alephcore --lib memory::events memory::notes` green.
6. Full `cargo test -p alephcore --lib` green (no A/B/C2 regressions).
7. `rg -n 'fact_id =' --no-heading src/` returns zero.
8. `docs/reference/memory/NOTES.md` §12 updated.

### R2.8 Caveat — rollback compatibility

A pre-R2 build cannot deserialize a post-R2 written envelope (it doesn't know the variant `NoteCreated`). Operators wanting downgrade safety must keep a pre-R2 build's snapshot of the event log. Forward compatibility (R2 reads pre-R2 events) is guaranteed; backward compatibility (pre-R2 reads R2 events) is not.

## 6. Data flow

### 6.1 Compound ingest path

```
raw_memories → CompoundIngestor.plan
   → CandidateNote (with fact_provenance)
   → governance::gate.evaluate
       ├─ Accept → apply.rs.write
       │   → NoteIndexer.index_file
       │       ├─ to_markdown (origin comments + status/supersedes/source_notes)
       │       ├─ store.index_note (set-diff upsert)
       │       ├─ upsert notes_provenance
       │       ├─ governance::supersession.sync (frontmatter ↔ body)
       │       └─ pending_embeddings.push
       │   → MemoryEvent::NoteCreated|NoteContentUpdated emit
       │   → orientation.refresh_index_after_ingest
       │   → orientation.append_log (LogAction::Ingested|Superseded)
       ├─ Defer → notes_review_queue (status=pending)
       └─ Reject → notes_review_archive (final_status=rejected)
```

### 6.2 Feedback distill path (second raw → note path)

Identical structure, governed by `governance::gate` symmetrically. Required by §1.2.

### 6.3 Dream pipeline order

```
note_lint → note_review → note_consolidate → note_drift → note_synthesis
  → note_decay (with recall_signals decay) → daily_digest → index_refresher (full health-check)
```

### 6.4 Query / retrieval path

- `note_retrieval/hybrid.rs`: BM25 (FTS) + vector (sqlite-vec) → RRF → MMR → recency → time-decay → confidence multiplier.
- `strict_origin_filter` config (default false): when true, joins `notes_provenance` and excludes `origin = prior_note` facts.
- If embedding for a path is in `pending_embeddings`, vector layer returns no row for that path; result is BM25-only for that note (graceful degradation).

### 6.5 `note_manage` LLM tool

- `Create / Update / Append / Delete` route through `governance::gate` (same as ingest).
- `Query / List` go straight to retrieval.

## 7. Error handling

| Failure point | Strategy |
|---|---|
| `governance::gate` SQLite unavailable | Fail-closed: `Defer { reason: "gate unavailable" }`. Items resume once SQLite recovers. |
| `note_review` LLM call failure | Candidate stays in queue; `retry_count++`. After 3 failures, archive as `final_status='timeout'`. |
| serde alias miss on legacy event | Single-event deserialization error logged; `fold_events_to_note` skips that event but continues processing. |
| Missing paragraph provenance comment | Post-process patches to `<!-- origin: inferred, inferred: true -->` rather than rejecting ingest. |
| Supersession sync conflict (frontmatter and body disagree on targets) | Take union; emit warning log; consistency restored on next reindex. |
| Wikilink ambiguous (`find_by_filename` returns multiple) | `to_note` written as raw form; `note_lint` retries on next dream. |
| YAML parse hard error (corrupt frontmatter) | `index_file` returns Err; `full_rebuild` continues with `IndexStats.errors++`. |
| `sanitize_title` empty result | Explicit `Err`; caller propagates to LLM as actionable feedback. |
| `pending_embeddings` lost on crash | Next startup `full_rebuild` regenerates embeddings; retrieval temporarily BM25-only. |

**Universal principle:** A single note failure must not block batch ingest; batch ingest failure must not block dream; dream failure must not block main session.

## 8. Testing strategy

### 8.1 Unit tests

Each file-level change (A1-A6, B1-B5, C2.1-C2.8, R2.1-R2.5) ships with at least two unit tests; specific tests are enumerated per item in Sections 2-5.

### 8.2 Integration tests

New file: `tests/memory_note_layer.rs`. Cases:

- `ingest_with_governance_gate_low_confidence_defers` — A → B → C2 end-to-end.
- `dream_review_approves_pending_then_apply` — review queue lifecycle.
- `feedback_distill_path_also_passes_governance` — second raw → note path.
- `note_drift_supersession_syncs_frontmatter_and_body` — C2.4 bidirectional sync.
- `legacy_event_envelope_with_fact_id_deserializes_as_note_path` — R2 backward compatibility.
- `legacy_source_facts_markdown_round_trips_to_source_notes` — R2 natural migration.
- `recall_signal_decay_respects_severity_floor` — C2.7 boundary.
- `incoming_links_resolve_mixed_link_forms` — A2 cross-form resolution.

### 8.3 Property tests (proptest)

Extend `src/memory/proptest_enums.rs`:

- Frontmatter round-trip preserves all fields under arbitrary `status` / `supersedes` / `superseded_by` combinations.
- Wikilink regex never panics on random ASCII / Unicode bodies (with and without pipe-alias).
- `extract_facts` does not panic on random indented / mixed-bullet input; fact count never exceeds line count.

### 8.4 Benchmarks

`cargo bench -p alephcore` adds:

- `bench_index_note_diff_upsert` — B1.
- `bench_full_rebuild_1000_notes_parallel` — B3.
- `bench_governance_gate_throughput` — C2.3 sustained >= 5,000 QPS.

### 8.5 Manual smoke

Each phase run `full_rebuild` against the author's own `~/.aleph/memory/note/` corpus. Acceptance: zero data loss, zero parse errors, no new untracked errors in `IndexStats.errors`.

### 8.6 Phase regression matrix

| Phase | Required-green test set |
|---|---|
| A | `memory::notes::wikilink memory::notes::note memory::store::sqlite::notes` |
| B | A's set + `memory::notes::indexer memory::dreaming::stages::index_refresher tests/memory_note_layer.rs` |
| C2 | A+B set + `memory::notes::governance memory::dreaming::stages::note_review` |
| R2 | A+B+C2 set + `memory::events` + complete `cargo test -p alephcore --lib` |

## 9. Open items (to surface in the implementation plans)

These intentionally-unsettled choices belong in their respective phase plans rather than this design:

- Exact threshold values in `DefaultNoteWriteGate` — defaults given (0.5 / 0.8 / Critical=0.85 floor) but should be configurable in `[memory.governance]` section of settings.
- Whether `aleph-server admin events migrate-naming` ships with R2 or in a follow-up; design assumes follow-up.
- Whether `note_review` dream stage gets a fast-track 5-minute idle trigger in addition to dream cadence; design assumes dream cadence only.
- Whether `notes_review_archive` rows ever get pruned (no auto-prune in this spec).

## 10. References

- Karpathy's LLM-wiki gist: `/Volumes/TBU4/Workspace/llm-wiki.md` (April 2026).
- Deep analysis of LLM-wiki: `/Volumes/TBU4/Workspace/llm-wiki2.md`.
- `docs/reference/memory/NOTES.md` — current note layer reference (will be updated post-implementation).
- `docs/reference/memory/RAW_MEMORY.md` — raw layer reference.
- `docs/reference/memory/RETRIEVAL.md` — retrieval pipeline reference.
- `docs/reference/memory/DREAM_DAEMON.md` — dream pipeline reference.
- Aleph CLAUDE.md — R3, R7, R8, R10, R11 redlines and design principles.
