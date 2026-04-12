# Memory Palace Evolution — Design Spec

> Enhance Aleph's long-term memory with structural navigation, temporal validity, and
> automatic cross-domain association. Inspired by the MemPalace project's spatial
> metaphor but fully integrated with Aleph's Rust architecture, SQLite+sqlite-vec
> storage, and DreamDaemon pipeline.

**Date**: 2026-04-09  
**Status**: Draft  
**Scope**: `src/memory/` — context, store, hybrid_retrieval, dreaming, ripple, ingestion

---

## 1. Motivation

### Problem

Aleph's memory system has rich classification (11 FactTypes, 6 dimensions) and strong
retrieval (vector + BM25 hybrid), but three gaps limit long-term memory quality:

1. **Flat retrieval** — All facts compete in a single search space. VFS paths exist but
   aren't used as search filters. MemPalace demonstrated 34% retrieval improvement by
   narrowing search scope via structural metadata (Wing/Room filtering).

2. **No temporal truth tracking** — The `strength` field models "how well remembered"
   (Ebbinghaus decay), but not "is this still true". When a fact is superseded, the old
   fact is soft-deleted instead of preserved as history. There's no way to ask "what did
   I know about X in January?"

3. **No cross-domain association** — Knowledge in `aleph://user/preferences/` and
   `aleph://knowledge/projects/` about the same topic (e.g., "Rust") exists in isolation.
   The knowledge graph has edges, but no automatic mechanism discovers that the same
   concept appears across domains.

### Approach

Three complementary enhancements, implemented in dependency order:

| Phase | Name | Core Idea |
|-------|------|-----------|
| 1 | Palace Topology | Derive `domain`/`topic` from VFS path; progressive search scope narrowing |
| 2 | Temporal Validity | `valid_from`/`valid_to` on MemoryFact; DriftDetect closes time windows |
| 3 | Associative Ripple | Write-time tunnel candidate flagging; DreamDaemon batch tunnel creation |

---

## 2. Phase 1 — Palace Topology

### 2.1 Concept

Extract two navigation coordinates from existing VFS paths:

```
aleph://user/preferences/coding
         ↑        ↑
       domain    topic
```

**Parsing rule**: `aleph://{domain}/{topic}/...`
- `domain`: First path segment (`user`, `knowledge`, `agent`)
- `topic`: Second path segment (`preferences`, `projects`, `tools`, `lessons`)
- Purely derived from `path` — no redundant storage, no new concepts

### 2.2 SQLite Implementation

Generated columns with a composite index:

```sql
ALTER TABLE facts ADD COLUMN domain TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%'
    THEN substr(path, 9, instr(substr(path, 9), '/') - 1)
    ELSE '' END
  ) STORED;

ALTER TABLE facts ADD COLUMN topic TEXT
  GENERATED ALWAYS AS (
    CASE WHEN path LIKE 'aleph://%/%/%'
    THEN substr(path, 9 + instr(substr(path, 9), '/'),
         instr(substr(path, 9 + instr(substr(path, 9), '/')), '/') - 1)
    ELSE '' END
  ) STORED;

CREATE INDEX idx_facts_domain_topic ON facts(domain, topic);
```

STORED (not VIRTUAL) — index is usable, zero runtime computation.

### 2.3 Progressive Search Strategy

Three-level scope narrowing in `HybridRetrieval`:

```rust
pub enum SearchScope {
    /// Same topic within same domain
    TopicLocal { domain: String, topic: String },
    /// Same domain, all topics
    DomainWide { domain: String },
    /// Full corpus
    Global,
}
```

**Algorithm**:
1. Infer `domain`/`topic` from query context: take the most frequent domain/topic pair
   among the last N facts retrieved or injected in the current session. If no prior
   context exists (cold start), skip directly to `Global`.
2. Search `TopicLocal` first — if result count ≥ `min_results` (default: 3) and top
   score ≥ similarity threshold, return
3. Expand to `DomainWide`, merge results
4. If still insufficient, fall back to `Global`

### 2.4 SearchFilter Changes

```rust
pub struct SearchFilter {
    // ... all existing fields preserved ...

    /// Restrict to a specific domain (derived from VFS path).
    pub domain: Option<String>,
    /// Restrict to a specific topic (derived from VFS path).
    pub topic: Option<String>,
}
```

`to_lance_filter()` adds corresponding WHERE clauses. The existing `path_prefix` filter
is fully preserved — domain/topic are additional, not replacement, filters.

### 2.5 Configuration

```toml
[memory.progressive_search]
enabled = true
min_results = 3           # Minimum results before expanding scope
topic_boost = 0.1         # Score bonus for same-topic results
domain_boost = 0.05       # Score bonus for same-domain results
```

