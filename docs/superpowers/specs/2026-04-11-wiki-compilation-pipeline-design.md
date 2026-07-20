# Graph-Driven Wiki Compilation Pipeline

## Overview

Enhance Aleph's Dream Pipeline to automatically "compile" discrete facts into structured wiki knowledge pages, inspired by Karpathy's LLM Wiki pattern. Instead of re-discovering knowledge from scratch on every query, the system incrementally builds and maintains a persistent wiki — a structured, interlinked collection of synthesized knowledge pages backed by the knowledge graph.

### Core Insight

**"Write-time compilation" replaces "query-time discovery."** Facts accumulate as atomic knowledge units via real-time extraction. During Dream Pipeline idle cycles, the system scans the knowledge graph for high-value entity nodes, collects their associated facts, and uses LLM to synthesize structured wiki pages. The wiki becomes a persistent, compounding artifact where cross-references are pre-built and contradictions pre-flagged.

### Scope

- Fill the existing `WikiIngestStage` stub with graph-driven wiki compilation logic
- Register WikiIngestStage in the daily Dream Pipeline
- Enhance WikiLintStage with stale page detection linked to source fact validity
- No new files, no new FactType variants, no retrieval path changes

## Architecture

### Pipeline Order Change

```
Before: Summarize → DriftDetect → Consolidate → WikiLint → Decay
After:  Summarize → DriftDetect → Consolidate → WikiIngest → WikiLint → Decay
```

WikiIngestStage compiles wiki pages from graph entities. WikiLint runs immediately after to health-check the results (broken links, orphan pages, suggested links).

### Three-Layer Memory Model (Unchanged)

```
Raw (SessionManager SQLite)
  ↓ fact extraction (real-time)
Facts (MemoryFact + sqlite-vec + knowledge graph)
  ↓ wiki compilation (Dream Pipeline, async)
Wiki (FactType::Wiki pages, interlinked via wikilinks + graph)
```

The Facts layer remains the real-time retrieval fallback. Wiki compilation is a background enrichment that produces higher-quality, pre-synthesized knowledge.

## WikiIngestStage Compilation Flow

### Step 1: Entity Scanning

Query `graph_nodes` for high-value entities:
- Filter: `score > min_node_score` (default 0.5)
- Sort: by score descending
- Limit: `max_pages_per_run` (default 10)
- Exclude: entities that already have a fresh wiki page (compiled within `cooldown_days`)

### Step 2: Facts Collection

For each candidate entity node:
- Query `memory_entities` table for all associated fact IDs
- Load the facts from the facts store, filtering `is_valid = true`
- Skip entity if fewer than `min_facts_for_compile` (default 3) valid facts

### Step 3: Existing Page Lookup

Determine create vs update:
- Query `memory_entities` for associations where `source = 'wiki_compile'` and `node_id = entity_node_id`
- If a matching `FactType::Wiki` fact exists → update candidate
- If no match → create candidate

For update candidates, check staleness:
```
valid_ratio = count(valid source facts) / count(total source facts)
if valid_ratio < stale_threshold (0.5):
    mark as needs recompilation
```

### Step 4: LLM Synthesis

Send collected facts to the AI provider with a structured prompt requesting markdown output:

```markdown
# {Entity Name}

## Summary
{Synthesized description integrating all facts}

## Key Facts
- {fact 1 with context}
- {fact 2 with context}

## Related
- [[related-entity-1]]
- [[related-entity-2]]
```

The prompt instructs the LLM to:
- Synthesize, not just concatenate — identify patterns and connections
- Note contradictions between facts explicitly
- Generate `[[wikilinks]]` to related entities that exist in the graph
- Keep the page concise (under 500 words)

For updates: include the existing page content in the prompt so the LLM can perform incremental revision rather than full rewrite.

### Step 5: Storage and Graph Sync

1. Create or update a `MemoryFact` with:
   - `fact_type = FactType::Wiki`
   - `fact_source = FactSource::Synthesis`
   - `path = aleph://wiki/{slug}.md` (slug derived from entity node name)
   - `source_memory_ids = [source fact IDs]`
   - `tier = MemoryTier::Core`
   - `layer = MemoryLayer::L0Abstract`
   - `scope = MemoryScope::Global`

2. Call `wiki_sync::sync_wikilinks_to_graph()` to synchronize `[[wikilinks]]` in the content to graph edges.

3. Create a `memory_entities` association with `source = 'wiki_compile'` linking the wiki fact to the entity node.

## Stale Detection and Recompilation

WikiIngestStage performs staleness checks on existing wiki pages each run:

1. Load `source_memory_ids` from the wiki fact
2. Check validity of each source fact
3. Compute `valid_ratio = valid_count / total_count`
4. If `valid_ratio < stale_threshold` (default 0.5), add to compilation queue

WikiLintStage's existing `stale_pages` field reports pages that WikiIngestStage flagged as stale, providing visibility into wiki health.

## Configuration

Extends the existing `WikiIngestConfig`:

```rust
pub struct WikiIngestConfig {
    pub enabled: bool,              // default: true
    pub max_pages_per_run: usize,   // default: 10
    pub cooldown_days: u32,         // default: 1
    pub min_node_score: f32,        // minimum entity node score, default: 0.5
    pub min_facts_for_compile: usize, // minimum facts to trigger compilation, default: 3
    pub stale_threshold: f32,       // source validity ratio below which to recompile, default: 0.5
}
```

## LLM Provider Requirement

WikiIngestStage requires `ctx.provider.is_some()` to run. When no AI provider is available, `should_run()` returns false and the stage is skipped entirely. Template-based fallback is intentionally avoided — wiki value comes from LLM synthesis quality, not mechanical concatenation.

## What This Design Does NOT Do

- **No real-time compilation** — all wiki work happens in Dream Pipeline async cycles
- **No new FactType variants** — reuses `FactType::Wiki` with `fact_source = Synthesis` to distinguish compiled pages
- **No retrieval path changes** — existing vector search naturally covers wiki facts; no routing logic needed
- **No index.md file** — Aleph uses sqlite-vec for indexing, not file-level catalogs
- **No new source files** — all changes in existing files

## Files Modified

| File | Change |
|------|--------|
| `src/memory/dreaming/stages/wiki_ingest.rs` | Fill stub with compilation logic |
| `src/memory/dreaming/mod.rs` | Register WikiIngestStage in daily pipeline |
| `src/memory/dreaming/stages/wiki_lint.rs` | Enhance stale detection to check source fact validity |

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| Dream Pipeline only (no real-time) | Facts layer provides real-time retrieval; wiki's value is cross-referencing which benefits from batch context; avoids routing complexity (R8) |
| Entity-driven page organization | Graph nodes provide natural page boundaries; avoids topic overlap; aligns with Karpathy's "entity pages" concept |
| Fill WikiIngestStage (not new stage) | Stub already exists with config and should_run; follows P6 simplicity |
| Graph node matching for create/update | memory_entities table already links facts to nodes; zero new infrastructure |
| Skip when no LLM provider | Wiki value is synthesis quality; template concatenation is noise |
| Stale detection by source validity ratio | Balanced — doesn't cascade-invalidate aggressively, but ensures wiki reflects current knowledge |
