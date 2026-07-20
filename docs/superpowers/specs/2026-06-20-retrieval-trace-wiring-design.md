# Retrieval Scoring Transparency — `memory.retrieve_with_trace` Wiring (Design Spec)

**Date:** 2026-06-20
**Scope:** Aleph panel 记忆面板三方向重构之 #3（补齐缺失支柱+治理）的**第一块**：检索打分透明度。
**Status:** Approved design — ready for implementation plan.

---

## 1. Goal & Context

The panel already ships a **Retrieval Debug Panel** (Settings ▸ Memory, collapsed by default) that calls `memory.retrieve_with_trace` and renders a pipeline-stage table + scored results. But the backend handler is a **placeholder** (`src/gateway/handlers/memory_config.rs:136`) that always returns `{query, results: [], status: "placeholder"}`. The rich scoring pipeline (`NoteFactRetrieval` — RRF hybrid search, graph expansion, cross-encoder rerank, recency decay, reinforcement, MMR) computes real relevance scores but they are **discarded** on the `memory.search` path and never reach any RPC.

**Goal:** Wire the placeholder to the real scorer so a user typing a query into the existing debug panel sees: (a) the real retrieved notes with real final relevance scores, and (b) real per-stage telemetry (name / duration / input→output counts).

**This is a thin backend read-through.** The panel UI already exists and consumes the contract — **no frontend change**.

## 2. Architecture

Read-only JSON-RPC handler over the existing retrieval pipeline:

1. Add an **additive** `NoteFactRetrieval::retrieve_traced()` that runs the exact same orchestration as `retrieve()` but records each stage's timing + working-set sizes. Single source of truth via an internal `retrieve_inner(.., sink)`; the hot `retrieve()` path passes a no-op sink (byte-identical behavior, zero allocation).
2. Move `handle_retrieve_with_trace` from the dependency-free registry block into `register_memory_handlers`, where `memory_db` + `embedder` + `app_config` are available; construct `NoteFactRetrieval` per the production recipe; call `retrieve_traced`; map results+stages to the panel's wire contract.

**Units (high cohesion, clear boundaries):**

| Unit | Files | Responsibility |
|---|---|---|
| 1. Trace instrumentation | `src/memory/note_retrieval/trace.rs` (new), `src/memory/note_retrieval/mod.rs` | `StageTrace` type, `TraceSink`, `retrieve_inner` refactor, `retrieve_traced`, sub-step recording in `apply_rerank`/`apply_scoring` |
| 2. Handler | `src/gateway/handlers/memory_config.rs` (or a new sibling), registration in `src/bin/aleph-server/commands/start/builder/handlers/memory.rs`, deregistration in `src/gateway/handlers/mod.rs` | Real handler: param parsing, construction, embedder degradation, response mapping |
| 3. Contract alignment | (verification only) `interfaces/webchat/src/api/memory_config.rs` | Confirm backend JSON matches panel types field-by-field |

## 3. Redline / Principle Check

- **R4** (Interface = pure I/O): panel only renders backend response; no panel logic added. ✓
- **R7 / R9** (LLM sovereignty / intelligence in prompt): trace exposes raw telemetry only; no heuristic "why retrieved" interpretation in code. ✓
- **R10** (thin harness): memory subsystem, not `src/harness/`. Not applicable. ✓
- **P6 / YAGNI**: no per-result score breakdown (panel ignores `TraceStage.scores`); no new panel UI; no hub surfacing. ✓
- Instrumentation is additive and observational — `retrieve()` semantics unchanged. ✓

## 4. Unit 1 — Trace Instrumentation

### 4.1 Types (`src/memory/note_retrieval/trace.rs`, new small file)

```rust
/// One scoring-pipeline stage's telemetry, recorded inline during retrieval.
pub struct StageTrace {
    pub name: String,         // see stage names below
    pub duration_ms: u64,
    pub input_count: usize,   // working-set size entering the stage
    pub output_count: usize,  // working-set size leaving the stage
}

/// Collects stage telemetry only when retrieval is run in traced mode.
/// `Off` is the hot path: `record` is a no-op, no allocation.
pub enum TraceSink {
    Off,
    On(Vec<StageTrace>),
}

impl TraceSink {
    pub fn record(&mut self, name: &str, duration_ms: u64, input: usize, output: usize) { /* push only when On */ }
    pub fn into_stages(self) -> Vec<StageTrace> { /* On -> vec; Off -> [] */ }
}
```

