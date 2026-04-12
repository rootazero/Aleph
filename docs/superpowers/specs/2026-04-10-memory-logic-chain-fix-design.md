# Memory Module Logic Chain Fix

> Fix disconnected implementations, dead code, and broken pipelines in `src/memory/`

**Date**: 2026-04-10
**Scope**: 166 .rs files in `src/memory/`, based on 6-agent parallel logic review

---

## Problem Statement

A comprehensive logic review of the memory module revealed:
- **8 Critical issues**: trait with no implementation, no-op storage methods, retrieval pipeline 90% disconnected, stub stages in dream pipeline, parallel reimplementations
- **6 Warnings**: event sourcing unwired, dead rerank/query modules, incomplete context composer
- **~15 dead code modules**: written but never called from any production path

The core data flow (SessionCompactor → CompressionService → FactExtractor → insert_fact) works, but most surrounding infrastructure (hybrid search, scoring pipeline, reranking, event sourcing, cognitive modules) is disconnected.

---

## Execution Order

L2 (cleanup) → L1 (persistence) → L3 (retrieval) → L4 (dream) → L5 (docs)

Cleanup first: fewer files means faster compilation and cleaner diffs for subsequent layers.

---

## L1: Data Persistence Fix (Highest Priority)

### Problem

`DreamStore` (4 methods) and `CompressionStore` (3 methods) in `store/sqlite/sessions.rs` are all no-ops. Data is silently discarded:
- Dream status resets on restart → duplicate dream runs
- Daily insights lost → consolidation summaries disappear
- Compression timestamp lost → full re-compression after restart

### Solution

Add three SQLite tables in `SqliteMemoryBackend::new()` migration path:

```sql
CREATE TABLE IF NOT EXISTS dream_status (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at INTEGER,
    last_status TEXT,
    last_duration_ms INTEGER
);

CREATE TABLE IF NOT EXISTS daily_insights (
    date TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    source_memory_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS compression_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

Implement all 7 methods with real SQLite reads/writes replacing the no-ops.

### Files Changed

- `src/memory/store/sqlite/sessions.rs` — implement 7 methods
- `src/memory/store/sqlite/schema.rs` — add 3 table migrations

---

## L2: Dead Code Cleanup

### Modules to Delete

| Category | Files | Reason |
|----------|-------|--------|
| Cognitive (replaced by stages) | `evolution/detector.rs` | DriftDetectStage reimplements |
| | `evolution/resolver.rs` | No callers |
| | `evolution/chain.rs` | No write path creates EvolutionNode |
| | `evolution/mod.rs`, `evolution/tests.rs` | Parent module for deleted files |
| | `consolidation/analyzer.rs`, `consolidation/profile.rs` | ConsolidateStage reimplements |
| | `consolidation/mod.rs`, `consolidation/tests.rs` | Parent module for deleted files |
| | `reflection/` (entire directory) | Zero external references |
| | `promotion.rs` | ConsolidateStage hardcodes promotion |
| Auxiliary (never used) | `backup.rs` | Zero callers, redesign later |
| | `cleanup.rs` | Zero callers, redesign later |
| | `performance_monitor.rs` | Declared but never imported |
| | `lazy_decay.rs` | Never instantiated |
| | `adaptive_retrieval.rs` | Gate never gates anything |
| | `vfs/migration.rs` | Zero callers |
| | `compression/archival.rs` | Zero callers |
| Orphaned trait | `MemoryEventStore` trait in `store/mod.rs` | No type implements it |

### Cleanup in Existing Files

- `mod.rs`: remove `pub mod` declarations and `pub use` re-exports for all deleted modules
- `ingestion.rs`: remove `_noise_filter` field (instantiated but never used)
- `compression/mod.rs`: remove `archival` re-exports

### Preserved (valuable, wire later)

- `rerank/` — 5 provider implementations complete, next-round connection
- `query_expander.rs` — functional synonym expansion, next round
- `events/handler.rs`, `events/projector.rs`, `events/traveler.rs` — event sourcing for dedicated project
- `transcript_indexer/` — fix `_indexer` field in L3

---

## L3: Retrieval Pipeline Connection

### Problem

Production retrieval only uses `FactRetrieval` → `vector_search` (pure vector). HybridRetrieval (vector + BM25 RRF fusion) and ScoringPipeline (7 scoring stages) exist and are tested but never instantiated in production.

### Current Path

```
memory_search tool → FactRetrieval.retrieve() → database.vector_search()
```

### Target Path

```
memory_search tool → HybridRetrieval.search_facts()
  → database.vector_search() + database.text_search()
  → RRF fusion (k=60)
  → ScoringPipeline.run() [recency_boost, time_decay, importance_weight, ...]
  → results
