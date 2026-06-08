# Memory Entity Graph (Gap A) — Design

> **Status:** Approved (brainstorming complete, 2026-06-08)
> **Scope:** Single implementation plan. Aleph-native entity graph: entities as
> first-class `entity/` notes, relationships as typed edges encoded in
> frontmatter, extracted inline by the existing compound ingestor.

## 1. Purpose

Give the memory system an **entity-centric layer** without resurrecting the
deprecated structured triple store. mem0's value — "know who/what the memories
are about, and how they relate" — is delivered the Aleph way: entities become
ordinary markdown notes, and relationships become **typed wikilink edges** that
live in the note's frontmatter (the source of truth) and are mirrored into a
rebuildable SQLite index column.

### Why not a faithful mem0 triple store

Three deliberate Aleph choices rule it out:

1. **The structured graph was already removed.** `graph_nodes` / `graph_edges`
   / `memory_entities` were `DROP`'d from the memory schema
   (`src/memory/store/sqlite/schema/migrations.rs:143-156`); the documented
   replacement is "Knowledge Notes with wikilink-based linking", and
   `ripple/task.rs:107` stubs the old graph-tunnel discovery with
   "Future: use notes_links."
2. **"Markdown is the source of truth; SQLite is a rebuildable index"**
   (MEMORY_SYSTEM.md §2). Any edge store must be reconstructible from the `.md`
   files.
3. **R3 (core minimalism) + R7/R9 (LLM sovereignty, intelligence in the
   prompt) + P6 (YAGNI).** The wikilink graph already exists — it is merely
   untyped. The compound ingestor already runs one LLM call that emits
   `create`/`append`/`link` ops. We extend that, adding **zero** new LLM calls.

## 2. Architecture

The compound ingestor (`src/memory/notes/ingest/ingestor.rs`) — the single LLM
call already made when raw memories are compressed into notes — additionally:

- creates/append **entity notes** under a new `entity/` category, and
- expresses relationships between entities via a `relations:` frontmatter list.

The relation is parsed from frontmatter into a new nullable
`notes_links.relation` column (rebuildable index), and surfaced as an edge label
in `memory_explore`/ripple output. Entity notes are ordinary notes, so they
enter hybrid (FTS + vector) retrieval the moment they are written.

```
raw_memories ──► CompoundIngestor.plan() (existing single LLM call)
                     │  emits PageOp::{Create,Append,...} now WITH optional relations
                     ▼
                 apply.rs ──► entity/<slug>.md   (frontmatter: relations:[{to,type,confidence}])
                     │
                     ▼
                 NoteIndexer ──► notes_index · notes_fts · notes_vec_{dim}
                                 notes_links(+ relation column)   ◄── rebuildable from .md
                     │
                     ▼
          hybrid retrieval (entity notes are normal notes)
          memory_explore/ripple output annotates each edge with its relation label
```

## 3. Data model — the typed edge in markdown

A new note category `entity/`. Entity notes reuse the existing `KnowledgeNote`
shape (`src/memory/notes/note/mod.rs:37`) — title, tags, facts — plus one
additive frontmatter block.

```yaml
---
category: entity
title: Alice
aliases: [Ali, A. Chen]          # dedup / alias resolution (reuses reference-note aliases)
tags: [person]
created: "2026-06-08"
updated: "2026-06-08"
relations:                        # NEW — typed edges, reconstructible from this file
  - to: entity/acme-corp
    type: works_at
    confidence: 0.9
  - to: entity/bob
    type: colleague
    confidence: 0.7
---
- Alice leads the Acme migration project. <!-- src: raw/abc, origin: raw_source, inferred: false -->
- Sits in the Berlin office.
```

### `Relation` struct

```rust
/// A typed, directed edge from the containing note to `to`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Relation {
    /// Target note path ("entity/bob") or raw wikilink target.
    pub to: String,
    /// Free-form snake_case relationship verb chosen by the LLM
    /// (no fixed taxonomy — R7 LLM sovereignty). E.g. "works_at", "colleague".
    #[serde(rename = "type")]
    pub rel_type: String,
    /// LLM-judged edge confidence, clamped to [0,1]; defaults to 1.0 when absent.
    #[serde(default = "default_relation_confidence")]
    pub confidence: f32,
}
```

Notes:

- `type` is free-form (LLM chooses); we impose **no** taxonomy.
- `confidence` is optional, defaults to 1.0, clamped to `[0,1]` at parse and at
  apply time.
- The body still carries plain `[[entity/bob]]` wikilinks for human
  readability and existing ripple traversal. The **typed** layer is additive in
  frontmatter — it never replaces body wikilinks.

## 4. Extraction — extend ingestor op schema + prompt

### 4.1 `PageOp` extensions (`src/memory/notes/ingest/plan.rs:19`)

```rust
Create {
    note_path: String,
    title: String,
    summary: String,
    #[serde(default)] facts: Vec<String>,
    #[serde(default)] links: Vec<String>,
    #[serde(default)] tags: Vec<String>,
    #[serde(default)] relations: Vec<Relation>,   // NEW
},
Append {
    note_path: String,
    #[serde(default)] new_facts: Vec<String>,
    #[serde(default)] new_links: Vec<String>,
    #[serde(default)] new_relations: Vec<Relation>, // NEW
},
```

Both fields are `#[serde(default)]` — existing plans (and every existing unit
test fixture) parse byte-for-byte unchanged. No new op variant is introduced;
entity notes are created with the existing `Create` op targeting the `entity/`
category.

