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
`executor/builtin_registry/`, and the boot site.
