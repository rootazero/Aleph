# Knowledge Notes (L1)

> Markdown-first persistent knowledge. Each note is one `.md` file; SQLite tables are rebuildable indexes.

## Curated Hot Memory (MEMORY.md) — sibling concept

A separate, single-file *curated hot memory* lives at
`~/.aleph/agents/{agent_id}/MEMORY.md` alongside the L1 notes library. It is **not** a Knowledge Note — it is a small bounded "hot zone" rendered into the system prompt at session start.

- **Format:** entries separated by `\n§\n`. The `remember` tool is the only writer (LLM-driven add / replace / remove, plus an atomic `batch` action — several ops all-or-nothing, budget checked on the final state, duplicate adds skipped idempotently); direct edits via `self_config(write_file)` are rejected.
- **Char budget:** default 2,200 chars (configurable in `[memory.curated]`). Over-budget writes are rejected; the LLM must `replace` or `remove` first.
- **Frozen snapshot:** captured once per `(agent_id, session_key)` and reused for every prompt build in the session. Refreshes only on compression-run completion or session end (Hermes-inspired prefix-cache stability).
- **Threat scanning:** every write goes through `content_scanner` (prompt-injection / exfiltration / SSH access / invisible-unicode patterns).
- **Legacy compatibility:** existing free-format `MEMORY.md` is read as a single legacy entry; `add` is blocked until the LLM curates it via `replace` / `remove`.
- **Third block of the same envelope:** `OPEN_LOOPS.md` (written by `SessionReflector`, injected as `<OpenLoops>`) takes its char budget and its **staleness ceiling** from the same `[memory.curated]` section (`open_loops_char_limit`, `open_loops_max_age_days`, default 14 days). The ceiling exists because the file is rewritten *only* when a reflection runs to completion: naming the capture date tells the model how old the loops are, it does not stop them arriving. An unreadable/absent capture date counts as expired.
- **Acknowledgment contract (D4), both halves:** after a landed write, one short sentence in the user's language naming the destination (never quoting the content). After a settled call that wrote **nothing** — over-budget, `retry_exhausted`, or a duplicate refusal — never acknowledge a save that did not happen. Both halves are asserted over one list of the three ladder writers in `memory_protocol.rs`, so they cannot end up covering different tools.

Module: `src/memory/curated/`. Spec: `docs/superpowers/specs/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md`.

## 1. Overview

Notes are the L1 **persistent** layer of Aleph's memory stack. Three claims define the contract:

