# Memory System Overhaul Design

**Date:** 2026-04-08
**Status:** Approved
**Scope:** Full memory pipeline repair + knowledge graph upgrade + lifecycle management

---

## Problem Statement

After the SessionStore removal (commit `e1f5eed5`), the memory system has five broken subsystems forming a causal chain:

```
SessionStore removed → CompressionService dead → No Extracted facts
→ ConflictDetector never runs → No deduplication
→ Graph never updated → Empty knowledge graph
→ Dream Cycle no LTM input → Synthesis idle
```

Additionally:
- `store_raw_chunk()` tags raw conversation text as `Extracted` instead of `SessionCompressed`
- Dashboard API returns empty for raw memories, shows raw data as compressed facts
- Knowledge graph uses regex entity extraction (misses CJK, poor relation types)
- No data lifecycle management (raw chunks never cleaned up)

## Design Decisions

1. **Single `facts` table retained** — no schema split. Lifecycle managed by pipeline + `fact_source`/`scope`/`tier` filtering. Splitting adds cross-table complexity without solving the core pipeline problem.
2. **LLM-first entity extraction** — replaces regex. Aligns with R8 (LLM Sovereignty). One LLM call extracts facts + entities + relationships.
3. **Dual-layer deduplication** — C-layer (inject existing facts into extraction prompt) + B-layer (vector initial screening + LLM arbitration for conflicts).
4. **Async decoupled compression** — SessionCompactor produces raw chunks independently; CompressionService consumes them on a 1-hour background timer.
5. **Phased Dream Cycle activation** — Phase 1: Consolidate + Decay only. Phase 2: Synthesis after LTM > 50 facts.

## Architecture Overview

```
Conversation Turn
  │
  ▼
SessionCompactor (per-turn, intra-session)
  ├─► d0/d1/d2 summaries (SessionCompressed, for context injection)
  └─► raw chunks (SessionCompressed, for fact extraction input)

CompressionService (background, 1-hour cycle)
  ├─► Read raw chunks (since last_timestamp)
  ├─► Inject existing related facts into prompt (C-layer dedup)
  ├─► LLM extraction: facts + entities + relationships (single call)
  ├─► Vector screening + LLM arbitration (B-layer dedup)
  ├─► Store Extracted facts
  ├─► Update Knowledge Graph (structured triples)
  └─► Invalidate consumed raw chunks

Dream Cycle (Daily/Weekly)
  ├─► Consolidate: promote ShortTerm → LongTerm
  ├─► Decay: reduce weight of stale information
  └─► [Phase 2] Synthesis: cluster and synthesize core knowledge

Dashboard API
  ├─► Raw memories tab: SessionCompressed source + session paths
  └─► Compressed facts tab: Extracted + Synthesis facts only
```

## Data Lifecycle

| Data Type | Tier | Scope | Lifecycle |
|-----------|------|-------|-----------|
| raw chunks | ShortTerm | SessionLocal | Invalidated after CompressionService consumes them |
| d0 summaries | ShortTerm | SessionLocal | Invalidated when condensed to d1 (existing behavior) |
| d1 summaries | ShortTerm | SessionLocal | Invalidated when condensed to d2 (existing behavior) |
| d2 summaries | ShortTerm | SessionLocal | Invalidated when session expires (24h) |
| extracted facts | LongTerm | Global | Decay-managed (half_life = 30 days), conflict replacement |
| synthesis facts | Core | Global | Near-permanent (half_life = 365 days) |

## Component Details

### 1. CompressionService Reconnection

**Data source:** Raw chunks at `aleph://session/*/raw/*` paths, fetched via `get_uncompressed_session_facts(since_timestamp)`.

**Trigger:** Background timer (1-hour default via `start_background_task()`). Also triggered by signal detection in user messages (correction signals trigger immediate compression).

**Post-consumption cleanup:** After successful extraction, invalidate consumed raw chunks with reason `"consumed_by_compression"`.

### 2. Unified LLM Extraction (FactExtractor Refactor)

Single LLM call produces three outputs:

```json
{
  "facts": [
    {
      "content": "The user prefers using Rust for backend development",
      "fact_type": "preference",
      "confidence": 0.9,
      "source_ids": ["fact-abc"]
    }
  ],
  "entities": [
    { "name": "Rust", "kind": "technology", "aliases": ["rust-lang"] },
    { "name": "Aleph", "kind": "project", "aliases": [] }
  ],
  "relationships": [
    { "subject": "user", "relation": "uses", "object": "Rust", "context": "backend" },
    { "subject": "user", "relation": "works_on", "object": "Aleph" }
  ]
}
```

**System prompt additions:**
- Inject top-10 existing related facts as context ("You already know these. Only extract genuinely new or updated information.")
- Request structured entity/relationship output alongside facts
- Support both Chinese and English entity names
- Relationship types are free-text (not enumerated)

### 3. Dual-Layer Deduplication

