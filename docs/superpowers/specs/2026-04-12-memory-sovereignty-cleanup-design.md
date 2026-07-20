# Memory Sovereignty Cleanup — Design

**Date:** 2026-04-12
**Branch:** main (single-branch development)
**Scope:** Rust Core (`alephcore`), memory subsystem only

## 1. Purpose

The memory refactor from `raw + facts (SQLite-only)` to `raw + note
(SQLite + Markdown)` left three legacy mechanisms in a degraded state:

- **Short-term memory** (`MemoryTier::ShortTerm`) — a SQLite-era tier concept
  whose responsibilities are already covered by `raw_memories`,
  `SessionCompactor`, and `recall_context`.
- **Strength/confidence-based decay** (`MemoryFact.strength`,
  `MemoryFact.confidence`, `ApplyDecayCommand`, `importance_weight` pipeline
  stage, residual `ValueEstimator`) — deterministic heuristics that
  attempted to encode "what is important" and "what should be forgotten"
  as persistent numbers.
- **Query-time age ranking + dormant-note archival**
  (`time_decay`, `recency_boost`, Dream Daemon `NoteDecay`,
  `recall_signals` telemetry) — lightweight, stateless ranking hints plus
  janitorial archival.

This spec takes a **pure LLM-sovereignty stance** (R8, R10) on each:
the first two are **deleted**, the third is **preserved**. The core-memory
injection responsibility (what goes in the system prompt vs. what gets
retrieved by query) is restructured around **category-based routing** with
a bounded budget that overflows to the query-retrieval pool.

The result is a memory subsystem where:

- Markdown is the single source of truth for persistent knowledge.
- Deterministic code never judges "importance" or "staleness" — those are
  LLM reading-time decisions.
- The only allowed status bits on a note are LLM-written observations
  (`stale: true` from `NoteDrift`) and filesystem-level archival
  (moving to `archive/`).
- Retrieval gives the LLM a ranked candidate set; the LLM decides what
  matters.

## 2. Related Work

This spec is **architectural**, not a dead-code sweep. It operates at a
different layer than and is complementary to:

- `2026-04-12-memory-legacy-cleanup-design.md` — deletes orphan modules
  (`decay.rs`, `GraphDecayPolicy`, `src/wiki/`, unused `DreamingConfig`
  keys, legacy `dream_reports` columns). Execute that spec first or in
  parallel; they do not conflict.
- `2026-04-11-memory-notes-migration-design.md` and
  `2026-04-12-retrieval-layer-notes-migration-design.md` — the
  raw+facts → raw+note migration that created the degraded state this
  spec addresses.
- `2026-04-10-event-sourcing-wiring-design.md` — event-sourcing for note
  mutations; interacts with this spec via `ApplyDecayCommand` removal
  (see §10.4).

## 3. Non-Goals

- **No retrieval quality experiments.** `hard_min_score`,
  `mmr_similarity_threshold`, `rerank_blend`, `time_decay_half_life_days`
  etc. keep their current defaults.
- **No Dream Daemon stage additions or removals.** `NoteConsolidate`,
  `NoteDrift`, `NoteSynthesis`, `NoteLint`, `NoteDecay`, `DailyDigest`
  are untouched beyond what `recall_signals` wiring requires.
- **No new frontmatter fields.** No `pinned`, no `importance`, no
  `priority`. The only status bit allowed on disk remains
  `stale: true` written by `NoteDrift`.
- **No category-list changes.** The set of valid note categories
  (`CATEGORY_DIRS` in `src/memory/notes/indexer.rs` + the four
  `subagent-*` categories) is preserved as-is.
- **No `src/memory/cortex/` changes.** That subsystem is deprecated in
  favor of `src/poe/` and will be migrated in its own spec.
- **No reranker / query-expander wiring.** Both remain "implemented but
  not wired" per `RETRIEVAL.md` §5–6.
- **No `raw_memories` GC / TTL work.** The "retained-but-inert" phase
  described in `RAW_MEMORY.md` §8 remains manual.

## 4. Design Principles

Applied to every decision in this spec:

- **R8 LLM Sovereignty** — deterministic code does not judge importance,
  staleness, relevance, or priority. Those are LLM read-time decisions.
- **R10 Intelligence Lives in the Prompt** — removed heuristics are not
  re-implemented as middleware; the main-loop LLM call absorbs the
  judgment.