### 2.6 Invariants

- Existing `path_prefix` filtering untouched — full backward compatibility
- Facts with empty/malformed paths get `domain = ""`, `topic = ""` → always Global scope
- Generated columns auto-populate for all existing data — zero migration cost
- No new Rust types needed for domain/topic (plain String matching)

---

## 3. Phase 2 — Temporal Validity

### 3.1 Concept

Two orthogonal dimensions for fact lifecycle:

| Dimension | Question | Mechanism | Affects |
|-----------|----------|-----------|---------|
| `strength` (existing) | "Do I remember this?" | Ebbinghaus decay curve | Retrieval ranking |
| `valid_from`/`valid_to` (new) | "Is this still true?" | Deterministic time window | Retrieval filtering |

### 3.2 MemoryFact Changes

```rust
pub struct MemoryFact {
    // ... all existing fields preserved ...

    /// When this fact became true (None = since creation)
    pub valid_from: Option<i64>,
    /// When this fact stopped being true (None = still valid)
    pub valid_to: Option<i64>,
}
```

**State matrix**:

| `valid_from` | `valid_to` | Meaning |
|:---:|:---:|---|
| `None` | `None` | Always valid (default for all existing facts) |
| `Some(t1)` | `None` | Valid since t1, still current |
| `Some(t1)` | `Some(t2)` | Historical: was true during [t1, t2] |
| `None` | `Some(t2)` | Was valid since creation, ended at t2 |

### 3.3 SQLite Schema

```sql
ALTER TABLE facts ADD COLUMN valid_from INTEGER DEFAULT NULL;
ALTER TABLE facts ADD COLUMN valid_to INTEGER DEFAULT NULL;

-- Partial index: fast filtering for "currently valid" facts
CREATE INDEX idx_facts_validity ON facts(valid_to) WHERE valid_to IS NULL;
```

### 3.4 SearchFilter Changes

```rust
pub struct SearchFilter {
    // ... all existing fields preserved ...

    /// Query facts valid at this point in time (Unix seconds).
    /// None = only currently-valid facts (default behavior).
    pub as_of: Option<i64>,

    /// Include historically-valid facts (valid_to IS NOT NULL).
    /// Default: false.
    pub include_historical: bool,
}
```

**Filter logic in `to_lance_filter()`**:
- Default (`as_of = None, include_historical = false`): adds `valid_to IS NULL`
  — identical to current behavior, zero breakage
- `as_of = Some(t)`: adds `(valid_from IS NULL OR valid_from <= t) AND (valid_to IS NULL OR valid_to >= t)`
- `include_historical = true`: no validity filter applied

### 3.5 DriftDetectStage Enhancement

Current behavior when contradiction detected:

```
DriftAction::Supersede { old_id, new_id }
→ old fact: is_valid = false (soft delete)
```

Enhanced behavior:

```
DriftAction::Supersede { old_id, new_id }
→ old fact: valid_to = now (time window closed, remains in DB)
→ new fact: valid_from = now
→ old fact: is_valid remains true (it WAS true, just not anymore)
```

The old fact becomes a historical record. `as_of` queries can retrieve it.

### 3.6 Interaction with TemporalScope

`TemporalScope` (Permanent/Contextual/Ephemeral) is an LLM classification label
expressing "how long is this fact expected to last". `valid_from`/`valid_to` express
"when was this fact actually true".

They interact during DriftDetect:
- `TemporalScope::Permanent` facts require higher contradiction confidence (≥ 0.8) to
  trigger supersede
- `TemporalScope::Ephemeral` facts: DreamDaemon can auto-set `valid_to` after their
  expected lifetime (e.g., 24h for "User wants to focus on docs today")

### 3.7 Builder Methods

```rust
impl MemoryFact {
    pub fn with_valid_from(mut self, ts: i64) -> Self {
        self.valid_from = Some(ts);
        self
    }

    pub fn with_valid_to(mut self, ts: i64) -> Self {
        self.valid_to = Some(ts);
        self
    }

    /// Close the validity window (mark as historical)
    pub fn close_validity(mut self) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        self.valid_to = Some(now);
        self
    }
}
```

### 3.8 Invariants

- All existing facts default to `valid_from = None, valid_to = None` → always valid
- Default search behavior unchanged (`valid_to IS NULL` filter)
- `is_valid` soft-delete mechanism fully preserved — orthogonal to validity
- `strength` decay fully preserved — orthogonal to validity
- Historical facts still count toward graph node/edge decay scoring

---

## 4. Phase 3 — Associative Ripple (Cross-Domain Tunnels)

### 4.1 Concept

When the same semantic topic appears across different domains, automatically discover and
create "tunnel" edges in the knowledge graph. This enables Ripple to traverse across
domain boundaries.

