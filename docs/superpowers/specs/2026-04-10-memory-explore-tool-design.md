# Memory Explore Tool — RippleTask Wiring

> Expose RippleTask as `memory_explore` builtin tool for multi-hop knowledge exploration.

**Date**: 2026-04-10
**Scope**: New `src/builtin_tools/memory_explore.rs`, registry wiring
**Prerequisite**: Memory Logic Chain Fix (completed)

---

## Problem

RippleTask (`src/memory/ripple/`) is fully implemented with BFS multi-hop vector similarity exploration and tunnel traversal, but never instantiated outside tests. No production code uses it.

## Solution

Register RippleTask as a builtin tool `memory_explore`. LLM decides when to invoke it for deep knowledge exploration (per R9: Everything is a Tool).

## Tool Schema

```json
{
  "name": "memory_explore",
  "description": "Explore related knowledge by following semantic connections from a starting query. Use when you need deeper context about a topic — discovers related facts across multiple hops of similarity.",
  "parameters": {
    "query": { "type": "string", "description": "Starting query to explore from" },
    "max_hops": { "type": "integer", "description": "Maximum exploration depth (default: 2, max: 4)", "default": 2 },
    "max_per_hop": { "type": "integer", "description": "Facts to discover per hop (default: 5, max: 10)", "default": 5 }
  },
  "required": ["query"]
}
```

## Data Flow

```
LLM calls memory_explore(query, max_hops, max_per_hop)
  → embed query
  → vector_search for seed facts (top 3)
  → load_embedding_for_fact for each seed
  → RippleTask::explore(seed_facts)
    → BFS: for each hop, vector_search from current level's embeddings
    → filter by similarity_threshold (0.7)
    → deduplicate by fact ID
  → return seed_facts + expanded_facts (sorted by relevance)
```

## Changes

### 1. New File: `src/builtin_tools/memory_explore.rs`

Struct `MemoryExploreTool` holds:
- `database: MemoryBackend`
- `embedder: Arc<dyn EmbeddingProvider>`

Implements `AlephTool` trait:
- `name()` → `"memory_explore"`
- `description()` → tool description for LLM
- `parameters_schema()` → JSON schema with query, max_hops, max_per_hop
- `execute()`:
  1. Parse parameters (query, optional max_hops/max_per_hop with defaults and caps)
  2. Embed query via `embedder.embed(query)`
  3. Vector search for seed facts (top 3, valid only)
  4. Call `database.load_embedding_for_fact()` for each seed
  5. Create `RippleTask` with `RippleConfig { max_hops, max_facts_per_hop, similarity_threshold: 0.7 }`
  6. Call `ripple.explore(seed_facts).await`
  7. Format results: combine seed + expanded facts, deduplicate, sort by score
  8. Return formatted text result

### 2. Modify: `src/builtin_tools/mod.rs`

Add `pub mod memory_explore;` and re-export `MemoryExploreTool`.

### 3. Register in Builder

Add `memory_explore` to the builtin tool registry (same file/pattern as `memory_search`).

## Error Handling

| Failure | Behavior |
|---------|----------|
| Embedding fails | Return error "Failed to embed query" |
| No seed facts found | Return "No related knowledge found for this query" |
| load_embedding_for_fact fails for a seed | Skip that seed, continue with others |
| RippleTask explore returns empty | Return only seed facts |

## Output Format

```
## Knowledge Exploration Results

### Direct matches (seed facts):
- [fact content] (relevance: 0.85)

### Related discoveries (hop 1-2):
- [expanded fact content] (relevance: 0.72, discovered via: [seed fact summary])

Explored 3 seed facts across 2 hops, discovered 7 related facts.
```

## Out of Scope

- Tunnel traversal via `explore_tunnels()` (requires graph edges that nothing creates yet — same issue as TunnelDiscoveryStage)
- Configurable similarity_threshold via tool parameter (use default 0.7)
- Integration into automatic retrieval (this is an on-demand tool only)
