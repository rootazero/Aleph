# Graph-Augmented Memory Architecture

> Elevate the knowledge graph from a disconnected appendage to the connective tissue
> of Aleph's entire memory system.

## Problem Statement

Aleph's memory system has evolved into a sophisticated multi-dimensional knowledge
store with 15 FactTypes, 8 enum dimensions, tiered loading, cognitive decay, and
dreaming synthesis. However, the knowledge graph (`graph_nodes`, `graph_edges`) is
disconnected from the fact store:

1. **`link_memory_entity()` is a TODO** — no fact ↔ graph node association exists
2. **Graph does not participate in retrieval** — `hybrid_retrieval` uses only
   vector + full-text search; graph is invisible to the query path
3. **Wiki wikilinks and graph edges are isolated** — `[[wikilink]]` in wiki
   content has no relationship to graph edges
4. **No structural discovery** — related facts can only be found via semantic
   similarity (vector space), not via structural knowledge relationships

## Design Goals

- Complete the fact ↔ graph node bidirectional index (`memory_entities`)
- Integrate graph expansion into the retrieval pipeline as an enrichment layer
- Synchronize wiki wikilinks to graph edges (wiki-specific, not other FactTypes)
- Maintain R8 (LLM Sovereignty) — graph enhances, LLM decides
- Preserve existing vector + full-text retrieval as the primary path

## Non-Goals

- Graph-first retrieval (would violate R8 by hardcoding retrieval routing)
- Graph-centric rewrite of the memory module
- Panel graph visualization (deferred to a future phase)
- Renaming `Fact` / `MemoryFact` (keep existing names, update documentation)

## Fact Naming Clarification

The term "Fact" (`MemoryFact`) is retained as Aleph's domain-specific term for the
universal unit of persisted knowledge. It is not limited to factual statements —
it encompasses preferences, wiki pages, skills, transcripts, synthesized insights,
and agent experiences. The `FactType` enum already makes this clear through its
variants. A documentation update in `src/memory/context/enums.rs` will formalize
this definition.

---

## Architecture Overview

```
                    Write Path
                    ─────────
Conversation → CompressionService.compress()
    → FactExtractor.extract_unified()
        → facts[] + entities[] + relationships[]
    → Store facts to facts table
    → Upsert entities/relationships to graph
    → [NEW] Step 4y: link fact ↔ graph node via memory_entities
    → [NEW] Wiki-only: sync_wikilinks_to_graph()

                    Read Path
                    ─────────
Query → Embedding → hybrid_retrieval (vector + FTS) → candidate facts
                                                          ↓
                                                  [NEW] GraphExpander
                                                  (entity lookup → edge
                                                   traversal → reverse
                                                   fact lookup)
                                                          ↓
                                                  Merge + dedup + rerank
                                                          ↓
                                                  EnrichedRetrievalResult
                                                  {direct_hits,
                                                   association_clusters,
                                                   graph_expanded}
                                                          ↓
                                                        LLM
```

---

## Section 1: Storage Layer — `memory_entities` Table

### Schema

```sql
CREATE TABLE IF NOT EXISTS memory_entities (
    id          TEXT PRIMARY KEY,
    fact_id     TEXT NOT NULL,
    node_id     TEXT NOT NULL,
    weight      REAL NOT NULL DEFAULT 1.0,
    source      TEXT NOT NULL DEFAULT 'extracted',
    created_at  INTEGER NOT NULL,
    agent       TEXT,

    UNIQUE(fact_id, node_id)
);

CREATE INDEX IF NOT EXISTS idx_me_fact_id ON memory_entities(fact_id);
CREATE INDEX IF NOT EXISTS idx_me_node_id ON memory_entities(node_id);
CREATE INDEX IF NOT EXISTS idx_me_agent   ON memory_entities(agent);
```

### Design Decisions

- **`source` field** distinguishes association origins: `extracted` (LLM extraction),
  `wikilink` (wiki content parsing), `manual` (user-created). Different sources
  can be treated differently during decay and conflict resolution.
- **`weight`** expresses association strength (0.0–1.0). LLM-extracted associations
  default to 0.8; explicit wikilinks default to 1.0.
- **`UNIQUE(fact_id, node_id)`** prevents duplicate associations. Repeated writes
  upsert the weight.
- **No foreign keys** — consistent with existing `graph_nodes`/`graph_edges` style.
  Application-layer logic ensures consistency. Cascade deletion during decay cleanup.

### GraphStore Trait Extensions

```rust
async fn link_memory_entity(
    &self, fact_id: &str, node_id: &str, weight: f32,
    source: &str, workspace: &str,
) -> Result<(), AlephError>;

async fn get_nodes_for_fact(
    &self, fact_id: &str, workspace: &str,
) -> Result<Vec<(GraphNode, f32)>, AlephError>;

async fn get_facts_for_node(
    &self, node_id: &str, workspace: &str,
) -> Result<Vec<(String, f32)>, AlephError>;

async fn unlink_memory_entity(
    &self, fact_id: &str, node_id: &str, workspace: &str,
) -> Result<(), AlephError>;
```