```
domain=user,      topic=preferences  →  "User prefers Rust for systems"
domain=knowledge, topic=projects     →  "Aleph uses Rust + axum"
                                              ↑
                                    Same semantic concept "Rust"
                                    → tunnel edge created
```

### 4.2 Write-Time Candidate Flagging

On fact ingestion (`ingestion.rs`), check if this fact's topic already has facts in
other domains:

```rust
fn check_tunnel_candidate(
    fact: &MemoryFact,
    store: &MemoryBackend,
) -> Result<bool, AlephError> {
    let (domain, topic) = parse_domain_topic(&fact.path);
    if domain.is_empty() || topic.is_empty() {
        return Ok(false);
    }
    let cross_domain_count = store
        .count_facts_by_topic_excluding_domain(&topic, &domain)?;
    Ok(cross_domain_count > 0)
}
```

Flag column on facts table:

```sql
ALTER TABLE facts ADD COLUMN tunnel_pending BOOLEAN DEFAULT FALSE;
```

This adds O(1) overhead to the write path: one indexed count query + one bool update.

### 4.3 TunnelDiscoveryStage

New DreamDaemon stage, runs after ConsolidateStage:

```rust
pub struct TunnelDiscoveryStage;

impl DreamStage for TunnelDiscoveryStage {
    fn name(&self) -> &str { "tunnel_discovery" }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Only run if there are pending tunnel candidates
        ctx.database.has_tunnel_pending().unwrap_or(false)
    }

    async fn run(&self, ctx: &mut DreamContext) -> Result<(), AlephError> {
        // 1. Collect tunnel_pending facts (bounded by config batch_size)
        let limit = ctx.config.tunnel_batch_size.unwrap_or(100);
        let candidates = ctx.database.get_tunnel_candidates(limit)?;

        // 2. Group by topic
        let by_topic: HashMap<String, Vec<MemoryFact>> = group_by_topic(candidates);

        // 3. For each topic group with facts in 2+ domains:
        for (topic, facts) in &by_topic {
            let domain_groups = group_by_domain(facts);
            if domain_groups.len() < 2 { continue; }

            // a. Pick representative fact per domain (highest strength)
            let reps: Vec<&MemoryFact> = domain_groups.values()
                .filter_map(|fs| fs.iter().max_by(|a, b|
                    a.strength.partial_cmp(&b.strength).unwrap()))
                .collect();

            // b. Pairwise embedding cosine similarity
            for pair in reps.windows(2) {
                let sim = cosine_similarity(
                    pair[0].embedding.as_deref(),
                    pair[1].embedding.as_deref(),
                );
                if sim >= ctx.config.tunnel_similarity_threshold {
                    // c. Create tunnel GraphEdge
                    ctx.graph_store.upsert_edge(GraphEdge {
                        relation: "tunnel".to_string(),
                        weight: sim,
                        context_key: topic.clone(),
                        ..Default::default()
                    })?;
                }
            }

            // d. Clear tunnel_pending flag
            ctx.database.clear_tunnel_pending(&topic)?;
        }
        Ok(())
    }
}
```

### 4.4 Pipeline Integration

```rust
impl DreamPipeline {
    pub fn daily() -> Self {
        Self::new()
            .stage(CollectStage)
            .stage(ClusterStage)
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(TunnelDiscoveryStage)  // NEW
            .stage(DecayStage)
    }
}
```

### 4.5 Ripple Tunnel Traversal

Enhance `RippleConfig` and `RippleTask`:

```rust
pub struct RippleConfig {
    // ... existing fields ...

    /// Enable cross-domain traversal via tunnel edges.
    pub enable_tunnels: bool,  // default: true

    /// Max tunnel hops per ripple (prevent combinatorial explosion).
    pub max_tunnel_hops: u32,  // default: 1
}
```

**Traversal priority during BFS**:
1. Direct edges within same domain (standard graph edges)
2. Tunnel edges across domains (relation = "tunnel", weight ≥ 0.6)
3. Weak associations (weight < 0.6)

Tunnel hops are counted separately from regular hops to avoid polluting the core
exploration depth.

### 4.6 Tunnel Lifecycle

Tunnel edges follow existing graph decay:
- `last_seen_at` updated each time retrieval traverses the tunnel
- `decay_score` decreases over time via `GraphDecayConfig`
- Pruned in `DecayStage` when below threshold
- No new decay mechanism needed

### 4.7 Configuration

```toml
[memory.tunnel_discovery]
enabled = true
similarity_threshold = 0.6    # Min embedding similarity for tunnel creation
max_tunnels_per_topic = 5     # Prevent hub topics from creating too many edges
batch_size = 100              # Max candidates per DreamDaemon run

[memory.ripple]
enable_tunnels = true
max_tunnel_hops = 1
```