**C-layer (extraction-time, pre-emptive):**
1. Build embeddings for raw chunk batch
2. Vector search existing facts (top 10, threshold 0.6)
3. Inject matched facts into extraction prompt as "already known" context
4. LLM naturally avoids re-extracting known information

**B-layer (storage-time, safety net):**
1. For each extracted fact, vector search existing facts (threshold 0.7)
2. If candidates found, LLM classifies the relationship:
   - `same_updated` — old fact invalidated, new fact stored (e.g., "learning Rust" → "learned Rust for 2 months")
   - `contradicts` — old fact invalidated with reason, new fact stored (e.g., "uses Vim" → "switched to VS Code")
   - `coexists` — both retained (e.g., "likes Rust" and "also likes Go")
3. If no candidates, store directly

**LLM conflict arbitration prompt:**
```
Given an existing fact and a new fact, classify their relationship:
- same_updated: The new fact is an updated version of the existing fact
- contradicts: The new fact contradicts the existing fact
- coexists: Both facts are independently true

Existing: "{old_fact}"
New: "{new_fact}"

Output JSON: {"verdict": "same_updated|contradicts|coexists", "reason": "..."}
```

### 4. Knowledge Graph Upgrade

**Write path:** CompressionService writes LLM-output entities and relationships directly to `graph_nodes` / `graph_edges`.

- Entity `name` + `kind` → `upsert_node()`
- Relationship triple → `upsert_edge()` with `relation` as free text
- `context_key` set to source fact ID for provenance

**Existing regex extraction in `graph.rs`:** Removed. All entity/relationship extraction delegated to LLM.

**Read path (Phase 2):** During retrieval, query graph for entities mentioned in the query → find connected facts via edges → supplement vector search results.

### 5. Dream Cycle Adjustments

**Phase 1 (this implementation):**

- **Consolidate:** Change promote condition from `strength >= threshold` to `access_count >= 2 AND strength >= 0.5`. Facts must be actually accessed (retrieved in conversations) to be promoted to LongTerm.
- **Decay:** Tiered half-life:
  - ShortTerm (session) facts: half_life = 1 day (aggressive cleanup)
  - LongTerm (extracted) facts: half_life = 30 days
  - Core (synthesis) facts: half_life = 365 days
- **Other stages:** Collect, Cluster, Summarize, DriftDetect — left as-is, they will naturally activate once LTM facts exist.

**Phase 2 (future, when LTM > 50 facts):**

- Activate DeepSynthesis
- Optimize clustering parameters

### 6. Dashboard API (Already Fixed)

- `handle_search` → returns raw memories (SessionCompressed + session paths)
- `handle_list_facts` → returns only Extracted/Synthesis facts
- `handle_stats` → real counts for both categories + graph stats

## File Change Inventory

| File | Change | Priority |
|------|--------|----------|
| `src/memory/session_compactor/mod.rs` | Fix `store_raw_chunk` fact_source | Done |
| `src/memory/store/sqlite/mod.rs` | Dashboard helpers, lifecycle methods | Done + extend |
| `src/memory/store/sqlite/facts.rs` | Expose `row_to_fact_pub` | Done |
| `src/gateway/handlers/memory.rs` | Dashboard API fix | Done |
| `src/memory/compression/service.rs` | Reconnect to raw chunks source | Done + extend |
| `src/memory/compression/extractor.rs` | Unified extraction (facts + entities + relationships) | New |
| `src/memory/compression/conflict.rs` | Add LLM arbitration path | New |
| `src/memory/graph.rs` | Replace regex extraction with LLM triples input | New |
| `src/memory/dreaming/stages/consolidate.rs` | Adjust promote threshold | New |
| `src/memory/dreaming/stages/decay.rs` | Tiered decay rates | New |

## Implementation Phases

### Phase 1: Pipeline Repair (Critical)
- [x] Fix `store_raw_chunk` fact_source
- [x] Reconnect CompressionService data source
- [x] Fix dashboard API
- [ ] Add raw chunk invalidation after consumption
- [ ] Session expiry cleanup (24h)

### Phase 2: Extraction Quality
- [ ] Refactor FactExtractor for unified output (facts + entities + relationships)
- [ ] C-layer dedup (inject existing facts into prompt)
- [ ] B-layer dedup (vector screening + LLM arbitration)

### Phase 3: Knowledge Graph
- [ ] Replace regex entity extraction with LLM triple output
- [ ] Write LLM-extracted entities/relationships to graph tables
- [ ] Remove legacy regex extraction code

### Phase 4: Dream Cycle
- [ ] Adjust Consolidate promote threshold
- [ ] Implement tiered decay rates
- [ ] Guard Synthesis behind LTM count threshold

### Phase 5: Graph-Enhanced Retrieval (Future)
- [ ] Graph-based entity lookup during retrieval
- [ ] Entity-to-fact linking for context enrichment

## Non-Goals

- Schema migration / table splitting — single `facts` table retained
- Real-time compression — async 1-hour cycle is sufficient
- Event sourcing activation — existing opt-in mechanism left as-is
- Full graph-based reasoning — graph supplements vector search, does not replace it
