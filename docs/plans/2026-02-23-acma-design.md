# Aleph Cognitive Memory Architecture (ACMA) — Design Document

> "From Database to Soul: A Human-Like Memory Evolution"

**Status**: Approved
**Date**: 2026-02-23
**Approach**: Incremental Evolution (Option A) — extend existing MemoryFact model

---

## 1. Overview

ACMA introduces three orthogonal dimensions to Aleph's memory system:

| Dimension | Field | Values | Purpose |
|-----------|-------|--------|---------|
| **Abstraction** | `layer` | L0 / L1 / L2 | Granularity (already being implemented) |
| **Temperature** | `tier` | Core / ShortTerm / LongTerm | Temporal lifecycle |
| **Visibility** | `scope` | Global / Workspace / Persona | Access isolation |

These dimensions are independent — a fact can be any combination (e.g., `Core + L0 + Persona`, `LongTerm + L2 + Global`).

Working Memory is the current session's message list in RAM. It is not stored in LanceDB.

---

## 2. Data Model Extension

### 2.1 New Enums

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    /// Always online. Injected into system prompt at session start.
    Core,
    /// Recent high-fidelity data. Default for new facts. Decays over ~7 days.
    ShortTerm,
    /// Consolidated semantic knowledge. Persists indefinitely.
    LongTerm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Shared across all personas and workspaces.
    Global,
    /// Shared across all personas within one workspace.
    Workspace,
    /// Private to a specific persona.
    Persona,
}
```

### 2.2 MemoryFact New Fields

```rust
pub struct MemoryFact {
    // ... existing 21+ fields (id, content, fact_type, embedding, namespace,
    //     workspace, path, parent_path, confidence, is_valid, etc.) ...
    // ... layer + category (being implemented) ...

    // ACMA additions:
    pub tier: MemoryTier,              // default: ShortTerm
    pub scope: MemoryScope,            // default: Global
    pub persona_id: Option<String>,    // only set when scope = Persona

    // Forgetting curve support:
    pub strength: f32,                 // 0.0-1.0, default: 1.0
    pub access_count: u32,             // reinforcement factor, default: 0
    pub last_accessed_at: Option<i64>, // unix timestamp of last retrieval hit
}
```

### 2.3 Backward Compatibility

Existing facts receive these defaults during migration:

| Field | Default | Rationale |
|-------|---------|-----------|
| `tier` | `ShortTerm` | Conservative — not promoted to LTM until consolidated |
| `scope` | `Global` | All existing facts remain universally visible |
| `persona_id` | `None` | No persona isolation until explicitly assigned |
| `strength` | `1.0` | Existing facts start at full strength |
| `access_count` | `0` | No access history |
| `last_accessed_at` | `None` | No access history |

---

## 3. Context Composition

### 3.1 ContextComposer

A new component that assembles memory context at session start:

```rust
pub struct ContextComposer {
    store: MemoryBackend,
    embedder: Arc<SmartEmbedder>,
}

pub struct CompositionRequest {
    pub persona_id: Option<String>,
    pub workspace: String,
    pub namespace: String,
    pub token_budget: usize,
}

pub struct ComposedContext {
    pub core_facts: Vec<MemoryFact>,      // always injected
    pub relevant_facts: Vec<MemoryFact>,  // ranked by relevance
    pub total_tokens: usize,
}
```

### 3.2 Composition Logic (Four-Layer Union)

```
compose(persona=P, workspace=W):
  1. Core(Persona=P)    → tier=Core, scope=Persona, persona_id=P
  2. Core(Global)       → tier=Core, scope=Global
  3. Query(Workspace=W) → scope=Workspace, workspace=W, tier≠Core
  4. Query(Persona=P)   → scope=Persona, persona_id=P, tier≠Core
  5. Query(Global)      → scope=Global, tier≠Core

  Deduplicate → rank by relevance → trim to token budget