### 4.8 Invariants

- Write path overhead: one count query + one bool column update (O(1))
- DreamDaemon is the only tunnel edge creator — no concurrent write conflicts
- Ripple tunnel traversal is opt-out (`enable_tunnels = false`)
- Existing graph edges unaffected — "tunnel" is a new relation type
- Tunnel edges participate in existing decay — no new lifecycle logic

---

## 5. Data Flow Summary

```
                    ┌─────────────────────────────────────────┐
                    │            Fact Ingestion                │
                    │                                         │
                    │  1. Extract fact from conversation      │
                    │  2. Assign VFS path → auto-derive       │
                    │     domain/topic (generated cols)        │
                    │  3. Check tunnel_candidate → flag        │
                    │  4. Store with valid_from = now          │
                    └──────────────┬──────────────────────────┘
                                   │
                    ┌──────────────▼──────────────────────────┐
                    │          Hybrid Retrieval                │
                    │                                         │
                    │  1. Infer domain/topic from context     │
                    │  2. Progressive: Topic → Domain → Global│
                    │  3. Filter by valid_to IS NULL (default)│
                    │  4. RRF fusion + optional rerank        │
                    │  5. Ripple expansion (incl. tunnels)    │
                    └──────────────┬──────────────────────────┘
                                   │
                    ┌──────────────▼──────────────────────────┐
                    │        DreamDaemon (Nightly)            │
                    │                                         │
                    │  Collect → Cluster → Summarize          │
                    │  → DriftDetect (close valid_to)         │
                    │  → Consolidate                          │
                    │  → TunnelDiscovery (NEW)                │
                    │  → Decay                                │
                    └─────────────────────────────────────────┘
```

---

## 6. Migration Strategy

All three phases use **additive schema changes only** — no data migration needed:

| Change | Type | Existing Data Impact |
|--------|------|---------------------|
| `domain`/`topic` generated columns | ALTER TABLE | Auto-computed from existing `path` |
| `valid_from`/`valid_to` columns | ALTER TABLE | Default NULL = always valid |
| `tunnel_pending` column | ALTER TABLE | Default FALSE |
| `idx_facts_domain_topic` index | CREATE INDEX | Built from generated columns |
| `idx_facts_validity` partial index | CREATE INDEX | Covers existing NULL valid_to |

Schema version bump: one migration file covering all three phases, applied at startup.

---

## 7. Testing Strategy

### Phase 1 Tests
- Unit: VFS path parsing → domain/topic extraction (edge cases: empty, malformed, short)
- Unit: SearchFilter with domain/topic → correct SQL generation
- Integration: Progressive search returns narrower-scope results first
- Property: For any valid VFS path, `domain` and `topic` are non-empty strings

### Phase 2 Tests
- Unit: `close_validity()` sets `valid_to` correctly
- Unit: SearchFilter `as_of` → correct temporal range SQL
- Integration: DriftDetect supersede → old fact gets `valid_to`, new gets `valid_from`
- Integration: Default search excludes historical facts; `include_historical` includes them
- Property: `valid_from <= valid_to` whenever both are set

### Phase 3 Tests
- Unit: `check_tunnel_candidate` returns true only when cross-domain facts exist
- Unit: Cosine similarity below threshold → no tunnel edge created
- Integration: DreamDaemon TunnelDiscoveryStage creates edges for cross-domain topics
- Integration: Ripple traverses tunnel edges and returns cross-domain facts
- Integration: Tunnel edges decay and get pruned like regular edges

---

## 8. Performance Considerations

| Operation | Current | After Change |
|-----------|---------|-------------|
| Fact write | ~1ms | ~1.2ms (+count query for tunnel check) |
| Fact search (typical) | Vector scan all facts | Vector scan within domain/topic subset (faster) |
| DreamDaemon daily | 6 stages | 7 stages (+TunnelDiscovery, bounded by batch_size) |
| Ripple BFS | N hops × M edges | Same + ≤1 tunnel hop (bounded by max_tunnel_hops) |

The progressive search strategy should **improve** search latency for most queries by
reducing the vector scan corpus size.

---

## 9. Non-Goals

- **No new UI** — These are backend memory improvements, transparent to the user
- **No MemPalace migration** — We don't import MemPalace data or use its formats
- **No emotional weighting** — MemPalace's AAAK dialect is interesting but doesn't fit
  Aleph's fact-centric model. Existing `confidence` + `strength` are sufficient
- **No verbatim storage mode** — MemPalace stores raw conversations. Aleph already has
  Layer 1 (raw) + Layer 2 (facts), which is a superset of this approach
- **No agent diaries** — Multi-agent memory isolation is already handled by `namespace`
  and `agent` fields
