# Memory Auto-Injection Design

> Automatically inject relevant LanceDB memories into LLM prompts via a new PromptPipeline layer.

**Date**: 2026-03-07
**Status**: Approved

---

## Problem

LanceDB stores both Layer 1 memories (raw conversations) and Layer 2 facts (compressed knowledge), but neither is automatically injected into LLM prompts. Agents must explicitly call the `memory_search` tool to access past context. This means agents often operate without relevant historical context unless they "remember" to search.

## Approach

New `MemoryAugmentationLayer` in the PromptPipeline (priority 1600, after WorkspaceFilesLayer at 1550). Every LLM call triggers a vector search against LanceDB using the user's latest message as query, injecting relevant results into the system prompt.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Reuse existing modules | No — implement directly in Layer | `PromptAugmenter` and `MemoryRetrieval` APIs don't match Layer trait; logic is simple enough to inline |
| Search scope | Both Layer 1 + Layer 2 | Facts are more refined but sparse; memories provide fallback coverage |
| Priority ordering | Facts first, then memories | Facts are higher-quality distilled knowledge |
| Token budget | 2000 tokens (~8000 chars) | Enough for ~10 results without dominating context window |
| Failure mode | warn + skip | Memory injection is enhancement, never blocks LLM calls |

## Data Flow

```
LLM Call (every call)
    |
    +- PromptPipeline.apply(context)
    |     |
    |     +- ... (earlier layers: Core, Identity, WorkspaceFiles)
    |     |
    |     +- MemoryAugmentationLayer.apply(context)  [priority 1600]
    |           |
    |           1. Extract user's latest message as query
    |           2. Generate query embedding via EmbeddingProvider
    |           3. tokio::join!(search facts, search memories)
    |           4. Merge results: facts first, then memories, dedup
    |           5. Truncate to token_budget (2000 tokens)
    |           6. Format as "## Relevant Memory" section
    |           7. Append to context.system_prompt
    |
    +- Thinker sends augmented prompt to LLM
```

## Code Changes

### File 1: `core/src/thinker/layers/memory_augmentation.rs` (NEW)

```rust
pub struct MemoryAugmentationLayer {
    memory_db: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: MemoryAugmentationConfig,
}

pub struct MemoryAugmentationConfig {
    pub enabled: bool,
    pub max_facts: usize,        // default 5
    pub max_memories: usize,     // default 5
    pub similarity_threshold: f32, // default 0.3
    pub token_budget: usize,     // default 2000
}
```

Implements `Layer` trait with `apply()` that:
1. Extracts user message from context
2. Generates embedding
3. Searches facts (limit 5) + memories (limit 5) in parallel
4. Merges, deduplicates, formats
5. Appends to system prompt

### File 2: `core/src/thinker/layers/mod.rs`

Register `MemoryAugmentationLayer` in module exports.

### File 3: `core/src/bin/aleph/commands/start/mod.rs`

Create `MemoryAugmentationLayer` when embedder is available, inject into PromptPipeline.

### File 4: `core/src/bin/aleph/commands/start/builder/handlers.rs`

Add `init_memory_augmentation_layer()` helper function.

## Retrieval Strategy

- **Facts table**: Vector search, cosine similarity >= 0.3, limit 5
- **Memories table**: Vector search, cosine similarity >= 0.3, limit 5
- **Merge**: Facts first (higher quality), then memories, deduplicate by content overlap
- **Total output**: Max 10 items, truncated to 2000 token budget

**Output format:**

```
## Relevant Memory

**Facts:**
- User prefers dark mode for all applications
- Project uses Rust + Tokio async runtime

**Past Conversations:**
- [2026-03-05] Q: How to configure embedding? A: Use aleph.toml...
```

## Degradation

| Scenario | Behavior |
|----------|----------|
| No embedding provider | Layer not registered, pipeline runs normally |
| Embedding generation fails | warn log, skip injection, don't block LLM call |
| LanceDB query fails | warn log, skip injection |
| Empty results | No injection (no empty "## Relevant Memory" section) |
| Token over budget | Truncate, prioritize facts over memories |
| Empty/non-text user message | Skip memory search |

## Dependency Injection Path

```
run_server()
  → register_agent_handlers(embedder, memory_db)
    → create MemoryAugmentationLayer(memory_db, embedder, config)
    → inject into PromptPipeline via pipeline.add_layer(layer)
    → Thinker holds pipeline, every LLM call triggers all layers
```

## Not In Scope

- MEMORY.md → LanceDB one-way sync (future phase)
- LanceDB workspace field populated with real workspace_id (future)
- Per-agent memory isolation beyond current workspace filtering
- Configurable token budget via aleph.toml (hardcoded for now)
- Cache/memoization of embedding queries

## Files Affected

| File | Change |
|------|--------|
| `core/src/thinker/layers/memory_augmentation.rs` | NEW — Layer implementation |
| `core/src/thinker/layers/mod.rs` | Register new layer |
| `core/src/bin/aleph/commands/start/mod.rs` | Create and inject layer |
| `core/src/bin/aleph/commands/start/builder/handlers.rs` | Init helper function |
