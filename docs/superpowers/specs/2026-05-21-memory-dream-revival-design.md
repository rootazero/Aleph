# Memory Dream-Daemon Revival + Wiring-Gap Closure — Design

**Date:** 2026-05-21
**Branch:** `worktree-memory-dream-revival`
**Driver:** Compare hermes-agent's memory management against Aleph; fix bugs, close
wiring gaps, optimize, clean dead code. Non-destructive.

## Background

hermes-agent (Python) implements memory as: a char-budgeted curated Markdown store
(frozen-snapshot injection for prefix-cache stability) + an always-on FTS5 session
search + pluggable external providers. Its consolidation is **reactive** — a
char-budget back-pressure plus a per-N-turns "nudge" that forks a review agent.
hermes has **no scheduled background consolidation daemon**.

Aleph's architecture is already more advanced: a three-layer model (L0 raw → L1
notes → Dream Daemon consolidation) with scheduled window/idle-gated dreaming.
The problem is **not design** — it is unwired infrastructure and unfinished stubs.

### Findings (from two exploration sweeps + a feasibility analysis)

| Severity | Finding | Location |
|---|---|---|
| 🔴 CRITICAL | `run_dream` builds a `DreamPipeline` then drops it (`let _pipeline = pipeline`) and returns an all-zero stub `DreamReport`. All 8 dream stages (~3,500 lines) are dead at runtime. No consolidation / decay / drift / synthesis / digest ever happens. | `src/memory/dreaming/mod.rs:693-707` |
| 🟠 HIGH | `memory.search` RPC ignores its `query` param entirely — just returns recent raws ordered by time. Doc-comment is false. | `gateway/handlers/memory.rs:102` |
| 🟠 HIGH | 7 memory tools missing from `BUILTIN_TOOL_DEFINITIONS` ("authoritative source"): `memory_reflect`, `recall_context`, `note_orient`, `note_schema`, `user_profile`, `session_complete`, `flag_user_correction`. | `executor/builtin_registry/definitions.rs` |
| 🟠 MEDIUM | 4 no-op RPC handlers with TODO markers: `memory.delete`, `memory.clear`, `memory.clearFacts`, `memory.appList`. | `gateway/handlers/memory.rs` |
| 🟡 MEDIUM | Dead modules with zero production consumers (~2,650 lines): `ai_retrieval.rs`, `query_expander.rs`, `scoring_pipeline/`, `reranker.rs`, `noise_filter.rs` + dead config tunables. | `src/memory/` |

**Deferred to separate cycles:** event-sourcing wiring (~2,850 lines, invasive
write-path redirect — already scoped in `project_event_sourcing_next.md`);
`ripple::explore_tunnels` stub (feature, not a bug).

## Goal

Revive the dead L1-consolidation lifecycle and close the highest-impact wiring
gaps, then remove genuinely-dead code. Wiring the Dream Daemon gives Aleph
**scheduled background consolidation** that hermes-agent does not have — the
"learn from it, then surpass it" objective.

## Scope — 3 independently-committable phases

### Phase 1 — Dream Daemon revival (CRITICAL)

Feasibility analysis confirmed this is **pure plumbing** — every dependency
already exists as an `Arc` at the boot site `agent_init.rs:1239`.

`DreamContext` (`dreaming/mod.rs:75-93`) needs: `database` (held), `indexer`
(`NoteIndexer::new(note_dir, database)` — trivial), `provider` (held, `Option`),
`embedder` (**missing**), `orientation` (held). No `command_handler` field.

Changes (2 files):
1. `DreamDaemon` struct — add `embedder: Option<Arc<dyn EmbeddingProvider>>` and
   `note_memory_dir: Option<PathBuf>`; init `None` in `from_config`; add
   `with_embedder` builder.
2. `ensure_dream_daemon` / `ensure_dream_daemon_with_orientation` — accept the
   two new params; `ensure_dream_daemon` passes `None` (keeps `ingestion.rs`
   caller compiling).
3. `run_dream` Phase 4 — replace the stub: build `notes` from
   `database.list_notes(DEFAULT_AGENT_ID)`, build `NoteIndexer`, construct
   `DreamContext`, call `pipeline.run(ctx).await`, return the real report.