- **P6 KISS/YAGNI** — delete > refactor > add. No abstractions for
  hypothetical future needs.
- **P2 High Cohesion** — category directories become the one place that
  encodes "this content is persistent enough to always-load".

## 5. Current-State Audit

### 5.1 Short-term memory (`MemoryTier::ShortTerm`)

| Signal | Status |
|---|---|
| `MemoryFact.tier` field populated by `NoteSearchResult::to_memory_fact` | Hardcoded `LongTerm`. ShortTerm value never reaches retrieval. |
| `ComptrollerConfig` / `ContextComposer` branching on tier | Vestigial. |
| Session-scoped recall | Fully served by `raw_memories` path-prefix queries (`aleph://session/{id}/…`) via `memory_search` `scope=current_session`, `recall_context`, and `SessionSummarySource`. |

**Verdict:** `MemoryTier` is conceptually dead. The "short-term" surface
area is already elsewhere.

### 5.2 Strength / confidence decay

| Signal | Status |
|---|---|
| `MemoryFact.strength` | Hardcoded to `1.0` in `NoteSearchResult::to_memory_fact`. Never decays at retrieval. |
| `MemoryFact.confidence` | Hardcoded to the RRF/vector score. Then used as input to `importance_weight` which scales by `0.7 + 0.3 * confidence`. This is **self-referential** — retrieval score scales itself. |
| `ApplyDecayCommand` → `StrengthDecayed` event | Projected via `MemoryCommandHandler::project_to_notes`, but the projection writes markdown whose `strength` is never read back. Event log grows; effect on retrieval is zero. |
| `ValueEstimator` | Residual keyword + LLM hybrid scorer; feeds `fact.confidence` on extraction. Outcome: see the confidence row above — self-referential pollution. Comment in code explicitly notes `SignalDetector` was already removed for violating LLM sovereignty. |

**Verdict:** The decay machinery is a partially-removed system. Finish
the removal.

### 5.3 Age-based ranking + archival

| Signal | Status |
|---|---|
| `time_decay` pipeline stage | Works. Reads `fact.created_at` (from frontmatter). Floor 0.5. |
| `recency_boost` pipeline stage | Works. Additive boost, independent of content state. |
| Dream Daemon `NoteDecay` | Works. Archives (never deletes) by `access × recency × links` to `archive/{category}/`. |
| `recall_signals` telemetry | Works. Powers `NoteDecay`'s `last_accessed_at` and drives dedup via `(note_path, query_hash, day_bucket, channel)`. |

**Verdict:** This is the part that stays. All four are stateless or
filesystem-level; none infer semantic importance.

### 5.4 Core-set injection (`ContextComposer`)

`ContextComposer::build_core_filter` currently matches `tier=Core AND
(scope=Global OR scope=Persona(P))`. When `MemoryTier` is deleted (§5.1),
this filter stops making sense. A replacement is required; see §7.

## 6. Philosophical Framing

### 6.1 "Short-term memory" is a SQLite-era artifact

In a SQLite-only world, every conversation turn produced a row that
either became "short-term" (decaying, transient) or "long-term"
(persistent). Tier was the only way to distinguish them.

In the note-first world, the distinction is **structural**:

- **Active conversation** → the live context window.
- **Session history** → `raw_memories` rows with path
  `aleph://session/{id}/…`, structured into d0/d1/d2 summaries by
  `SessionCompactor`, retrievable via `recall_context` and session-scoped
  `memory_search`.
- **Persistent knowledge** → markdown notes under
  `~/.aleph/memory/note/{agent_id}/{category}/*.md`.

Each layer has its own storage, its own retrieval, its own lifecycle.
There is no fact that spans these layers and needs a tier-based switch.

### 6.2 "Decay" as identity confusion

The old decay system conflated three separate concerns:

1. **Ranking preference for recent content** — a query-time signal.
2. **Archival of dormant content** — a filesystem janitorial operation.
3. **Encoding of semantic importance** — a judgment that belongs to the LLM.

Concerns 1 and 2 are legitimate and stateless; concern 3 violates R8.
Delete concern 3; keep concerns 1 and 2.

### 6.3 Always-loaded context as structural choice

The system prompt contains a bounded set of notes that must always be
present (identity, persona, user preferences) regardless of the current
query. The old mechanism was `tier=Core`. The new mechanism is
**category membership**:

- Writing a note to `persona/` declares "this is part of who the agent
  or user is — always load."
- Writing to `preference/` declares "this is a stable preference —
  always load if budget permits."
- Writing to any other category declares "load when relevant."

This is not importance inference; it is an explicit action by the author
(user or LLM via `note_manage`). The system only honors the structure.

## 7. Disposition Summary

| Mechanism | Verdict | Replacement |
|---|---|---|
| `MemoryTier` enum + all `tier=…` filters | **Delete** | None. Not needed. |
| `MemoryScope::Core` branching | **Delete** | Category-based core-set selection (§8). |
| `MemoryFact.strength` in retrieval path | **Delete** | None. Strength was already hardcoded. |
| `MemoryFact.confidence` in `importance_weight` | **Delete** | None. Self-referential. |
| `importance_weight` pipeline stage | **Delete** | None. LLM judges importance from content. |
| `ApplyDecayCommand` + `StrengthDecayed` event | **Delete** | None. Was writing zombie events. |
| `ValueEstimator` + `CortexValueEstimator` + `LlmScorer` | **Delete** | None. Confidence becomes structural. |
| `ContextComposer::build_core_filter` (tier-based) | **Rewrite** | Category-based filter (§8). |
| `ContextComposer::build_retrieval_filter` (scope stack) | **Simplify** | `agent_id` + `namespace` + `is_valid` only. No tier. |
| `time_decay` / `recency_boost` | **Keep** | — |
| Dream Daemon `NoteDecay` | **Keep** | — |
| `recall_signals` telemetry | **Keep** | — |
| `stale: true` frontmatter marker from `NoteDrift` | **Keep** | — |
| `MemoryFact` struct (as a DTO) | **Shrink** | Remove `tier`, `strength`, `scope`, `confidence`. Keep `id`, `content`, `path`, `tags`, `agent`, `created_at`, `updated_at`, `is_valid`. Or deprecate the struct entirely in favor of `NoteSearchResult` once downstream callers are updated. Decision deferred to writing-plans stage. |

## 8. Core-Set Injection: Category-as-Routing

### 8.1 Rule

`ContextComposer::build_core_filter` becomes:

```text
category IN ('persona', 'preference')
  AND agent_id = $agent
  AND namespace = $namespace
  AND is_valid = true
```

Any note under `persona/` or `preference/` is a **core candidate**.
Selection within that candidate pool is governed by the budget strategy
in §9.

`ContextComposer::build_retrieval_filter` becomes:

```text
agent_id = $agent
  AND namespace = $namespace
  AND is_valid = true
```

No tier, no scope stack, no persona branching. The retrieval pool is
"everything the agent can legitimately see," and scoring ranks it.

### 8.2 Why persona and preference only

- **`persona/`** — identity content: who the agent is, who the user is,
  relationships. Should be small (2–10 notes typical).
- **`preference/`** — stable preferences expressed by the user or
  confirmed by the agent. Slightly larger (up to ~50 notes typical).

Other categories (`plan/`, `project/`, `learning/`, `tool/`, `lesson/`,
`skill/`, `wiki/`, `transcript/`, `subagent-*/`, `other/`) are
query-scoped: loaded only when retrieval ranks them high enough.

Rationale: every category that is not identity-or-preference is
either (a) inherently query-relevant (project, learning) or (b) a
high-volume derivative (transcript, subagent-*) that would dominate the
system prompt if always-loaded.

## 9. Budget Strategy (Option iii)

### 9.1 Selection algorithm

```text
core_budget_tokens   = 2000        # default; configurable
estimator            = chars / 4   # same heuristic ContextComptroller uses

notes_p = list_notes where category = 'persona'    and is_valid
notes_f = list_notes where category = 'preference' and is_valid

persona_ordered    = notes_p sorted by updated_at DESC
preference_ordered = notes_f sorted by updated_at DESC

selected = []
remaining_budget = core_budget_tokens

for n in persona_ordered:
    cost = estimator(n.body)
    if cost <= remaining_budget:
        selected.append(n)
        remaining_budget -= cost
    else:
        overflow.append(n)   # still reachable by query-time retrieval

for n in preference_ordered:
    cost = estimator(n.body)
    if cost <= remaining_budget:
        selected.append(n)
        remaining_budget -= cost
    else:
        overflow.append(n)

return CoreSet { selected, overflow_paths }
```