```

### Changes

1. **`builtin_tools/memory_search.rs`**: construct `HybridRetrieval` instance instead of calling `FactRetrieval` directly. `HybridRetrieval` internally calls `ScoringPipeline` — no extra wiring needed.

2. **`FactRetrieval`**: retain as fallback when embedder is unavailable.

3. **`retrieval.rs` (MemoryRetrieval stub)**: change `retrieve_memories()` from returning empty Vec to delegating to `FactRetrieval.retrieve()`. This fixes `MemoryStrategy`'s AI retrieval zero-candidate problem.

4. **`transcript_indexer`**: fix `_indexer` field in `memory_search.rs` — remove underscore prefix and call indexer to supplement transcript chunks in retrieval results.

### Not Changed (next round)

- `rerank/` — requires network calls, higher risk
- `query_expander` — needs synonym table quality validation
- `AdaptiveRetrievalGate` — needs benchmark data

---

## L4: Dream Pipeline Patch

### WikiIngestStage → Remove from Pipeline

`execute` only logs "LLM ingestion pending" and returns. Remove from `DreamPipeline::daily()` registration. Keep `wiki_ingest.rs` file for future wiki feature completion.

### WikiLintStage → Persist Report

`WikiLintReport` is built as a local variable then discarded. Fix: serialize report and store on `DreamContext` or persist to `PersistedDreamReport.lint_summary` field.

### TunnelDiscoveryStage → Remove from Pipeline

`should_run` depends on `tunnel_pending` which nothing ever sets. Remove from `DreamPipeline::daily()`. Keep `tunnel.rs` file for future tunnel write-path implementation.

### Result

Daily pipeline: 7 stages → 5 stages

```
SummarizeStage → DriftDetectStage → ConsolidateStage → WikiLintStage → DecayStage
```

Every remaining stage has verified, functional implementation.

---

## L5: Documentation Sync

### `docs/reference/MEMORY_SYSTEM.md`

- Fix `mod.rs` top comment (still says "SQLite + sqlite-vec", should be "SQLite")
- Remove sections for deleted modules (Reflection, ArchivalService, RippleTask usage examples)
- Update retrieval flow diagram to show HybridRetrieval as production path
- Update DreamPipeline description: 5 stages, not 7
- Mark rerank, query_expander, event sourcing as "implemented / pending connection"

### `src/memory/mod.rs` Top Comment

Align with actual `pub mod` list after cleanup.

### No New Documents

No migration guide or separate design doc beyond this spec.

---

## Estimated Impact

| Layer | Content | LOC Change |
|-------|---------|------------|
| L1 Persistence | DreamStore + CompressionStore impl | ~+200 |
| L2 Cleanup | Delete ~15 modules + re-export cleanup | ~-3000 |
| L3 Retrieval | HybridRetrieval connection + stub fix | ~100 modified |
| L4 Dream | Remove 2 stub stages + WikiLint persist | ~50 modified |
| L5 Docs | MEMORY_SYSTEM.md + mod.rs comments | ~200 modified |

**Net effect**: ~2500 lines removed, production functionality increased.

---

## Risk Assessment

| Risk | Mitigation |
|------|-----------|
| L2 deletion breaks compilation | Incremental: delete one module, compile, repeat |
| L3 HybridRetrieval changes retrieval behavior | Existing HybridRetrieval tests pass; add integration test comparing results |
| L1 schema migration on existing databases | Use `CREATE TABLE IF NOT EXISTS`; no destructive changes |
| L4 reducing pipeline stages | Only removing no-op stages; functional stages untouched |

---

## Out of Scope

- Event sourcing production wiring (dedicated project)
- Rerank provider integration (next round)
- Query expansion integration (next round)
- Backup/cleanup feature redesign (dedicated project)
- AdaptiveRetrievalGate (needs benchmark data)
- ContextComposer retrieval execution (needs ACMA design review)