---

## Section 2: Write Layer — Automatic Association Building

### Path 1: CompressionService (All FactTypes)

After step 4x (graph entity/relationship upsert), add step 4y:

```rust
// 4y. Build fact ↔ node associations
for stored_fact in &stored_facts {
    let entity_names = GraphStore::extract_entities_from_text(&stored_fact.content);
    for name in &entity_names {
        if let Ok(resolved) = graph_store.resolve_entity(name, None).await {
            if let Some(best) = resolved.first() {
                let _ = graph_store.link_memory_entity(
                    &stored_fact.id, &best.node_id, 0.8,
                    "extracted", "default",
                ).await;
            }
        }
    }
}
```

Reuses existing `extract_entities_from_text()` and `resolve_entity()`. Weight 0.8
for implicit LLM-extracted associations (lower than explicit wikilink 1.0).

### Path 2: Wiki Wikilink Sync (FactType::Wiki Only)

```rust
pub async fn sync_wikilinks_to_graph(
    &self,
    fact: &MemoryFact,
    graph_store: &GraphStore,
) -> Result<(), AlephError> {
    if fact.fact_type != FactType::Wiki {
        return Ok(());
    }

    let wikilinks = extract_wikilinks(&fact.content);

    for link_target in &wikilinks {
        // 1. Ensure target entity exists (create as wiki kind if missing)
        let target_node = graph_store
            .upsert_node(link_target, "wiki", &[], None).await?;

        // 2. Link fact ↔ target node (source=wikilink)
        graph_store.link_memory_entity(
            &fact.id, &target_node.id, 1.0, "wikilink", "default",
        ).await?;

        // 3. Create references edge in graph (source page → target page)
        let source_slug = fact.path.split('/').last()
            .unwrap_or(&fact.id);
        let source_node = graph_store
            .upsert_node(source_slug, "wiki", &[], None).await?;

        graph_store.upsert_edge(
            &source_node.id, &target_node.id,
            "references", "", 1.0, 1.0,
        ).await?;
    }

    Ok(())
}
```

### Trigger Points

| Scenario | Trigger |
|----------|---------|
| CompressionService extracts new fact | Path 1 auto-executes |
| Wiki fact created/updated | Path 2 auto-executes |
| WikiIngestStage (dream phase) | Path 2 after generating new wiki page |
| Manual fact creation | Path 1 auto-executes |

### Idempotency

- `UNIQUE(fact_id, node_id)` constraint ensures safe repeated calls
- `upsert_node` and `upsert_edge` are already idempotent
- On wiki fact update: clear all `source=wikilink` associations for that fact
  first (`DELETE FROM memory_entities WHERE fact_id = ? AND source = 'wikilink'`),
  then re-parse content and rebuild associations (handles removed wikilinks).
  `source=extracted` associations are NOT cleared — they are managed independently
  by the CompressionService path.

---

## Section 3: Retrieval Layer — Graph-Augmented Retrieval

### GraphExpander

```rust
pub struct GraphExpander {
    graph_store: GraphStore,
    config: GraphExpansionConfig,
}

pub struct GraphExpansionConfig {
    pub enabled: bool,
    pub max_hops: usize,                // default: 1
    pub max_expanded_per_seed: usize,   // default: 3
    pub max_total_expanded: usize,      // default: 10
    pub min_weight: f32,                // default: 0.3
}
```

### Expansion Flow (Single Hop)

```
For each candidate fact (seed):
  1. get_nodes_for_fact(seed.id) → associated graph nodes
  2. For each node:
     get_edges_for_node(node.id) → neighbor nodes
  3. For each neighbor:
     get_facts_for_node(neighbor.id) → associated facts
  4. Filter: exclude facts already in candidate set, exclude weight < min_weight
  5. Score: expanded_score = seed_score × edge_weight × link_weight × decay_factor
```

### Scoring Formula

```
expanded_fact_score = original_seed_score
                    × edge.weight          -- graph edge weight
                    × link.weight          -- fact-node association weight
                    × decay_factor         -- hop decay (0.7 for hop=1)
```

Expanded facts always score below direct hits. Graph expansion supplements,
never replaces.

### Integration with AssociationCluster

Not a replacement. They are complementary:
- `AssociationCluster` — vector-space semantic similarity ("sounds like")
- `GraphExpander` — structural knowledge relationships ("knowledge-related")

```rust
pub struct EnrichedRetrievalResult {
    /// Direct hits (vector + full-text)
    pub direct_hits: Vec<ScoredFact>,
    /// Vector-space association clusters
    pub association_clusters: Vec<AssociationCluster>,
    /// Graph-expanded related facts
    pub graph_expanded: Vec<ScoredFact>,
}
```