Key properties:

- **Persona beats preference** on byte-for-byte budget competition.
- **Within a category, recent wins.** `updated_at DESC` is the only
  tiebreaker — no importance score.
- **Overflow is not lost.** Any persona/preference note that doesn't
  fit remains in the normal `is_valid` pool and is retrievable via
  `NoteFactRetrieval::retrieve` on the current query. Dedup against
  `selected` happens at assembly time (§9.3).
- **Stateless.** No disk writes; re-computed per composition request.

### 9.2 Budget tuning

`core_budget_tokens` defaults to **2000** based on:

- Typical persona note: 100–300 tokens.
- 2000 tokens → ~6–20 persona notes, still leaving the retrieval pool
  room in a ~100k-token window.
- User override via `[memory.context_composer] core_budget_tokens = N`
  in config.

**Failure mode:** if `persona/` alone exceeds budget, log a
`WARN` with path list and truncate by `updated_at DESC` within persona.
`preference/` gets nothing in that case. This is a configuration
problem, not a code problem — the user has written identity content
that exceeds any reasonable system-prompt budget.

### 9.3 Deduplication at assembly

`ComposedContext` already separates `core_facts` from `relevant_facts`.
The retrieval path (`NoteFactRetrieval::retrieve`) returns a list of
ranked paths. After retrieval:

```text
core_paths   = { n.path for n in selected }
filtered     = [ f for f in retrieved if f.path not in core_paths ]
```

This ensures no note is injected twice when a persona/preference note
also ranks well on the current query.

## 10. Removal List

Items to delete, grouped by file/module. Each group should land as its
own commit with green `cargo check` and `cargo test -p alephcore --lib`.

### 10.1 Tier and scope (enum level)

Actual shape at time of writing:

- `MemoryTier { Core, ShortTerm, LongTerm }` (defaults to `ShortTerm`).
- `MemoryScope { Global, Agent, Persona, SessionLocal }` (defaults to
  `Global`). `SessionLocal` is used by `SessionCompactor` to mark
  intra-session facts.

Changes:

- `src/memory/context/enums.rs` — **delete `MemoryTier` enum entirely**
  (all three variants are either dead or replaced by category routing).
- `src/memory/context/enums.rs` — **shrink `MemoryScope`** by dropping
  `Agent` and `Persona` (redundant once retrieval filter collapses per
  §8.1). Keep `Global` and `SessionLocal` — `SessionLocal` remains
  meaningful for session-compactor semantics. If after downstream
  updates `MemoryScope` is only `Global | SessionLocal` and no code
  branches on the value, consider collapsing to a `bool is_session_local`
  on the relevant store rows; decision deferred to writing-plans.
- `src/memory/context/fact.rs` — remove `tier`, `scope`, `strength`,
  `confidence` fields from `MemoryFact`. (If `MemoryScope` survives as
  `Global | SessionLocal`, `scope` may remain on `MemoryFact` as a
  session-flag; revisit in writing-plans once call sites are enumerated.)
- `src/memory/context/tests/enum_tests.rs`, `fact_tests.rs` — delete
  tier tests; update scope and fact tests.
- `src/memory/proptest_enums.rs` — drop `MemoryTier` arbiter; shrink
  `MemoryScope` arbiter to surviving variants.

### 10.2 Retrieval bridge

- `src/memory/notes/search_result.rs` — `to_memory_fact`: drop the
  hardcoded `tier`, `strength`, `scope`, `confidence` lines. Keep
  `is_valid = true`, `created_at`, `updated_at`, `path`, `tags`.
- `src/memory/note_retrieval/mod.rs` — scrub any references to removed
  `MemoryFact` fields.

### 10.3 Scoring pipeline

- `src/memory/scoring_pipeline/stages/importance_weight.rs` — delete file.
- `src/memory/scoring_pipeline/stages/mod.rs` — remove `pub mod
  importance_weight`.
- `src/memory/scoring_pipeline/mod.rs` — remove the
  `importance_weight::ImportanceWeightStage` insertion in
  `from_config`.
- `src/memory/scoring_pipeline/config` (wherever defined) — no key was
  exposed for this stage, so no config cleanup required.

### 10.4 Value estimation

- `src/memory/value_estimator/` — delete the entire directory
  (`mod.rs`, `estimator.rs`, `cortex.rs`, `llm_tests.rs`).