1. **Markdown is the source of truth.** Every note is a single `.md` file on disk at `~/.aleph/memory/note/{agent_id}/{category}/{filename}.md`. A human can read, diff, back up, and version-control these files without ever touching the database.
2. **SQLite is a rebuildable index.** The `notes_index`, `notes_links`, `notes_fts`, `notes_vec_map`, and `notes_vec_{384,768,1024,1536,3072}` tables exist solely to make lookup, wikilink graph traversal, full-text search, and semantic search fast. `NoteIndexer::full_rebuild` can reconstruct every row of every index table from the markdown files alone.
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
    ├── goal-lessons/*.md          # per-goal lessons appended by GoalLessonsPromoteStage
    ├── skill/*.md
    ├── reference/*.md
    ├── feedback/*.md              # user-taught corrections distilled by FeedbackDistill
    ├── transcript/*.md
    ├── subagent-run/*.md
    ├── subagent-session/*.md
    ├── subagent-checkpoint/*.md
    ├── subagent-transcript/*.md
    ├── contradiction/*.md         # Phase C2.6: note_drift conflict pages
    ├── other/*.md
    └── query/*.md                 # Spec 8: filed-back query answers
```

`NoteIndexer::ensure_dirs` creates every one of these directories lazily on first use. Filenames are sanitized by `sanitize_title` (see §4) to strip path separators and filesystem-unsafe characters, so a malicious note title like `../../etc/passwd` becomes the literal `etcpasswd` before ever touching disk.

## 3. Frontmatter Schema

### 3.1 Fields

Notes are read and written by `KnowledgeNote::from_markdown` / `KnowledgeNote::to_markdown` (`src/memory/notes/note/mod.rs`). The real `Frontmatter` struct (`src/memory/notes/note/parsing.rs`) declares:

```rust
pub(super) struct Frontmatter {
    pub(super) category: String,            // #[serde(default)]
    pub(super) tags: Vec<String>,           // #[serde(default)]
    pub(super) created: Option<String>,     // RFC3339 or legacy YYYY-MM-DD; #[serde(default)]
    pub(super) updated: Option<String>,     // RFC3339 or legacy YYYY-MM-DD; #[serde(default)]
    pub(super) confidence: f32,             // default 1.0
    pub(super) severity: Severity,          // #[serde(default)]
    pub(super) source_notes: Vec<String>,   // alias "source_facts"; #[serde(default)]
    pub(super) status: NoteStatus,          // #[serde(default)]
    pub(super) supersedes: Vec<String>,     // #[serde(default)]
    pub(super) superseded_by: Vec<String>,  // #[serde(default)]
    pub(super) permanent: bool,             // true → exempt from decay; #[serde(default)]
    pub(super) relations: Vec<Relation>,    // typed relation edges; #[serde(default)]
    pub(super) note_type: Option<String>,   // #[serde(rename = "type")]; Obsidian/llm_wiki page-type
    pub(super) title: Option<String>,       // round-trip only — filename stays SSOT
    pub(super) aliases: Vec<String>,        // Obsidian aliases; #[serde(default)]
}
```

Every field is `#[serde(default)]`, so missing values fall through to sane defaults rather than erroring. Dates are serialized by `to_markdown` as RFC3339 at second precision (e.g. `"2026-07-02T09:30:00Z"`) — day-granular dates collapsed `updated_at` to midnight on every reparse, breaking recency ordering after a rebuild. `parse_date_to_unix` accepts both RFC3339 and the legacy `YYYY-MM-DD` (midnight UTC); empty / missing dates yield `0`. The `type` / `title` / `aliases` trio exists for Obsidian / llm_wiki vault byte-compatibility: `note_type` mirrors the category, `title` is parsed but never mapped into `KnowledgeNote.title` (the filename remains the single source of truth), and `aliases` feeds wikilink alias resolution.

The on-disk shape for a note created via `NoteManageTool` is:

```yaml
---
type: {category}
title: {title}
aliases: []
category: {category}
tags: {tags_json}
created: "{RFC3339}"
updated: "{RFC3339}"
confidence: 1.0000
severity: low
source_notes: []
status: active
supersedes: []
superseded_by: []
---
```

`relations` and `permanent` are emitted only when non-default, so legacy notes without them serialize byte-for-byte as before those fields existed. Forward-compatible: unknown fields are ignored by the parser.

> **Every frontmatter scalar that can carry model output MUST go through `yaml_scalar` (`note/helpers.rs`).** This is a durability rule, not a style preference: an unquoted `title: [wip] plans` makes the whole note permanently unparseable, and *the failure is silent* — `mention_weave` drops the note with `.ok()?`, `note_decay` `continue`s past it, nothing logs above `debug!`, and `load_existing_or_default` hands the next ingest an empty note that overwrites the original's facts. The note simply disappears from the corpus.
>
> `title` / `type` / `category` and the arrays (via `yaml_inline_array`) were always quoted; the `relations:` block was not, until 2026-08-01. Both of its fields are raw model input — the ingest prompt tells the model `"to": "<entity path or [P<n>] token>"`, and `rel_type` is an explicitly free-form LLM-chosen verb (R7, no fixed taxonomy) — so `to: [[plan/x]]`, the wikilink form the model sees everywhere else in the note API, parsed as a nested YAML flow sequence and bricked the note.
>
> The paired rule on the ingest side: any new `IngestPlan` field that can hold a path must be added to `RefTable::resolve_plan`'s field policy. `create.relations` / `append.new_relations` were missing from it, so a prompt-instructed `[P<n>]` token reached `Relation.to` verbatim — the exact leak that table exists to prevent.

## 4. `KnowledgeNote` Data Model

Abridged from `src/memory/notes/note/mod.rs` (the struct grew into a module directory: `mod.rs` + `helpers.rs` + `parsing.rs` + `relation.rs` + `types.rs`):

```rust
/// A knowledge note — the primary memory unit.
///
/// Parsed from (and serializable back to) a markdown file with YAML frontmatter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNote {
    /// Filename without `.md` extension
    pub title: String,
    /// From frontmatter `category` field
    pub category: String,
    /// From frontmatter `tags` field
    pub tags: Vec<String>,
    /// Bullet points from the body — derived index view when `body` is Some
    pub facts: Vec<String>,
    /// Extracted `[[wikilinks]]` from the body — same derived-view rule
    pub links: Vec<String>,
    /// Verbatim markdown body (everything after the closing `---` fence)
    pub body: Option<String>,
    /// Typed relation edges from frontmatter `relations:`
    pub relations: Vec<Relation>,
    /// Unix timestamp (seconds) — from frontmatter `created` date
    pub created_at: i64,
    /// Unix timestamp (seconds) — from frontmatter `updated` date
    pub updated_at: i64,
    /// SHA-256 hex digest of the full file content
    pub content_hash: String,
    /// LLM-assigned distillation confidence; 1.0 for legacy notes
    pub confidence: f32,
    /// LLM-judged importance; Severity::Low for legacy notes
    pub severity: types::Severity,
    /// Source synthesis-note paths or raw-memory IDs that produced this note
    pub source_notes: Vec<String>,
    /// Governance status: Active | Deprecated | Contradicted
    pub status: types::NoteStatus,
    /// Note paths this note supersedes / that supersede this note
    pub supersedes: Vec<String>,
    pub superseded_by: Vec<String>,
    /// Per-fact provenance from inline `<!-- src: ..., origin: ... -->` markers
    pub fact_provenance: Vec<FactProvenance>,
    /// Permanent core-knowledge marker — exempt from decay and archival
    pub permanent: bool,
    /// Obsidian / llm_wiki page-type (mirrors category); None for legacy notes
    pub note_type: Option<String>,
    /// Obsidian aliases from frontmatter `aliases:`
    pub aliases: Vec<String>,
    /// Frontmatter keys this layer does not model, preserved verbatim
    pub extra_frontmatter: BTreeMap<String, serde_yaml::Value>,
}
```

**Body fidelity.** `body` is the verbatim markdown body. `from_markdown` populates it with the raw text after the closing frontmatter fence; `to_markdown` re-emits it byte-for-byte under regenerated frontmatter, so prose, headings, and code blocks survive every round-trip. `body: None` (programmatically constructed notes) falls back to the legacy rendering: facts as `- ` bullets plus a trailing `Related: [[...]]` line — byte-identical to the pre-body-field output. When `body` is `Some`, `facts` and `links` are *derived index views*: a direct `facts.push` would be silently dropped by `to_markdown` (the body wins). Mutations must go through the sync helpers — `set_body` (replaces the body and re-derives facts/links/provenance), `append_facts` (extends the body and the facts view), `add_links` (dedupes and merges targets into the body's link footer).

**Frontmatter fidelity.** The body was only half the contract. `Frontmatter` models a fixed key set and `to_markdown` regenerates the header from it, so until 2026-08-05 every key this layer did not model — `cssclass`, `publish`, `id`, `up`, anything an Obsidian plugin or a human wrote — was parsed away and never re-emitted, destroyed by the first write that passed through here. Unknown keys now ride on `extra_frontmatter` and are re-emitted after the modelled ones. `BTreeMap`, not `HashMap`: emission order must be deterministic or every rewrite would reshuffle the header and churn `content_hash`. An empty map emits nothing, so a note whose header this layer fully models serializes byte-for-byte as before.

Collection is a **second parse of the YAML minus a known-key list**, not `#[serde(flatten)]`: flatten routes the whole struct through serde's buffered `Content` representation, which changes what `deserialize_optional_date_string` sees for a native YAML date — a silent behaviour change on the very parse path this module's regression tests exist to pin. `KNOWN_FRONTMATTER_KEYS` is guarded against drift by a source-level scan of the struct that also asserts how many fields it found, because a scanner matching nothing passes vacuously. `source_facts` is in the list even though no field is named that: it is a serde `alias` for `source_notes`, and omitting it would emit the same data under two keys.

**The link footer is a position, not an event.** `add_links` used to append a *new* `Related:` line on every call and `append_facts` appended bullets after it. The nightly link weaver calls `append_to_note` with links and no facts, so a well-connected note grew one footer line per night with facts interleaved between them. Footer targets now merge into a single trailing line (`split_trailing_related` recognises only a trailing run, so `Related:` appearing mid-prose is untouched), facts are inserted above it, and a consecutive run left by the old behaviour collapses on the next weave. A body whose footer needs no change is not rewritten at all — a no-op link add would otherwise churn `content_hash` and, downstream, mark the note's vector stale.

Parsing splits frontmatter and body at the `---` fences; the closing fence match is **line-anchored** (a whole line equal to `---`), so a `---` embedded inside a value like `title: phase---2` no longer truncates the YAML mid-line. The body contributes `facts` (top-level `- ` bullets, with indented continuation lines attached) and `links` (via `extract_wikilinks`; see §5). `content_hash` is computed over the entire file content and is how the indexer decides whether a re-scanned file needs to be re-indexed.

`sanitize_title` guards every filename before it reaches the filesystem:

```rust
pub fn sanitize_title(title: &str) -> Result<String, AlephError>
```

It strips `/ \ \0 : * ? " < > |`, removes every occurrence of `..`, trims surrounding whitespace, and strips a trailing `.md` (preventing doubled `*.md.md` files). It returns `Err(AlephError::Validation)` when the result is empty / all-dots / all-whitespace, so callers reject the operation instead of writing a note with an unstable filename. This is applied in `NoteIndexer::write_note`, `write_note_raw`, `append_to_note`, `delete_note`, `rename_note`, and every action handler in `NoteManageTool` — LLM-generated titles cannot escape the agent's category directory.

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

`src/memory/notes/links/resolve.rs` implements the resolution strategy chain as pure functions over a prefetched `LinkResolveContext` — built once per store operation from `notes_index` rows, so `resolve()` itself does zero I/O (mirrors the `graph/` pure-over-snapshot pattern, P4). Four tiers are tried in order; **ambiguity at any tier (≥2 candidates) dangles rather than guessing** — a wrong link in a personal vault is worse than no link, deliberately more conservative than fuzzy-matching schemes:

| Tier | Match rule | Confidence | `resolved_by` |
|---|---|---|---|
| 1 | Target contains `/` → exact `category/filename` path hit | 1.0 | `exact_path` |
| 2 | No `/` → unique exact filename match | 0.95 | `exact_filename` |
| 3 | Unique exact alias match (frontmatter `aliases`) | 0.85 | `alias` |
| 4 | Unique normalized filename-or-alias match (case-fold + full-width→half-width fold, `normalize_link_key`) | 0.7 | `normalized` |
| — | Miss, or ≥2 candidates at any tier | 0.0 | `None` (dangling) |

Tier 1 never falls through to the other tiers: a path-form link (`[[category/name]]`) names one specific note, so a miss dangles immediately instead of guessing a filename/alias match elsewhere. Tiers 2–4 test filename and alias candidates in a single merged table per tier (tier 4 merges filename **and** alias into one normalized table — a normalized key hitting both a filename and a different note's alias is itself ambiguous and dangles). `normalize_link_key` is zero-dependency: lowercase + fold `U+FF01..=U+FF5E` / ideographic space `U+3000` to their half-width ASCII equivalents + trim.

`extract_wikilinks_with_alias(text) -> Vec<(String, Option<String>)>` (`src/memory/notes/wikilink.rs`) extracts `(target, alias)` pairs from `[[target|alias]]` syntax alongside the plain-target `extract_wikilinks`. The alias survives end-to-end: `index_note` builds a `to_raw → label` map from it and persists the label into `notes_links.label` (§8), which the panel renders as the edge/excerpt-anchor display text (Obsidian JSON Canvas `edge.label` convention) instead of the raw target string.

### 5.3 Persistence and lifecycle

Every wikilink target and typed relation is upserted into `notes_links` (§8) as one row keyed by `(agent_id, from_note, to_note)`, carrying the full resolution outcome rather than just the raw pair:

- `to_raw` — the raw target text as written in the source (`[[to_raw]]` or `[[to_raw|label]]`); also the join key for targeted backfill (below).
- `to_note` — the resolved `category/filename` path on a unique hit; falls back to `to_raw` itself while dangling (so the row stays uniquely keyed per raw target).
- `resolved_by` — which tier resolved the target (§5.2); the same resolver chain runs for both plain wikilinks and typed relations, so this is populated for either — `NULL` only when the target dangles.
- `confidence` — the tier's confidence for a plain wikilink, or the LLM/tool-declared confidence for a typed relation (clamped to `[0,1]`; a typed relation's declared confidence overrides the tier's, since it reflects the caller's judgement, not just resolvability).
- `status` — lifecycle state, `NOT NULL DEFAULT 'active'`, domain `active` | `dangling` | `tombstone`. An unrecognized value from a foreign writer parses back to `active` (`LinkStatus::parse`, P7 fail-toward-visibility) rather than making the row invisible.
- `label` — the display alias from `[[target|label]]`; `NULL` for plain wikilinks and typed relations.

**Tombstone semantics (delete).** `remove_note_index` deletes the deleted note's own **outgoing** rows outright (`from_note = this path` — the note is gone, its edges have no meaning), but only **marks** its **inbound** rows `status = 'tombstone'` (`to_note = this path`) rather than deleting them: the linking note's body keeps its `[[link]]` text completely untouched, so the row is revivable — recreating a same-named note flips it back to `active` via targeted backfill instead of requiring the linking note to be re-edited.

**Targeted backfill.** `NoteStore::backfill_inbound_links(agent_id, keys)` (`src/memory/store/sqlite/notes/store_impl.rs`) is the create/rename-time counterpart to the corpus-wide `relink_unresolved` sweep (§6.3): it scans only `dangling`/`tombstone` rows whose `to_raw` **literally equals** one of the just-written note's identity keys — title, `category/title`, and frontmatter aliases on create; title and `category/title` on rename (a rename's aliases are unchanged, and its body-side `[[wikilink]]`s are already re-pointed by the rename cascade). `NoteIndexer::finalize_write` (every full write) and `NoteIndexer::rename_note` call it as the last step of their write pipeline, so a note that resolves another note's previously-dangling/tombstoned link becomes visible **within the same write** rather than waiting for the next dream-cycle sweep. Because the match is literal-string on `to_raw` (not a re-run of the tier-4 normalizer), a dangling link differing from the new note's keys only by case/full-width folding is **not** picked up by this targeted pass — it self-heals only on the next `relink_unresolved` sweep or `full_rebuild` (§6.3), both of which re-run the full resolver chain over every dangling row.

**Conflict defenses.** `relink_unresolved` and `backfill_inbound_links` both update candidate rows one at a time outside a single transaction, so a revival that would collide with `UNIQUE(agent_id, from_note, to_note)` — e.g. two dangling variants of the same source note normalizing to the same target — must not abort the whole pass. Each revival runs `UPDATE OR IGNORE notes_links SET to_note = ... WHERE id = ?`; the losing row (update ignored because another row already occupies that key) is then removed by a follow-up `DELETE FROM notes_links WHERE id = ? AND status IN ('dangling','tombstone')`, so no redundant duplicate lingers and every other row in the batch still gets processed.

## 6. `NoteIndexer` and the Write Pipeline

`NoteIndexer<S: NoteStore>` (`src/memory/notes/indexer.rs`) is generic over the store trait and owns both the filesystem root (`memory_dir: PathBuf`) and an `Arc<S>` handle. It is the only module that writes markdown files.

### 6.1 Write Flow

`index_file(agent_id, category, path)` is the per-file **reconcile** path — "this file's bytes changed, make the index agree":

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
                                                                 |
                                                                 v
                                                     +-------------------------+
                                                     | finalize_side_effects   |
                                                     |  • backfill_inbound_    |
                                                     |    links (revive edges  |
                                                     |    waiting on this note)|
                                                     |  • refresh_embedding    |
                                                     |    (vector ← new bytes) |
                                                     +-------------------------+
```

**`index_file` runs the same post-write legs a first-class write does**, and only on the changed path — the skip branch returns before them, so a self-write that already indexed itself costs one hash comparison. It used to be the `index_note` leg alone while its call sites' comments claimed it reconciled "notes_index/FTS/embedding/tags"; the five callers that rewrite a note's bytes without going through `write_note*` (`note_lint` frontmatter fix and link repair, `note_drift` supersede/stale banners, and the vault watcher of §6.4) therefore left each note's **vector describing the pre-edit text**, drift that surfaced only as a `stale_vectors` count at the next boot. `index_one_file` — the free function `reconcile_corpus` calls once per file — deliberately keeps the opposite trade: no embed (a whole corpus's re-embed is a cost the operator asks for via `reembed_all`) and one global `relink_unresolved` instead of a per-file backfill.

Write entry points on `NoteIndexer`:

- `write_note(agent_id, category, &note)` — serializes `KnowledgeNote::to_markdown` (plus the supersession section via `ensure_supersession_section`), writes `{category}/{title}.md` atomically, reparses the written file, indexes it, and notifies orientation. Used by `CompressionService` for fresh notes and by `NoteManageTool` create/update.
- `write_note_raw(agent_id, category, title, content)` — writes caller-supplied full markdown **byte-for-byte** (no `KnowledgeNote` reconstruction), then reparses and indexes. Backs the panel node editor's `graph.update_note` RPC, so hand-edited content survives untouched.
- `append_to_note(agent_id, "category/filename", &facts, &links)` — reads the existing file (or synthesizes an empty `KnowledgeNote`), then extends it through the body-preserving helpers `append_facts` / `add_links` (§4) so a verbatim prose body is extended rather than silently dropped; bumps `updated_at`, writes atomically, and re-indexes.
- `delete_note(agent_id, category, filename)` — removes the index rows (including any embedding) first, then the markdown file, then notifies orientation. Idempotent: a file already missing on disk is not an error; if the file delete fails after index removal, `full_rebuild` re-indexes the surviving file (self-healing in the safe direction).
- `rename_note(agent_id, old_title, new_title)` — renames the file, scans every category dir for `[[old_title]]` references, calls `rewrite_wikilinks` on each match, and re-indexes every changed file.

#### Abandoned apply-staging residue (`.tx/`)

`CompoundApplyTx` stages every write under
`memory/note/{agent}/.tx/{tx_id}/` and cleans that tree in three places: a
successful commit, a rollback, and the `Drop` impl that covers cancellation.
All three run **inside the process that staged it**. Kill the server between
staging and commit — `kill -9`, OOM, power loss — and the tree survives every
one of them, permanently: for its whole life nothing else in the repo so much as
named `.tx`. The residue is a full copy of every note the batch was about to
write, sitting inside the vault the product tells the user to open in Obsidian,
and it accumulated one tree per unlucky death forever.

`ingest::sweep_tx_residue(memory_dir, agent_id, older_than_secs)` deletes
staging trees older than the ceiling and returns how many went.
`full_rebuild_all` calls it once per corpus and reports the total on
`RebuildAllStats::tx_residue_removed`, which boot logs beside `stale_vectors`:
a non-zero count means a previous process died mid-ingest, and a count climbing
across boots means something is killing the server during consolidation.

Three decisions:

- **Boot is the right time and a sufficient one.** Residue exists exactly
  because a process died holding it, and this pass is the first thing that
  happens afterwards.
- **Age threshold, not "delete everything under `.tx`".** A live transaction
  owns its tree while it works. `memory.compound_ingest.tx_residue_gc_seconds`
  (default 3,600) is the width of the window an apply may take; an apply takes
  milliseconds, so that is three orders of magnitude of headroom. The knob was
  written with this sweep in the original Spec-6 plan and then shipped for its
  whole life with **zero consumers** — a user-visible setting that did nothing.
- **A tree whose mtime cannot be read is left alone.** "I could not look" is not
  evidence of "it is abandoned", and the other branch deletes — the same rule
  the vault watcher applies to its own stat failures (§6.4).

The sweep is deliberately scoped to `.tx` only. Orphaned `.aleph_atomic_*.tmp`
staging files from a killed atomic write are the same class and are **not**
swept (they are filtered out of listings by `vault_io` and by the watcher's
extension check, so they are inert rather than harmful) — stretching a knob
named `tx_residue` to cover them would be a wider promise than its name makes.

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

`CompressionScheduler` in `src/memory/compression/scheduler.rs` is now just a turn counter: it tracks `pending_turns: AtomicU32`, and `should_trigger_compression()` returns `CompressionTrigger::TurnThreshold(n)` when `pending_turns >= turn_threshold` (default 20, `[policies.memory.compression]`) or `None` otherwise. The earlier `IdleTimeout` / `SessionEnd` / `ManualRequest` / `BackgroundSchedule` variants and the idle timer were removed — none had a live production path (the manual / session-end / background flows call `CompressionService::compress()` directly, bypassing the scheduler). The live triggers are: turn threshold, the hourly background tick (`background_interval_seconds = 3600`), the session-end flush, the correction flush (`flag_user_correction`), and the `memory.compress` RPC. When a run fires, `CompressionService` (`src/memory/compression/service.rs`) drains a batch from `raw_memories`, splits it per source, and routes each group through `CompoundIngestor::ingest_batch`, which plans and dispatches `Create` / `Append` / `Update` note writes via `NoteIndexer` (see `RAW_MEMORY.md` §7.1).

### 6.3 Cold-Start reconcile — one corpus, and all of them

`reconcile_corpus(agent_id) -> IndexStats` scans `memory_dir/{agent_id}/{category}/*.md` for every category in `CATEGORY_DIRS`, parses each file through `KnowledgeNote::from_markdown`, and calls `NoteStore::index_note`. Files whose SHA-256 matches the existing `notes_index.content_hash` are skipped, yielding a cheap no-op on warm databases. After the scan it drops any index row for the agent whose backing `.md` file no longer exists on disk (orphans from a rename / deletion / agent-id relocation), `prune_orphan_vectors` sweeps embedding rows whose path has no `notes_index` row (best-effort — a sweep failure never fails the reconcile), `stale_vector_paths` **reports** (never repairs) how many notes carry a vector computed from an older body, and `relink_unresolved` retries raw wikilink edges now that every note is indexed. The returned `IndexStats { indexed, skipped, errors, pruned, stale_vectors }` makes the operation observable; parse failures are logged and counted rather than aborting the whole pass. This is the repair path if the SQLite index is deleted or goes out of sync with the markdown files.

`full_rebuild(agent_id)` = `ensure_dirs` (materialise the 21 category directories) **+** `reconcile_corpus`. The split is load-bearing: scaffolding is a *write* and reconciling is a *repair*, and only the first is right to apply to an agent the operator is actually running.

`full_rebuild_all(always_include) -> RebuildAllStats` is what boot calls. It reconciles **every corpus on disk** — `project_scope::list_note_corpora`, the single enumeration of "which `note/{agent_id}/` partitions exist" — scaffolding only `always_include` (the default agent, so a fresh install still gets its vault laid out). A base agent id and each composed scoped id (`{base}__proj-…` project namespace, `{base}__u-…` personal scope, `{base}__p-…` project scope) are corpora in exactly the same sense; they differ only in how the id was composed, never in how the notes underneath are stored, indexed or maintained.

> **Why this is not cosmetic.** Boot used to call `full_rebuild(default_agent_id)` and stop. For every other corpus nothing ever reconciled the index with disk: a note renamed or deleted while the server was down kept a recallable index row *forever*, a `[[wikilink]]` that dangled only because of category scan order was never retried, and vector drift was not even counted. Corpora multiply with the `[memory] project_scoped` toggle and with session scoping, and the dream daemon already fans its nightly maintenance over exactly this enumeration — the boot pass was the one maintenance job that did not.

Corpora are reconciled sequentially: each reconcile already fans out over categories up to `available_parallelism()`, so an outer fan-out would only multiply peak contention on the single SQLite connection. A corpus whose reconcile returns `Err` is recorded in `RebuildAllStats::failed` (name + error) and the pass continues — kept separate from `total.errors`, which counts *files* that would not parse (normal on a hand-edited vault). A whole namespace left unmaintained is a different fact from a bad file, and folding them together is how it would stay invisible.

### 6.4 Vault watcher — markdown edited outside Aleph

`src/memory/notes/watcher.rs`. `spawn_note_vault_watcher(indexer)` puts one debounced recursive watch (`notify` + `notify-debouncer-full`, 750 ms settle) over the whole note root and returns a `NoteVaultWatcher`; **dropping it stops the watch**, so the server binds it for the process lifetime.

The markdown is the source of truth and every agent directory gets an `.obsidian/` vault config written into it (§9) — i.e. the product tells the user to open their memory in Obsidian. Until this existed nothing honoured that: an edit made outside Aleph changed the truth and search, links and the graph went on serving the old body until the next restart (and, before §6.3, forever for any corpus but the default agent's). A deletion was worse — the index row survived, so recall kept surfacing a note whose file was gone.

Three decisions worth not reversing:

- **The filesystem, not the event kind, decides the action.** For each settled `.md` path: `metadata()` succeeds → `index_file` (which no-ops on an unchanged hash, so Aleph's own writes are free); `NotFound` → `remove_note_index` with the usual inbound-link tombstone semantics; **any other error → skip**. "I could not look" is not evidence of "it does not exist", and the other branch deletes. Event kinds coalesce and differ per platform; current state does not.
- **The watched root is canonicalized first.** Notifications report canonical paths (macOS resolves `/var` → `/private/var`; any home or volume symlink resolves the same way), so a watcher holding the un-resolved root would `strip_prefix` every incoming path to `None` and classify the entire vault as "not a note" — running, and doing nothing, with no symptom at all. This was caught by the end-to-end test, not by the path-grammar unit tests, which is the reason that test exists.
- **A bulk batch becomes a corpus reconcile.** Above `MAX_PATHS_PER_BATCH` (128) changed files in one window — a vault sync, a `git checkout`, an import — the affected corpora go through `reconcile_corpus` instead of 128 individual round-trips. It hash-skips too, so at that size it is the cheap path. (`reconcile_corpus`, not `full_rebuild`: a watcher reacts to what the user did to their files; provisioning directories is not part of that.)

`classify` accepts exactly `{root}/{agent_id}/{category}/{title}.md` with `category ∈ CATEGORY_DIRS`. Everything else in the tree is deliberately out: agent-root scaffold (`index.md` / `SCHEMA.md` / `LOG.md` / `USER.md`, regenerated every dream cycle), `archive/` (where `NoteDecay` parks cold notes — re-indexing from there would resurrect every archived note the moment its file was touched), dot-directories, and atomic-write staging files.

There is **no config flag to disable it.** It does not choose a behaviour on the user's behalf; it makes the index tell the truth about files the user already owns. If the watch cannot be established the caller logs it and the process behaves exactly as it did before the module existed.

> Comparison: EverOS runs the same idea as a `watchdog` observer feeding a durable `md_change_state` queue in SQLite, so a crash mid-sync replays on restart. Aleph's queue is in-process and a crash simply loses the pending batch — recovered by the boot reconcile of §6.3, which EverOS's design does not have an equivalent of. The trade is deliberate: no new table, no new migration, and the crash window is covered by a pass that has to exist anyway.

## 7. `NoteStore` Trait

`src/memory/notes/store.rs` defines the persistence contract. Every method is scoped by `agent_id`:

| Method | Purpose |
|---|---|
| `index_note(&self, note: &KnowledgeNote, agent_id: &str, category: &str) -> Result<()>` | Upsert `notes_index` row, replace `notes_links` rows, and rebuild `notes_fts` content. |
| `remove_note_index(&self, path: &str, agent_id: &str) -> Result<()>` | Remove index / links / FTS entries by `category/filename` path — and the note's embedding rows (`notes_vec_map` + `notes_vec_{dim}`) in the same transaction, so a deleted note's orphan vector stops occupying KNN slots. |
| `get_note_index(&self, path: &str, agent_id: &str) -> Result<Option<NoteIndexEntry>>` | Single-row lookup by path. |
| `list_notes(&self, agent_id: &str) -> Result<Vec<NoteIndexEntry>>` | All notes for an agent, most-recently-updated first. |
| `get_outgoing_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>>` | Raw wikilink targets emitted by this note. |
| `get_incoming_links(&self, path: &str, agent_id: &str) -> Result<Vec<String>>` | Paths of notes that link to this filename. |
| `search_notes_fts(&self, query, agent_id, limit) -> Result<Vec<NoteIndexEntry>>` | FTS5 full-text search. |
| `get_graph_data(&self, agent_id, limit) -> Result<(Vec<NoteIndexEntry>, Vec<(String,String)>)>` | Top nodes + edges for graph visualization. |
| `get_neighbors(&self, center, agent_id, depth, limit) -> Result<(Vec<NoteIndexEntry>, Vec<(String,String)>)>` | BFS neighborhood around a node. |
| `count_all_notes(&self) -> Result<i64>` | Cross-agent note count for diagnostics. |
| `find_by_filename(&self, filename, agent_id) -> Result<Vec<String>>` | Used by wikilink resolution (§5.2) to find exact filename matches. |
| `upsert_embedding(&self, path, agent_id, embedding, dim, content_hash) -> Result<()>` | Write or replace a note's embedding, recording which version of the note it was computed from (§ embed freshness). `""` means provenance unknown and reads as stale. |
| `stale_vector_paths(&self, agent_id) -> Result<Vec<String>>` | Notes whose vector is missing or was computed from an older version. |
| `vector_search(&self, embedding, dim, agent_id, limit) -> Result<Vec<(String, f32)>>` | Vector similarity returning paths + scores. |
| `hybrid_search_notes(&self, embedding, query_text, agent_id, dim_hint, limit) -> Result<HybridSearchOutcome>` | Vector + FTS fusion via RRF. Returns full content plus each leg's candidate count, so a caller can say what actually ran rather than what it configured. |
| `vector_search_notes_with_content(&self, embedding, agent_id, dim_hint, limit) -> Result<Vec<NoteSearchResult>>` | Vector-only search returning full content. |
| `get_notes_by_category(&self, agent_id, category, limit) -> Result<Vec<NoteIndexEntry>>` | Paginated category listing. |
| `get_embedding(&self, path, agent_id, dim_hint) -> Result<Option<Vec<f32>>>` | Read back a stored embedding. |
| `prune_orphan_vectors(&self, agent_id) -> Result<usize>` | Sweep embedding rows whose path has no `notes_index` row (historical deletes); returns the count removed. Called by `full_rebuild` (§6.3). |
| `list_review_archive(&self, agent_id, limit) -> Result<Vec<ReviewArchiveRow>>` | Most recent governance verdicts, newest first — the consumer half of `archive_review` (§15). |
| `prune_review_archive(&self, agent_id, older_than_secs) -> Result<usize>` | Age out decided verdicts; returns the count removed. Called by `NoteReviewStage` (§15). |

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

DDL is defined in `src/memory/store/sqlite/schema/ddl.rs`. All statements use `CREATE ... IF NOT EXISTS` and `init_schema` is idempotent.

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
    confidence  REAL NOT NULL DEFAULT 1.0,
    resolved_by TEXT,
    status      TEXT NOT NULL DEFAULT 'active',
    label       TEXT,
    UNIQUE(agent_id, from_note, to_note)
);
CREATE INDEX IF NOT EXISTS idx_notes_links_from ON notes_links(agent_id, from_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to ON notes_links(agent_id, to_note);
CREATE INDEX IF NOT EXISTS idx_notes_links_to_raw ON notes_links(agent_id, to_raw);
```

`to_raw` stores the raw wikilink text as written in the source note (before resolution), and `relation` carries an optional typed relation label (from the `Relation` frontmatter field, or the fixed string `"mention"` for auto-detected soft edges — §14). `confidence` is the resolver tier's confidence or the relation's declared confidence. `resolved_by` / `status` / `label` are the lifecycle columns described in §5.3: which resolver tier fired (or the fixed string `"mention_scan"` for `MentionWeaveStage`-authored soft edges — §14, not itself a resolver tier), the row's `active`/`dangling`/`tombstone` state, and the `[[target|label]]` display alias respectively. `idx_notes_links_to_raw` backs the targeted-backfill lookup (§5.3) that scans dangling/tombstone rows by `to_raw`.

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

`content` is the note's full frontmatter-stripped body when the note has one (`KnowledgeNote::body_text_for_fts`, provenance markers stripped) — bullet-facts-only indexing made all prose in raw-written notes (panel edits, hand edits) invisible to the FTS leg of search. `search_notes_fts` splits a multi-word query into an OR of quoted FTS5 phrases (one per whitespace-separated term, embedded quotes doubled) and orders by `rank` (bm25) — binding the whole query as one exact phrase required the exact token sequence, so multi-word natural-language queries matched nothing.

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
    Create,    // fails if filename already exists
    Update,    // replace body of existing note (markdown preserved verbatim)
    Append,    // extend facts + links (or relations-only) on existing or new note
    Query,     // hybrid (vector + FTS) search across indexed notes
    List,      // list notes, optionally filtered by category
    Get,       // read ONE note by address — full body, untruncated by ranking
    Delete,    // remove file + index entry
    Rename,    // rename filename/title, rewrite every inbound [[wikilink]], backfill
    Insights,  // read materialized graph-health insights (read-only, §14)
    Evolution, // read the memory-evolution gate state from recent dream cycles (read-only)
}
```

Args:

```rust
pub struct NoteManageArgs {
    pub action: NoteManageAction,
    pub category: Option<String>,   // required for create/update/append/delete
    pub filename: Option<String>,   // required for create/update/append/delete; current name for rename
    pub title: Option<String>,      // required for create
    pub content: Option<String>,    // required for create/update body
    pub facts: Option<Vec<String>>, // for append
    pub links: Option<Vec<String>>, // wikilink targets
    pub tags: Option<Vec<String>>,
    pub query: Option<String>,      // required for query
    pub limit: Option<usize>,       // max results for query/list (default: 20)
    pub new_title: Option<String>,  // required for rename; category is auto-located
    pub relations: Option<Vec<NoteRelationArg>>, // typed edges on create/update/append
    pub agent_id: Option<String>,   // per-agent vault scoping (default: "default")
}

pub struct NoteRelationArg {
    pub to: String,        // target note path or wikilink-style text
    pub rel_type: String,  // #[serde(rename = "type")] — free-form verb, no fixed taxonomy (R7)
}
```

Mutating actions run `validate_category(category)` directly against `CATEGORY_DIRS` imported from the indexer — single-sourced, so the tool can never drift from the directories the indexer creates (a hand-copied list here once drifted from `CATEGORY_DIRS`, locking the LLM out of `feedback` / `goal-lessons` / `query` notes; a regression test now pins the alignment). Unknown categories are rejected with a listing of valid values. Create and update both run input through `sanitize_title` before the filename is ever joined into a path.

`query` is hybrid search: when an embedder is injected (`with_embedder`, wired from the registry's `config.embedder`), the query text is embedded and `hybrid_search_notes` fuses vector + FTS results via RRF — which also gives CJK queries semantic recall that the unicode61 FTS tokenizer cannot. A missing embedder or a failed embed degrades to the FTS path rather than failing (P7). Every query records `recall_signals` on the dedicated channel `"note_manage"` (independent per-day dedup from the auto-recall channel). Output is budgeted: 4,000 chars per note, 24,000 chars total, with honest `…(+N chars truncated)` markers rather than silent cuts.

`get` reads one note **by address** rather than by rank. Until it existed the
only body-returning read on this tool was `query`, and `query` caps every hit at
4,000 chars — while `update` replaces a note's body *wholesale*. The two
together meant a model asked to edit a long note could only ever see its first
4,000 chars and would write back what it saw, silently dropping the rest; the
`…(+N chars truncated)` marker sat inside the very text it was copying forward.
The Panel never had this problem: `graph.node_detail` reads the same file from
disk, whole. This action is that capability on the model's face, and the tool
`DESCRIPTION` now tells the model to `get` before it `update`s.

Three rules `get` does not share with `query`:

- **The address must be unambiguous.** `category` is optional; omitted, the
  filename is resolved through `find_by_filename`, and a name held by two
  categories is a refusal that *names both candidates* rather than a first-hit
  guess. Handing the wrong note to a caller that is about to replace its body is
  worse than making the caller say which one — the same never-guess rule the
  wikilink resolver applies to an ambiguous tier (§5.2).
- **Disk is the answer.** The index row supplies tags and category; the body
  comes from the markdown file, which is the source of truth. A missing file is
  reported as missing rather than served as stale metadata.
- **It is a read, so it carries no `destination`.** That field is a *write*
  receipt (§D4 acknowledgment contract); stamping one on a read is how a model
  ends up telling the user something was filed when nothing was written.

Content is capped at `GET_MAX_CHARS` (64,000 — 16× the per-hit `query` cap),
and when the cap bites the *message* says so as well as the body, because a
wholesale `update` written from a truncated read is exactly the loss this action
exists to stop. A note above that ceiling has no paging path through this tool
(stated gap).

`create` and `update` store `content` verbatim as the note's `body` (via `set_body`, §4) and route through `NoteIndexer::write_note` — atomic write, orientation notification, supersession section. Both, plus `append`, refresh the note's embedding immediately after a successful write (embed-on-write, best-effort — never fails the write), so the note is discoverable by vector search without waiting for a manual `memory.reembed`. `create` additionally surfaces `related_notes` — semantic neighbors via the embedder's vector search, falling back to per-keyword FTS when no embedder is wired — so the model can weave the new note into the wiki via `links` instead of leaving an orphan island. `delete` routes through `NoteIndexer::delete_note` (§6.1): index rows including embedding, file, and orientation in one owned path.

`rename` takes `filename` (current name) + `new_title` (target name) — the note's category is located automatically via `find_by_filename`, so callers never pass `category` for this action (a duplicate filename across categories renames the first hit; disambiguate by delete/recreate instead). It delegates to `NoteIndexer::rename_note` (§6.1): renames the file, rewrites every inbound `[[old_title]]` wikilink across the corpus, re-indexes, re-embeds under the new path, and runs the targeted `backfill_inbound_links` pass (§5.3) so any dangling/tombstoned link that names the new title resolves within the same call.

`relations` (`Vec<{to, type}>`) declares typed semantic edges and is accepted on `create`, `update`, **and** `append`. Each pair is merged into the note's frontmatter `relations:` list — deduped by `(to, rel_type)` — with confidence fixed at 1.0 (a tool-authored relation is an explicit statement, not a resolver guess); `supersedes` / `superseded_by` / `contradicts` are the structural-strong verbs force-surfaced at retrieval regardless of score. `append`'s previously-strict emptiness guard now allows a **relations-only** append: declaring a relation on an existing note no longer requires also passing `facts` or `links`.

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
3. On a present projection, write a `KnowledgeNote` via `NoteIndexer::write_note`. `write_note` already reparses the written file and indexes it with the correct `content_hash` — the redundant second `index_note` call was removed, because indexing the empty-hash struct overwrote the row and defeated the hash-skip on every subsequent rebuild. The projected note preserves the event-log `created_at` (resetting it to projection time lost the original creation date on every fold).
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

## 14. Graph Subsystem

`src/memory/notes/graph/` is the note knowledge-graph intelligence layer — pure functions over an immutable `GraphSnapshot`, zero storage coupling (P4), no external graph crate (R3), concurrency via std threads:

| File | Contents |
|---|---|
| `mod.rs` | `GraphSnapshot` / `GraphNode` / `GraphEdge` / `GraphIndex` (shared adjacency), community detection entry |
| `relevance.rs` | Four-signal relatedness scoring: direct-link ×3 (edge-confidence-weighted), source-overlap ×4 (IDF-damped), Adamic-Adar ×1.5, type-affinity ×1 |
| `insights.rs` | Graph-health insights: isolated nodes (degree ≤ 1), sparse communities, bridge notes, surprising cross-community connections |
| `minhash.rs` | MinHash + LSH content-similarity edges over note bodies (word-level 3-shingles, K=64, zero embeddings, zero new deps) |

The graph is **materialized offline** by `GraphRecomputeStage` (`src/memory/dreaming/stages/graph_recompute.rs`) each dream cycle: it loads the snapshot, runs the five-signal / Louvain / insights algorithms inside `spawn_blocking` plus a MinHash similarity pass, and upserts `notes_graph_cache` + `notes_graph_related` + `notes_graph_insights` — pure deterministic aggregation, zero LLM calls (R7/R10-safe). Consumers: the `note_manage` **Insights** action (§11) reads the materialized insights, and the `note_weave` dream stage uses the `isolated` insight plus `related_peers` five-signal scores for orphan-note backfill. See [FEATURE_LOCATOR.md](../FEATURE_LOCATOR.md) §2.5① for the full anchor map and the NoteWeave three-signal orphan-rescue design.

**MentionWeaveStage** (`src/memory/dreaming/stages/mention_weave.rs`) is a separate, corpus-scanning consumer — not a reader of the materialized cache above. It sits in the Consolidate pipeline between `NoteWeaveStage` (real links win first) and `NoteDecayStage` (so mention edges count toward `link_weight` the same cycle it runs), and scans every note body for **unlinked mentions** of another note's filename/alias via `src/memory/notes/links/mentions.rs::scan_mentions`: deterministic exact matching (ASCII names require word boundaries on both sides; CJK names match as substrings since CJK text has no word boundaries; a name must be ≥4 ASCII chars or ≥2 CJK chars to qualify, and a name owned by more than one note is dropped wholesale — the same never-guess rule as §5.2), zero LLM. Each cycle **fully replaces** the `relation = 'mention'` edge set (`NoteStore::replace_mention_links`, one transaction) — capped at `MAX_MENTIONS_PER_NOTE = 5` per source note and `MAX_MENTIONS_PER_CYCLE = 200` overall (deterministic truncation over the `(from, to)`-sorted scan output) — inserting rows at `confidence = 0.35` with `resolved_by = 'mention_scan'` and `ON CONFLICT(agent_id, from_note, to_note) DO NOTHING`, so an existing real wikilink or typed relation for the same pair always wins over the soft mention edge.

**Canvas / gateway enrichment.** `graph.query` (`src/gateway/handlers/graph.rs::handle_query_impl`) layers three graph-health signals onto the base node/edge feed before returning: top-3-per-node MinHash similarity edges (`related_edges_between`, surfaced as edge kind `related_similarity`, deduped against real links by undirected pair), `bridge_nodes` (the materialized `bridge` insight, filtered to nodes visible in this response), and `surprising_edges` (the materialized `surprising` insight, both endpoints visible). The panel's galaxy renderer (`interfaces/webchat/src/platform/wide/views/canvas/galaxy_build.rs` + `gl/edges.rs`) maps each edge's `relation`/`kind` string to a render code and tint via `edge_kind_code`/`edge_kind_color`; `mention` and `related_similarity` render at a fixed dim brightness (not confidence-scaled, since they are soft/derived edges rather than authored links), and any edge present in `surprising_edges` overrides its base kind with a bloom-emphasized code regardless of its real relation.

## 15. Governance verdict archive

The admission gate (`memory.compound_ingest.governance_enabled`, default
`false`) defers a risky `Create` into `notes_review_queue`; `NoteReviewStage`
adjudicates it next dream cycle and `archive_review` moves the decided row into
`notes_review_archive` with a final status of `approved` | `rejected` |
`rewritten` | `timeout` | `max_retries_exceeded`.

That table was **write-only for its whole life** — one `INSERT`, and not one
`SELECT` anywhere in the repo, tests included. Two things followed from that:

1. **The `rejected` verdict was unreachable.** A rejected candidate's facts are
   never written to any note, so the archive row *is* the only surviving record
   of what was proposed and why it was turned down. The ingest path's own
   comment says a deferred op is fine because "the knowledge is preserved" —
   which is a claim about a table somebody has to be able to read.
2. **It grew for the life of the install.** One row per gated candidate, each
   carrying a full note-candidate payload, with no retention.

Both halves are now closed, and each is owned by the side that caused it:

- `note_manage(action='insights')` appends a `governance_verdicts` section — the
  10 most recent verdicts, each candidate payload capped at 400 chars. It is
  best-effort: an archive read failure must not stop the graph-health half of
  that action from answering.
- `NoteReviewStage` — the stage that *writes* the rows — prunes verdicts older
  than 90 days at the end of each pass. Retention living with the producer is
  what keeps the ceiling from becoming a second fact somebody has to remember to
  maintain elsewhere.

## See Also

- [Raw Memory (L0)](RAW_MEMORY.md) §7.1 — `CompressionService` reads unprocessed `raw_memories` and writes notes through the §6 pipeline.
- [Dream Daemon](DREAM_DAEMON.md) — the dream pipeline's subject is the note corpus: drift detection, decay, and lint stages all operate on the markdown + index layer described here.
- [Retrieval](RETRIEVAL.md) §1 — how notes are queried (FTS, vector, hybrid, graph) by the retrieval pipeline.