4. **Graceful degradation:** when `provider`/`embedder` are `None`, fall back to
   the `Conserve` strategy (lint/review/index-refresh only — no LLM/embedder
   stages) instead of panicking.
5. **Real `RawMetrics`:** replace `RawMetrics::default()` with a cheap helper
   computing `total_notes`, `notes_added_24h`, `never_recalled_count` from
   `count_all_notes()` + a `list_notes` scan + `recall_signals`. Makes strategy
   selection signal-driven.
6. `agent_init.rs:1239` — pass `embedder_out.clone()` + `note_memory_dir.clone()`
   (both already in scope).
7. Test: construct a `DreamContext` over an in-memory `SqliteMemoryBackend`, run a
   `Conserve` pipeline, assert stages execute and the report is non-stub.

### Phase 2 — RPC + tool-definition wiring fixes (HIGH)

1. `memory.search` — route the `query` param into `NoteFactRetrieval` (the live
   retrieval engine `HybridAssembler` uses); return scored hits. Fix the false
   doc-comment.
2. `memory.delete` / `memory.clear` — wire to real note deletion (the store
   supports deletion via the `forget`/note path); return real counts.
3. `memory.clearFacts` / `memory.appList` — wire to a real store op if one
   exists; otherwise remove the handler honestly (no silent no-op).
4. Add the 7 missing tools to `BUILTIN_TOOL_DEFINITIONS` so tool catalogs and
   `is_builtin_tool()` are correct.

### Phase 3 — Dead-code cleanup (R10 YAGNI withdrawal)

For each candidate (`ai_retrieval.rs`, `query_expander.rs`, `scoring_pipeline/`,
`reranker.rs`, `noise_filter.rs`): **verify zero consumers** with grep, then
delete the module + its `mod.rs` re-export + dead `MemoryConfig` tunables.
Any module found to have a real consumer is **kept**.

## Testing

- Phase 1: new unit test for the revived pipeline + existing `dreaming` tests
  stay green.
- Phase 2: RPC handler tests for `memory.search` returning query-relevant hits.
- Phase 3: `cargo check -p alephcore` clean after each deletion.
- `cargo clippy` clean on touched files. Honor the 3-cargo concurrency cap.

## Non-goals

No destructive refactor. No new Leptos Panel UI. No event-sourcing wiring. No new
heavy dependencies (R3). Changes stay inside `src/memory/`, `gateway/handlers/`,
and the boot site.

## Implementation status — 2026-05-21

**Shipped on `worktree-memory-dream-revival` (verified, not merged):**

- **Phase 1 — Dream Daemon revival** ✅ `c523967b0`. `run_dream` now builds a
  real `DreamContext` and runs the pipeline; graceful skip without
  provider/embedder; real `RawMetrics` from the note index. 131 dreaming
  tests pass (3 new: pipeline-execution + graceful-skip + metrics).
- **Phase 2 — `memory.search` query fix** ✅ `62d44380a`. Non-empty `query`
  now runs an FTS5 search over knowledge notes; false doc-comment fixed.
- **Phase 3 — dead-code removal** ✅ `7e9106fcd`. Deleted `ai_retrieval.rs`
  (554 lines) + `query_expander.rs` (122 lines) — verified zero consumers.

Net: +411 / −704 lines. Lib + bin compile; touched files are clippy-clean
(the one clippy error is a pre-existing baseline issue in
`markdown_skill/spec.rs`, unrelated).

**Deferred (each needs its own focused cycle):**

- **7 memory tools missing from `BUILTIN_TOOL_DEFINITIONS`** — a metadata
  inconsistency, not a runtime bug (the tools work via `constructor.rs`).
  Adding them safely needs a check for double-registration.
- **4 vestigial no-op RPC handlers** (`memory.delete/clear/clearFacts/appList`)
  — pre-notes-era; "fixing" them means redefining their semantics, which
  risks breaking the UI contract. Left as-is.