- Cross-reference the `memory-legacy-cleanup` spec — if that spec also
  touches `value_estimator/cortex.rs` for the cortex migration,
  coordinate which spec lands first.
- Remove any `[memory.value_estimator]` config section from
  `src/config/types/memory.rs`.

### 10.5 Event sourcing

- `src/memory/events/commands.rs` — delete `ApplyDecayCommand` struct
  and its `Command::execute` impl.
- `src/memory/events/mod.rs` — remove `MemoryEvent::StrengthDecayed`
  variant and any pattern arms.
- `src/memory/events/projector.rs` — in `fold_events_to_fact`, drop
  the `StrengthDecayed` match arm.
- `src/memory/events/handler.rs` — drop any `ApplyDecay` routing in
  `MemoryCommandHandler`.
- `src/memory/events/migration.rs` — if there is migration logic for
  `StrengthDecayed`, remove it; persisted events with the old variant
  become deserialization warnings (log and skip).
- `src/memory/events/traveler.rs` — drop any time-travel renderer for
  the deleted variant.

**Backwards compatibility:** the SQLite event log may contain
historical `StrengthDecayed` rows from previous runs. The projector
must tolerate them by **skipping unknown variants with a log line**
rather than erroring. Concrete mechanism deferred to writing-plans.

### 10.6 Composer and comptroller

- `src/memory/composer.rs` —
  - `build_core_filter`: replace tier/scope logic with the
    category-based filter from §8.1.
  - `build_retrieval_filter`: collapse to `agent_id + namespace +
    is_valid` filter from §8.1.
  - Remove `persona_id` field from `CompositionRequest` if it becomes
    unused after scope changes (it may still select between
    `persona/*` contents in a future multi-persona world; decision
    deferred).
  - `ComposedContext` keeps `core_facts` / `relevant_facts` split;
    `core_facts` is now populated by the §9 budget algorithm.
- `src/memory/context_comptroller/` — no direct changes beyond
  removing any `tier` / `strength` references if present.

### 10.7 Store and projection

- `src/memory/store/types.rs` — drop tier/strength fields from any
  fact DTOs.
- `src/memory/store/sqlite/*.rs` — no schema changes required; the
  `notes_index` table does not store tier/strength/confidence.
  `recall_signals` stays as-is.

### 10.8 Session compactor

- `src/memory/session_compactor/summary_engine.rs` — scrub any
  `MemoryTier` / `.strength` references. Session compaction writes to
  `raw_memories`, which does not carry these fields; references here
  are likely on the DTO path and can be deleted.

### 10.9 Ripple and integration tests

- `src/memory/ripple/tests.rs` — update test fact construction to use
  the shrunk `MemoryFact` (no tier/strength/confidence).
- `src/memory/integration_tests/mod.rs` — same.

### 10.10 Public module surface

- `src/memory/mod.rs` — remove any `pub use` re-exports for deleted
  types (`MemoryTier`, `ValueEstimator`, etc.).

## 11. Retention List

Explicitly preserved — these are not touched by this spec:

- `time_decay`, `recency_boost`, `cosine_rerank`,
  `length_normalization`, `hard_min_score`, `mmr_diversity` pipeline
  stages.
- All `NoteStore` methods and the `hybrid_search_notes` RRF algorithm.
- Dream Daemon stages (`NoteConsolidate`, `NoteDrift`, `NoteSynthesis`,
  `NoteLint`, `NoteDecay`, `DailyDigest`).
- `recall_signals` table, schema, and dedup index.
- `stale: true` frontmatter marker written by `NoteDrift`.
- `raw_memories` table, `SessionCompactor`, `TranscriptIndexer`,
  `recall_context` tool.
- `memory_search` tool (both `all` and `current_session` scopes).
- `ContextComposer` as a module (restructured, not removed).
- Event-sourcing commands that are not decay-related:
  `CreateFactCommand`, `UpdateContentCommand`, `InvalidateFactCommand`,
  `RestoreFactCommand`, `RecordAccessCommand`, `ConsolidateCommand`,
  `DeleteFactCommand`. These project to note mutations and remain the
  audit log. Note: their internal command names still contain "Fact"
  for historical reasons; renaming is out of scope.

## 12. Config Changes

Additions to `src/config/types/memory.rs`:

```rust
pub struct ContextComposerConfig {
    #[serde(default = "default_core_budget_tokens")]
    pub core_budget_tokens: usize,
}

fn default_core_budget_tokens() -> usize { 2000 }
```

Wired into `MemoryConfig` as `context_composer: ContextComposerConfig`
with `#[serde(default)]`. Existing configs silently inherit the default.

Deletions (if present): `[memory.value_estimator]` block.

## 13. Success Criteria

After **every** commit (not just at the end):

- `cargo check -p alephcore` passes.
- `cargo test -p alephcore --lib` passes.
- `cargo clippy -p alephcore -- -D warnings` introduces no new warnings
  in touched files.
- `grep -rn "MemoryTier\|ShortTerm\|ApplyDecayCommand\|StrengthDecayed\|ValueEstimator\|importance_weight" src/` returns zero hits after the
  full series lands (partial hits allowed mid-series).
- `~/.aleph/data/memory.db` with historical `StrengthDecayed` events in
  the event log starts cleanly; unknown variants are logged and
  skipped, not fatal.
- A user turn with an empty `persona/` and `preference/` directory
  composes a valid system prompt (empty core set is legal).
- A user turn with `persona/` exceeding `core_budget_tokens` logs a
  `WARN` with path list and still composes a valid system prompt
  (truncated).
- `memory_search`, `recall_context`, `memory_browse`, `memory_explore`
  return results identical to pre-change output for queries that do not
  depend on the deleted fields.

## 14. Migration Notes

- **Event log:** historical `StrengthDecayed` rows must deserialize
  via a permissive path (log + skip). The `MemoryEventEnvelope` is
  serialized JSON; if `serde` tagged-enum parsing rejects unknowns,
  switch to `#[serde(other)]` catch-all or pre-filter before parsing.
  Exact mechanism chosen in writing-plans.
- **MemoryFact callers:** every call site that reads `.tier`,
  `.strength`, `.confidence`, or `.scope` must be updated. These are
  likely concentrated in `composer.rs`, `comptroller`, and
  integration tests; the grep in §13 identifies the exact set.
- **No data migration for `notes_index`** — none of the deleted
  fields are stored there.
- **No user-facing CLI migration** — the tools API surface
  (`memory_search`, `recall_context`, `memory_browse`,
  `memory_explore`, `note_manage`) is unchanged.

## 15. Risks

| Risk | Mitigation |
|---|---|
| Historical `StrengthDecayed` events in production event logs become poison | Permissive deserialization path (log + skip). Validated against a test DB containing synthetic old events. |
| Users with large `persona/` directories see budget truncation silently | `WARN` log on first truncation per session; `memory_browse` can show current selection. |
| Deleting `MemoryTier` breaks downstream consumers outside `src/memory/` | `grep -rn "MemoryTier" src/` before commit 1 to scope the blast radius; update each site. |
| Deduplication in §9.3 has an off-by-one on path string format (e.g. `note://` prefix vs raw `category/filename`) | Centralize path normalization in a single helper; unit test against both forms. |
| `ValueEstimator` deletion conflicts with a concurrent `memory-legacy-cleanup` landing | Land this spec's §10.4 after the other spec's cortex work, or coordinate via a single commit touching both. |

## 16. Open Questions (Deferred to writing-plans)

1. Does `MemoryFact` survive as a shrunk DTO, or is it retired in favor
   of `NoteSearchResult` at every call site?
2. Is `persona_id` on `CompositionRequest` still needed after the scope
   stack is removed?
3. Should the `WARN` log for budget truncation be rate-limited? (Likely
   yes — once per session per agent.)
4. Event-log migration: `#[serde(other)]` catch-all, manual
   pre-filtering, or event-version field?

## 17. See Also

- `docs/reference/memory/NOTES.md` — the markdown + index substrate.
- `docs/reference/memory/RETRIEVAL.md` — scoring pipeline and
  composer.
- `docs/reference/memory/DREAM_DAEMON.md` — archival and drift
  detection that stays.
- `docs/reference/memory/RAW_MEMORY.md` — session-scoped ephemeral
  layer that replaces "short-term" semantically.
- `CLAUDE.md` — R8 LLM Sovereignty, R10 Intelligence Lives in the
  Prompt.
- `docs/superpowers/specs/2026-04-12-memory-legacy-cleanup-design.md`
  — parallel dead-code-sweep spec.