Core type carries **no serde** — the wire format is owned by the gateway boundary (Unit 2), so the core can evolve independently.

### 4.2 Single-source-of-truth refactor (`note_retrieval/mod.rs`)

- Extract the body of the existing `retrieve()` into:
  `async fn retrieve_inner(&self, query, agent_id, limit, sink: &mut TraceSink) -> Result<Vec<ScoredFact>, AlephError>`
- `pub async fn retrieve(..)` becomes `retrieve_inner(.., &mut TraceSink::Off)` — **results and ordering byte-identical** (trace is purely observational).
- New `pub async fn retrieve_traced(&self, query, agent_id, limit) -> Result<(Vec<ScoredFact>, Vec<StageTrace>), AlephError>` runs with `TraceSink::On(vec![])` and returns `(results, sink.into_stages())`.
- `apply_rerank` and `apply_scoring` gain a `sink: &mut TraceSink` parameter so recency / reinforcement / MMR sub-steps are timed at their real call sites (not glommed into one "scoring" row).

### 4.3 Stage recording

Wrap each real call site with `Instant` timing and `sink.record(...)`. Emit **only stages that actually run**:

| Stage name | When emitted | input → output |
|---|---|---|
| `hybrid_search` | embedder present & embed OK | `0` → candidate count |
| `fts_search` | embedder absent or embed failed (FTS fallback) | `0` → candidate count |
| `graph_expand` | expansion config active | seeds → seeds+peers |
| `rerank` | reranker present | N → N |
| `recency_decay` | recency enabled | N → N |
| `reinforcement` | reinforcement enabled | N → N |
| `mmr_diversity` | MMR enabled | N → N (reorder) |
| `truncate` | always | N → `min(N, limit)` |

`input_count` of each stage == previous stage's `output_count`. The first search stage's name makes the **FTS fallback visible** at a glance.

### 4.4 Tests (Unit 1)

- `retrieve_traced` vs `retrieve` on the same input → **identical result set and order** (proves zero side-effects from tracing). Use the existing `note_retrieval` test fixtures (temp store + test embedder).
- `stages` non-empty; first stage name ∈ {`hybrid_search`,`fts_search`}; `truncate.output_count == results.len()`; chain continuity (`stage[i].input_count == stage[i-1].output_count`).
- `TraceSink::Off` regression: `retrieve()` returns equivalent results.

## 5. Unit 2 — Handler

### 5.1 Migration & registration

- Remove the placeholder registration in the dependency-free block (`src/gateway/handlers/mod.rs:586`) — **entropy reduction, no dead path left**.
- Register the real handler in `register_memory_handlers` (`src/bin/aleph-server/commands/start/builder/handlers/memory.rs`), capturing `memory_db`, `embedder`, `app_config` like sibling handlers.

### 5.2 Parameters (backward compatible — panel sends only `{query}`)

| Param | Type | Default |
|---|---|---|
| `query` | `String` | required; empty → `INVALID_PARAMS` (preserve current behavior) |
| `agent_id` | `Option<String>` | `DEFAULT_AGENT_ID` (`"main"`) |
| `limit` | `Option<usize>` | `10` |

### 5.3 Construction (production recipe, `constructor.rs:179`)

```rust
let memory_dir = crate::utils::paths::get_note_memory_dir()?;
let indexer = Arc::new(NoteIndexer::new(memory_dir, memory_db.clone()));
let retrieval = NoteFactRetrieval::new(indexer, embedder)
    .with_rerank_config(&cfg.memory.assembler.rerank)
    .with_scoring_config(&cfg.memory.assembler.retrieval_scoring)
    .with_expansion_config(&cfg.memory.assembler.expansion);
let (results, stages) = retrieval.retrieve_traced(&query, &agent_id, limit).await?;
```
Config read from `app_config` (same source as sibling handlers). Exact config field paths confirmed against `MemoryConfig`/assembler config during planning.