### 4.2 Prompt section (`src/memory/notes/ingest/prompts.rs` / `ingestor.rs`)

Add a concise block to the compound-plan system prompt:

> **Entities & relationships.** When the source names durable entities (people,
> organisations, projects, concepts), create or append `entity/<slug>` notes for
> them. Express relationships between entities with the op's `relations` field —
> each `{to, type, confidence}` where `type` is a concise snake_case verb you
> choose (e.g. `works_at`, `depends_on`, `colleague`) and `confidence` is 0–1.
> Reuse existing entity notes shown in the related pages; never duplicate an
> entity that already exists.

The existing `related`-pages context + `content_hash` dedup + reference-token
resolution already prevent duplicate entity notes — no new dedup machinery.

## 5. Storage / index

### 5.1 `KnowledgeNote` + parsing/render

- `KnowledgeNote` (`note/mod.rs:37`) gains `pub relations: Vec<Relation>`
  (`#[serde(default)]` / `Default`).
- `parsing.rs::Frontmatter` gains `#[serde(default)] relations: Vec<Relation>`;
  `KnowledgeNote::from_markdown` carries it through.
- `to_markdown` (`note/mod.rs:153`) renders the `relations:` list when non-empty
  (omitted entirely when empty, so legacy notes round-trip byte-identical).

### 5.2 `notes_links` migration + upsert

- DDL (`schema/ddl.rs:103`) and an idempotent additive migration add a nullable
  `relation TEXT` column. `ALTER TABLE notes_links ADD COLUMN relation TEXT`
  guarded so re-running is a no-op (column-exists check, matching the existing
  migration style).
- The set-diff upsert (`store/sqlite/notes.rs:126-193`) carries `relation`
  through: a note's outgoing edges are the union of body wikilinks
  (`relation = NULL`) and frontmatter relations (`relation = <type>`). The
  diff key remains `(agent_id, from_note, to_note)`; `relation` and `to_raw`
  travel with the row.
- Because every edge is derived from the `.md`, a full reindex rebuilds the
  column. The source-of-truth invariant holds.

### 5.3 Edge resolution

`to` targets resolve through the existing `resolve_wikilink` (exact path if it
contains `/`, else global filename search). Unresolved targets store
`to_raw = <raw>` and `to_note = <raw>` exactly like today's unresolved
wikilinks — no new failure mode.

## 6. Retrieval (this spec's scope)

- **Entity notes need no retrieval change** — they are ordinary notes and enter
  hybrid (FTS + vector) retrieval immediately.
- **`memory_explore`/ripple edge labels.** `RippleResult` expanded facts
  (`src/memory/ripple/`) are annotated with the originating edge's `relation`
  (read from the new column; falls back to a neutral "links" label when NULL).
  This is an additive output enrichment — no change to the BFS algorithm.

**Explicitly deferred:** relation-aware multi-hop traversal (following typed
edges), entity-centric query expansion, and any dedicated entity-graph tool.

## 7. Error handling & safety (P7)

- A malformed `relations` entry (missing `to`, empty `type`) is dropped with a
  `warn!` and never fails the whole plan — same posture as the existing
  `repair_kind_tags` / reference-token resolution steps.
- `confidence` outside `[0,1]` is clamped, not rejected.
- Unresolvable `to` falls back to raw-target storage (see §5.3).
- All paths scoped by `agent_id` (unchanged isolation guarantee).

## 8. Testing (grep-guard + unit tests; no `cargo` per project protocol)

1. **Frontmatter round-trip:** a note with `relations` parses, `to_markdown`
   re-renders, re-parse equals the original (`note/tests.rs`).
2. **Legacy round-trip unchanged:** a note with no `relations` renders
   byte-identical to today (regression guard).
3. **`notes_links` upsert:** writing a note with a body wikilink + a frontmatter
   relation yields one `relation=NULL` row and one `relation=<type>` row;
   removing the relation deletes only that row (set-diff).
4. **Migration idempotency:** applying the `ADD COLUMN` migration twice is a
   no-op.
5. **Ingestor apply:** a plan with `Create{relations}` + `Append{new_relations}`
   writes the `entity/` note and indexes the edges; a plan with **no** relations
   produces output identical to the pre-change path (backward-compat).
6. **Explore label:** ripple output carries the `relation` label for a typed
   edge and the fallback label for a plain wikilink.

Grep guards (compiler substitute): after each task, verify every caller of a
changed signature is updated (`grep -rn '\.plan(' / 'PageOp::Create' /
'from_markdown' / 'NOTES_LINKS_DDL'`), and that no production path references a
dropped field.

## 9. Explicitly NOT in scope (YAGNI / honesty)

- No separate `entities` / `edges` triple-store tables — the deprecated ones
  stay dropped.
- No relation-aware traversal, no entity-centric query expansion, no
  `entity_graph` tool.
- No offline Dream-daemon extraction stage — extraction is inline only.
- No fixed relation taxonomy or relation normalisation.
- No automatic backfill of entity notes over the existing corpus (only
  newly-compressed memories produce entities; a one-shot backfill, if ever
  wanted, is a separate follow-up).

## 10. Backward compatibility

Every change is additive and `#[serde(default)]`-gated:

- Old notes (no `relations:`) parse and render unchanged.
- Old ingestor plans (no `relations` field) parse unchanged.
- The new `notes_links.relation` column is nullable; existing rows and the
  existing diff logic are unaffected.
- A full reindex of a pre-existing corpus produces the same rows as today plus
  `relation=NULL` everywhere — no data migration required.