```

### 3.3 Injection Points

| Layer | Injection | Timing |
|-------|-----------|--------|
| Core facts | Appended to system prompt | Every request |
| Relevant facts | Via existing `<relevant_memories>` tag | Per-request retrieval |

### 3.4 Integration

- Replaces the query logic inside `ContextAugmenter.augment()` with scope-stack filtering
- `ContextComptroller` still handles deduplication and token budget downstream
- Core facts bypass relevance ranking — always injected

---

## 4. Forgetting Curve & Strength

### 4.1 Strength Update (batch, in DreamDaemon)

```rust
pub fn update_strength(fact: &mut MemoryFact, now: i64, half_life_days: f64) {
    let age_days = (now - fact.created_at) as f64 / 86400.0;
    let last_access_days = match fact.last_accessed_at {
        Some(ts) => (now - ts) as f64 / 86400.0,
        None => age_days,
    };

    // Ebbinghaus exponential decay based on last access
    let base = (-last_access_days / half_life_days).exp();

    // Logarithmic access boost (spaced repetition effect)
    let access_boost = (fact.access_count as f64).ln_1p() * 0.15;

    fact.strength = (base as f32 + access_boost).clamp(0.0, 1.0);
}
```

### 4.2 Access Tracking (on retrieval hit)

```rust
pub fn on_access(fact: &mut MemoryFact, now: i64) {
    fact.access_count += 1;
    fact.last_accessed_at = Some(now);
}
```

### 4.3 Relationship to Existing decay.rs

- Existing `calculate_decay()` remains for non-persistent runtime scoring
- New `update_strength()` is called by DreamDaemon, results persisted to `strength` field
- Existing `cleanup_decayed_facts()` changes to check `strength < pruning_threshold`

---

## 5. STM → LTM Consolidation (DreamDaemon Extension)

### 5.1 Extended Dream Pipeline

```
run_dream():
  Phase 1: Graph Decay             ← existing
  Phase 2: Memory Strength Update  ← modified (uses update_strength)
  Phase 3: L0/L1 Generation        ← existing
  Phase 4: Consolidation           ← NEW
  Phase 5: Pruning                 ← NEW
```

### 5.2 Consolidation Logic

```
Scan STM facts where strength ≥ consolidation_threshold (default 0.6):
  │
  ├─ Cluster by topic (embedding similarity)
  │
  ├─ For each cluster:
  │   → LLM distills Episodic → Semantic
  │   → New fact: tier=LongTerm, fact_source=Summary
  │   → Original facts: is_valid=false
  │
  └─ Example:
      Input (STM, L2):
        "User asked to use Arc<Mutex<T>> for shared state on 2/20"
        "User again mentioned Arc<Mutex<T>> pattern on 2/21"
      Output (LTM, L2):
        "Project Aleph uses Arc<Mutex<T>> as standard shared state pattern"
        scope=Workspace, tier=LongTerm, confidence=0.9
```

### 5.3 Pruning Logic

```
Scan STM facts where strength < pruning_threshold (default 0.1):
  → Delete permanently (not soft-delete)
```

### 5.4 Configuration

```toml
[memory.consolidation_pipeline]
enabled = true
strength_threshold = 0.6     # STM fact minimum strength for consolidation
pruning_threshold = 0.1      # below this, delete
max_facts_per_run = 50       # batch size per Dream cycle
cooldown_days = 1             # minimum interval between checks for same fact
```

---

## 6. Persona System

### 6.1 Design Principle

Persona is NOT a first-class entity. It is an implicit concept defined by the presence of `persona_id` in Core Memory facts:

```
persona_id = "code-reviewer" is defined by:
  MemoryFact { tier: Core, scope: Persona, persona_id: Some("code-reviewer"),
               content: "You are a meticulous senior code reviewer..." }
  MemoryFact { tier: Core, scope: Persona, persona_id: Some("code-reviewer"),
               content: "Always check for OWASP Top 10 vulnerabilities" }
```

No Persona table, no Persona CRUD API, no extra aggregate root.

### 6.2 SearchFilter Extension

```rust
pub struct SearchFilter {
    // ... existing fields ...

    // ACMA additions:
    pub tier: Option<MemoryTier>,
    pub scope: Option<MemoryScope>,
    pub persona_id: Option<String>,
}

impl SearchFilter {
    /// Build scope-stack filter for current session context.
    /// Matches: Global OR (Workspace=W) OR (Persona=P)
    pub fn with_scope_stack(
        self,
        persona_id: Option<&str>,
        workspace: &str,
    ) -> Self { ... }