- **3 entangled dead modules** (`scoring_pipeline/`, `noise_filter.rs`,
  `reranker.rs`) — referenced from config handlers / `ingestion.rs`; not a
  clean cut, needs per-reference analysis.
- **Dead config tunables** (`ai_retrieval_*`, `query_expansion_enabled`) —
  left as harmless unused fields; removing touches config structs + tests.
- **Event-sourcing wiring** — large, invasive; see `project_event_sourcing_next`.

## Continuation cycle — 2026-05-22

All four deferred items were implemented. Two were straightforward
dead-code / bug-fix work; two needed careful scoping to stay
non-destructive.

**Shipped:**

- **Phase 3b — dead-module + dead-config removal** ✅ `9f373232f`.
  Full-tree grep confirmed `scoring_pipeline/`, `noise_filter.rs`, and
  `reranker.rs` have zero production consumers (only `mod.rs` re-exports +
  config struct fields; `noise_filter`'s supposed caller `ingestion.rs` is
  itself a no-op stub; the live reranking path uses `rerank/provider.rs`).
  Deleted all three plus the orphaned `MemoryConfig.ai_retrieval_*` /
  `.query_expansion_enabled` tunables, and the matching dead UI end-to-end
  (the `aleph-panel` memory-settings DTO fields + `AIRetrievalSettings` card
  + query-expansion toggle). Net −1,905 lines. `cargo check` clean for
  `alephcore` / `aleph-server` / `aleph-panel`; `config::types::memory`
  tests 10/10.
- **Phase 4 — honest memory-mutation RPCs** ✅ `366d07dd2`.
  `memory.delete` returned a fake `{ "ok": true }` and `memory.clear` /
  `memory.clearFacts` returned a fake `{ "deletedCount": 0 }`, so
  `aleph memory delete/clear` and the Panel delete button reported
  mutations that never ran. The notes-based model has no per-entry or bulk
  delete primitive, so all three now return explicit errors;
  `memory.appList` keeps its honest empty result with the boilerplate and
  stale TODO removed.

- **Phase 5 — register 7 memory tools in `BUILTIN_TOOL_DEFINITIONS`** ✅
  `82dd31d57`. `memory_reflect`, `recall_context`, `note_orient`,
  `note_schema`, `user_profile`, `session_complete`, and
  `flag_user_correction` were registered only via the dynamic builder in
  `constructor.rs`, so they were absent from the authoritative
  `BUILTIN_TOOL_DEFINITIONS` table — making `is_builtin_tool()`,
  `get_builtin_tool_names()`, and `agents.tools_schema` inconsistent with
  the tools actually available to the LLM. Added the 7 definitions (all
  `requires_config`, with matching `create_tool_boxed` `None` arms) and
  assigned them to the `memory_knowledge` tool-category group — required by
  `groups.rs::test_all_builtin_tools_have_a_group`, which also surfaces
  them in the Panel tool-category catalog.

- **Phase 6 — record note_manage lifecycle events into the event log** ✅
  `0d4de60f8`. The event-sourcing subsystem (`MemoryCommandHandler` /
  `EventProjector` / `MemoryTimeTraveler`) was fully built but had no
  producer — nothing ever wrote the event log, so the `memory_timeline`
  tool always returned empty. Wired the cleanest producer: `note_manage`,
  whose create/update/append/delete actions map 1:1 onto note-lifecycle
  events. After each successful write it records an event via the handler,
  keyed by the stable `category/filename` note path, so `memory_timeline`
  now has real history to fold.

  Scoping to stay non-destructive: the handler is built **without** a
  `NoteIndexer`, so its projection step is a no-op — it is a pure
  event-log writer here. `note_manage` keeps its own notes-filesystem
  write path 100% unchanged (the handler's own `project_to_notes` writes
  degenerate UUID-titled notes and must not touch real notes). Event
  recording is best-effort (the note write already committed). Routing
  `CompressionService` / dream-stage writes through the handler remains a
  genuine write-path overhaul — those hold already-built `KnowledgeNote`s
  while the handler builds notes from command fields (an architectural
  inversion) — and stays a separate scoped effort
  (`project_event_sourcing_next`).