### 5.4 Embedder degradation (behavior contract)

When no embedder is configured at runtime, the handler **must still work as FTS-only and must not error**. Mechanism chosen at implementation time after reading `NoteFactRetrieval::new`'s signature:
- if embedder param is required `Arc<dyn EmbeddingProvider>` and runtime value is `None`: construct with an always-error noop embedder so `retrieve()`'s built-in fallback runs `text_retrieve` (first stage records `fts_search`);
- if `new` accepts `Option`: pass `None` directly.

The spec **locks the behavior** (FTS-only available, no error); the plan picks the mechanism.

### 5.5 Response mapping (handler owns wire format — must match panel types exactly)

```jsonc
{
  "query": query,
  "trace": {
    "query": query,
    "timestamp": <unix_ms>,
    "stages": [ { "name": s.name, "duration_ms": s.duration_ms,
                  "input_count": s.input_count, "output_count": s.output_count }, ... ]
  },
  "results": [ { "id": fact.path, "content": <truncated>, "score": scored.score }, ... ]
}
```

- `id` = `fact.path` (consistent with graph node id).
- `content` truncated to `TRACE_CONTENT_MAX = 280` chars, **UTF-8 safe** via `char_indices()` (P7) — debug panel needs no full text; bounds payload.
- `stage.scores` / `ScoreSnapshot` **not emitted** (panel ignores them).

### 5.6 Error handling

- empty `query` → `INVALID_PARAMS` (preserved).
- retrieval error → `INTERNAL_ERROR` (sibling-handler convention).
- embedder absent → **not** an error (FTS degrade).

### 5.7 Tests (Unit 2)

- empty `query` → `INVALID_PARAMS` (regression of preserved behavior).
- `content` truncation UTF-8 safety unit test (multi-byte boundary does not panic).

## 6. Unit 3 — Contract Alignment (verification only)

Field-by-field check that the §5.5 response matches the panel's `RetrieveWithTraceResponse` / `RetrievalTrace` / `TraceStage` / `TracedResult` in `interfaces/webchat/src/api/memory_config.rs`:
- `TraceStage` required fields `name/duration_ms/input_count/output_count` all present.
- `TracedResult{id,content,score}` all present.
- `trace.query/timestamp/stages`, top-level `query` present.
- ignored fields (`stage.scores`, `ScoreSnapshot`) absent — fine (serde ignores unknown / defaults).
**No frontend file is modified this iteration.** Verification is a grep/read diff, not cargo.

## 7. Out of Scope (explicit boundaries)

- **No frontend changes** — the existing Settings debug panel is the delivery surface.
- **No** per-result `ScoreBreakdown` / `stage.scores` (panel ignores; YAGNI).
- **No** Memory Hub inline score surfacing (depends on #2 merge, which is unmerged off main).
- **No** dream insights / corrections governance (separate #3 sub-blocks, deferred).
- **No** wiring of the audit trail (`AuditEntry.Accessed`) into retrieval (not currently written; out of scope).

## 8. Implementation Constraints

- **Isolated worktree branch** off `main` (`memory-retrieval-trace-wiring`); never edit `main` directly.
- **`cargo check` IS allowed this iteration** (user override for #3 to ensure safety) — distinct from #1/#2's no-cargo constraint. Prefer `cargo check -p alephcore` (and `--bin aleph-server` for the handler) over full test runs; 极度节制 cargo 调用.
- **Entropy reduction**: delete the placeholder handler registration after migration.
- **连线优先**: reuse `NoteFactRetrieval`, scoring functions, construction recipe, and the existing panel; only add `retrieve_traced` + `StageTrace` + the real handler.
- Reply Chinese; code comments English.

## 9. Success Criteria

1. `cargo check -p alephcore` + `--bin aleph-server` clean.
2. `retrieve_traced` returns the same result set/order as `retrieve()` (test) + non-empty, chain-continuous stages.
3. Backend `memory.retrieve_with_trace` response deserializes into the panel's `RetrieveWithTraceResponse` with populated `trace.stages` and scored `results` (field-by-field contract check).
4. Placeholder registration removed; no dead path.
5. Empty query still returns `INVALID_PARAMS`; no-embedder runtime degrades to FTS without error.