    pub fn with_tier(self, tier: MemoryTier) -> Self { ... }
    pub fn with_scope(self, scope: MemoryScope) -> Self { ... }
    pub fn with_persona_id(self, id: &str) -> Self { ... }
}
```

### 6.3 Cross-Persona Interaction

**Explicit Publishing (Scope Promotion):**

```rust
pub async fn promote_fact(
    store: &MemoryBackend,
    fact_id: &str,
    new_scope: MemoryScope,
) -> Result<()> {
    // Update scope field, clear persona_id if promoting to Global/Workspace
}
```

**Cross-Persona Query:**

```rust
let filter = SearchFilter::new()
    .with_scope(MemoryScope::Persona)
    .with_persona_id("code-reviewer")
    .with_workspace("aleph");

let insights = store.hybrid_search(embedding, query, filter).await?;
```

### 6.4 Visibility Matrix

| Querying as | Global | Workspace(W) | Own Persona | Other Persona |
|-------------|--------|--------------|-------------|---------------|
| Default retrieval | Yes | Yes | Yes | No |
| Explicit cross-query | Yes | Yes | Yes | Yes |
| Core injection | Yes | No | Yes | No |

---

## 7. Integration Architecture

```
Session Start
  │
  ▼
┌─────────────────────────────────────────────────┐
│ ContextComposer                                  │
│  1. Query Core facts (tier=Core, scope stack)    │
│  2. Inject into system prompt                    │
│  3. Retrieve STM/LTM facts (scope stack)         │
│  4. ContextComptroller dedup + token budget       │
└──────────────────────┬──────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────┐
│ Agent Loop (unchanged)                           │
│  Observe → Think → Act → Feedback                │
│                                                   │
│  Retrieval: memory_search + scope stack filter    │
│  Ingestion: new facts → tier=STM + current scope  │
│  Access:    on_access() updates strength factors   │
└──────────────────────┬──────────────────────────┘
                       ▼
Session End / Idle
  │
  ▼
┌─────────────────────────────────────────────────┐
│ DreamDaemon (extended)                           │
│  Phase 1: Graph Decay              ← existing    │
│  Phase 2: Memory Strength Update   ← modified    │
│  Phase 3: L0/L1 Generation         ← existing    │
│  Phase 4: Consolidation            ← NEW         │
│     STM(high strength) → LLM distill → LTM       │
│  Phase 5: Pruning                  ← NEW         │
│     STM(low strength) → delete                    │
└─────────────────────────────────────────────────┘
```

---

## 8. Implementation Tasks

Prerequisite: Complete the existing `2026-02-23-memory-architecture-implementation.md` (Tasks 1-7: layer + category).

### Task Dependency Graph

```
A → B → C → D ─┐
          ↘ E → F ─→ G
```

| Task | Description | Dependencies | Key Files |
|------|------------|--------------|-----------|
| **A** | Add `MemoryTier`, `MemoryScope` enums + MemoryFact fields | None | `context.rs` |
| **B** | Persist tier/scope/persona_id/strength in LanceDB | A | `schema.rs`, `arrow_convert.rs` |
| **C** | Extend SearchFilter + scope-stack query | B | `store/types.rs`, `lance/facts.rs` |
| **D** | Implement ContextComposer | C | new `memory/composer.rs` |
| **E** | Persist strength + on_access() updates | B | `decay.rs`, `fact_retrieval.rs` |
| **F** | DreamDaemon consolidation + pruning phases | C, E | `dreaming.rs` |
| **G** | Integration tests + documentation update | D, F | `integration_tests/`, `MEMORY_SYSTEM.md` |

---

## 9. Excluded (YAGNI)

| Excluded | Reason |
|----------|--------|
| Persona CRUD API / management UI | Persona defined implicitly by Core Memory facts |
| Physical partitioning (separate STM/LTM tables) | Single table + tier field sufficient; optimize later if needed |
| Working Memory persistence | Just the session message list in RAM |
| Cross-persona messaging protocol | Direct memory query is sufficient |
| Real-time strength recalculation | Batch update in DreamDaemon; not per-query |
| Event sourcing for tier transitions | Direct field update is simpler and sufficient |

---

## 10. Configuration Summary

```toml
# ACMA additions to [memory] section

[memory.consolidation_pipeline]
enabled = true
strength_threshold = 0.6
pruning_threshold = 0.1
max_facts_per_run = 50
cooldown_days = 1

[memory.memory_decay]
# Existing config, strength field replaces runtime-only decay
half_life_days = 30.0
access_boost = 0.15
min_strength = 0.1
protected_types = ["personal"]
protected_tiers = ["core"]    # Core tier facts never decay
```