LLM receives all three categories and decides how to synthesize (R8 sovereignty).

### Performance Bounds

| Constraint | Value | Purpose |
|------------|-------|---------|
| `max_hops` | 1 | Prevent traversal explosion |
| `max_expanded_per_seed` | 3 | Cap per-seed expansion |
| `max_total_expanded` | 10 | Total expansion ceiling |
| `min_weight` | 0.3 | Filter weak associations |
| Hop decay factor | 0.7 | Ensure expanded < direct hits |

All configurable. Initial values are conservative; tunable based on observed behavior.

---

## Section 4: Maintenance Layer — Consistency & Wiki Lint

### 4.1 Decay Cascade

#### Fact Invalidation → Clean Associations

When `invalidate_fact()` is called, cascade-delete `memory_entities` records:

```rust
async fn invalidate_fact(&self, fact_id: &str) -> Result<(), AlephError> {
    // Existing: mark fact.is_valid = false
    // New: delete all memory_entities for this fact
    self.delete_memory_entities_for_fact(fact_id, workspace).await?;
    Ok(())
}
```

#### Graph Node Pruning → Clean Associations

In `apply_decay`, when a node is pruned (score below threshold):

```rust
// In the node pruning branch of apply_decay:
if new_score < policy.min_score {
    // Existing: delete cascade edges
    // New: delete cascade memory_entities
    conn.execute(
        "DELETE FROM memory_entities WHERE node_id = ?1 AND agent = ?2",
        params![node.id, workspace],
    )?;
    // Existing: delete node
}
```

### 4.2 Wiki Lint Enhancement — Suggested Links

Leverage graph topology to suggest missing wikilinks:

```rust
pub struct WikiLintReport {
    pub broken_links: Vec<(String, String)>,
    pub orphan_pages: Vec<String>,
    pub stale_pages: Vec<String>,
    pub suggested_pages: Vec<String>,
    pub suggested_links: Vec<SuggestedLink>,  // NEW
    pub auto_fixed: usize,
}

pub struct SuggestedLink {
    pub from_page: String,
    pub to_page: String,
    pub reason: String,       // graph relation type
    pub confidence: f32,
}
```

Logic: for each wiki fact, find its associated graph nodes → find neighbor nodes
with kind="wiki" → check if the wiki fact content contains a `[[wikilink]]` to
that neighbor → if not, add to `suggested_links`.

These suggestions are written to the dream report. The LLM decides whether to
adopt them during the next wiki maintenance pass (R8 sovereignty).

### 4.3 Fact Definition Update

Update module-level documentation in `src/memory/context/enums.rs`:

```rust
//! In Aleph's memory system, a "Fact" (`MemoryFact`) is the universal unit
//! of persisted knowledge — not limited to factual statements, but
//! encompassing preferences, wiki pages, skills, transcripts, synthesized
//! insights, and agent experiences. Each Fact is connected to the knowledge
//! graph via `memory_entities`, enabling structural retrieval across all
//! knowledge types.
```

---

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `src/memory/store/sqlite/schema.rs` | Modify | Add `memory_entities` DDL |
| `src/memory/store/mod.rs` | Modify | Extend `GraphStore` trait with 4 new methods |
| `src/memory/store/sqlite/graph.rs` | Modify | Implement new trait methods + decay cascade |
| `src/memory/graph.rs` | Modify | Implement `link_memory_entity()` (replace TODO) |
| `src/memory/compression/service.rs` | Modify | Add step 4y (fact ↔ node association) |
| `src/memory/wiki_sync.rs` | Create | `sync_wikilinks_to_graph()` logic |
| `src/memory/hybrid_retrieval/graph_expander.rs` | Create | `GraphExpander` implementation |
| `src/memory/hybrid_retrieval/mod.rs` | Modify | Wire `GraphExpander` into retrieval pipeline |
| `src/memory/dreaming/stages/wiki_lint.rs` | Modify | Add `SuggestedLink` and graph-based suggestions |
| `src/memory/context/enums.rs` | Modify | Update module doc comment |
| `src/memory/dreaming/stages/decay.rs` | Modify | Add `memory_entities` cascade in decay |

## Testing Strategy

- **Unit tests**: `memory_entities` CRUD, `GraphExpander` scoring, wikilink sync
- **Integration tests**: end-to-end compression → graph association → retrieval expansion
- **Property tests**: graph expansion never produces scores higher than direct hits

## Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| Graph traversal performance | Conservative defaults (1 hop, 10 max). All bounds configurable. |
| Entity resolution ambiguity | Reuse existing `resolve_entity()` with context scoring |
| Stale `memory_entities` records | Cascade deletion on both fact invalidation and node pruning |
| Wiki wikilink parsing errors | Reuse battle-tested `extract_wikilinks()` |
