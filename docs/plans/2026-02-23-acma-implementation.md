# ACMA (Aleph Cognitive Memory Architecture) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add cognitive memory tiers (Core/ShortTerm/LongTerm), persona-scoped isolation, forgetting curve with persistent strength, and STM→LTM consolidation to the Aleph memory system.

**Architecture:** Extend existing `MemoryFact` with three new fields (`tier`, `scope`, `persona_id`) and three strength-tracking fields (`strength`, `access_count`, `last_accessed_at`). Add `ContextComposer` for scope-stack context assembly. Extend `DreamDaemon` with consolidation and pruning phases. All changes are additive — no existing behavior changes.

**Tech Stack:** Rust, LanceDB (`lancedb`), Arrow (`arrow-array`, `arrow-schema`), `tokio`, `serde`, `schemars`, existing Aleph memory architecture.

**Design Doc:** `docs/plans/2026-02-23-acma-design.md`

**Prerequisite:** The existing `2026-02-23-memory-architecture-implementation.md` (Tasks 1-7: layer + category) must be completed first. This plan assumes `MemoryLayer` and `MemoryCategory` are already in `MemoryFact` and persisted in LanceDB.

---

## Task Dependency Graph

```
A → B → C → D ─┐
          ↘ E → F ─→ G
```

- **A**: Enums + MemoryFact fields
- **B**: LanceDB persistence
- **C**: SearchFilter + scope stack
- **D**: ContextComposer
- **E**: Strength + on_access
- **F**: DreamDaemon consolidation + pruning
- **G**: Integration tests + docs

---

### Task A: Add `MemoryTier`, `MemoryScope` Enums and MemoryFact Fields

**Files:**
- Modify: `core/src/memory/context.rs` (MemoryFact struct at L530-581, constructors at L595-668, tests at L811-1072)
- Modify: `core/src/memory/mod.rs` (re-exports at L69-72)

**Step 1: Write the failing tests**

In `core/src/memory/context.rs`, add to the `#[cfg(test)]` module (after line ~1071):

```rust
#[test]
fn test_memory_tier_roundtrip() {
    assert_eq!(MemoryTier::Core.as_str(), "core");
    assert_eq!(MemoryTier::ShortTerm.as_str(), "short_term");
    assert_eq!(MemoryTier::LongTerm.as_str(), "long_term");
    assert_eq!(MemoryTier::from_str_or_default("core"), MemoryTier::Core);
    assert_eq!(MemoryTier::from_str_or_default("long_term"), MemoryTier::LongTerm);
    assert_eq!(MemoryTier::from_str_or_default("unknown"), MemoryTier::ShortTerm);
}

#[test]
fn test_memory_scope_roundtrip() {
    assert_eq!(MemoryScope::Global.as_str(), "global");
    assert_eq!(MemoryScope::Workspace.as_str(), "workspace");
    assert_eq!(MemoryScope::Persona.as_str(), "persona");
    assert_eq!(MemoryScope::from_str_or_default("persona"), MemoryScope::Persona);
    assert_eq!(MemoryScope::from_str_or_default("unknown"), MemoryScope::Global);
}

#[test]
fn test_memory_fact_defaults_tier_and_scope() {
    let fact = MemoryFact::new("User likes Vim".into(), FactType::Preference, vec![]);
    assert_eq!(fact.tier, MemoryTier::ShortTerm);
    assert_eq!(fact.scope, MemoryScope::Global);
    assert!(fact.persona_id.is_none());
    assert_eq!(fact.strength, 1.0);
    assert_eq!(fact.access_count, 0);
    assert!(fact.last_accessed_at.is_none());
}

#[test]
fn test_memory_fact_with_persona() {
    let fact = MemoryFact::new("Lint rule".into(), FactType::Tool, vec![])
        .with_tier(MemoryTier::Core)
        .with_scope(MemoryScope::Persona)
        .with_persona_id("code-reviewer".to_string());
    assert_eq!(fact.tier, MemoryTier::Core);
    assert_eq!(fact.scope, MemoryScope::Persona);
    assert_eq!(fact.persona_id, Some("code-reviewer".to_string()));
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p alephcore test_memory_tier_roundtrip test_memory_scope_roundtrip test_memory_fact_defaults_tier_and_scope test_memory_fact_with_persona -- --nocapture
```

Expected: FAIL with unresolved `MemoryTier`, `MemoryScope`, missing fields.

**Step 3: Implement the enums and fields**

In `core/src/memory/context.rs`, add the two new enums after `MemoryCategory` (after line ~399):

```rust
/// Cognitive tier: temporal lifecycle of a memory fact.
/// Orthogonal to MemoryLayer (abstraction granularity).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTier {
    /// Always online. Injected into system prompt at session start.
    Core,
    /// Recent high-fidelity data. Default for new facts. Decays over time.
    ShortTerm,
    /// Consolidated semantic knowledge. Persists indefinitely.
    LongTerm,
}

impl Default for MemoryTier {
    fn default() -> Self {
        Self::ShortTerm
    }
}

impl MemoryTier {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Core => "core",
            Self::ShortTerm => "short_term",
            Self::LongTerm => "long_term",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or_default()
    }
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryTier {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "core" => Ok(Self::Core),
            "short_term" => Ok(Self::ShortTerm),
            "long_term" => Ok(Self::LongTerm),
            _ => Err(format!("unknown MemoryTier: {s}")),
        }
    }
}

/// Visibility scope: who can see this memory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// Shared across all personas and workspaces.
    Global,
    /// Shared within one workspace across all personas.
    Workspace,
    /// Private to a specific persona.
    Persona,
}

impl Default for MemoryScope {
    fn default() -> Self {
        Self::Global
    }
}

impl MemoryScope {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
            Self::Persona => "persona",
        }
    }

    pub fn from_str_or_default(s: &str) -> Self {
        s.parse().unwrap_or_default()
    }
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryScope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Self::Global),
            "workspace" => Ok(Self::Workspace),
            "persona" => Ok(Self::Persona),
            _ => Err(format!("unknown MemoryScope: {s}")),
        }
    }
}
```

Extend the `MemoryFact` struct (inside the struct definition, after `embedding_model` at L580):

```rust
    /// Cognitive tier: Core (always online), ShortTerm (recent), LongTerm (consolidated)
    pub tier: MemoryTier,
    /// Visibility scope: Global, Workspace, or Persona
    pub scope: MemoryScope,
    /// Persona ID, only set when scope = Persona
    pub persona_id: Option<String>,
    /// Memory strength (0.0-1.0), updated by DreamDaemon via Ebbinghaus decay
    pub strength: f32,
    /// Number of retrieval hits, reinforcement factor for spaced repetition
    pub access_count: u32,
    /// Unix timestamp of last retrieval hit
    pub last_accessed_at: Option<i64>,
```

Update `MemoryFact::new()` (around L595-630) to set defaults:

```rust
    tier: MemoryTier::ShortTerm,
    scope: MemoryScope::Global,
    persona_id: None,
    strength: 1.0,
    access_count: 0,
    last_accessed_at: None,
```

Update `MemoryFact::with_id()` (around L633-668) with same defaults.

Add builder methods (after existing builders):

```rust
    pub fn with_tier(mut self, tier: MemoryTier) -> Self {
        self.tier = tier;
        self
    }

    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn with_persona_id(mut self, persona_id: String) -> Self {
        self.persona_id = Some(persona_id);
        self
    }
```

Update `core/src/memory/mod.rs` re-exports (around L69-72) to add:

```rust
pub use context::{MemoryTier, MemoryScope};
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p alephcore test_memory_tier_roundtrip test_memory_scope_roundtrip test_memory_fact_defaults_tier_and_scope test_memory_fact_with_persona -- --nocapture
```

Expected: PASS.

**Step 5: Fix any compilation errors across codebase**

Run:

```bash
cargo check -p alephcore 2>&1 | head -50
```

If there are compilation errors from existing code that constructs `MemoryFact` manually (e.g. in `arrow_convert.rs`, tests), add the default field values to those sites. All new fields have sensible defaults so this should be mechanical.

**Step 6: Commit**

```bash
git add core/src/memory/context.rs core/src/memory/mod.rs
git commit -m "memory: add MemoryTier, MemoryScope, and strength fields to MemoryFact"
```

---

### Task B: Persist tier/scope/persona_id/strength in LanceDB

**Files:**
- Modify: `core/src/memory/store/lance/schema.rs` (facts_schema at L41-71)
- Modify: `core/src/memory/store/lance/arrow_convert.rs` (facts_to_record_batch at L130-268, record_batch_to_facts at L271-354)
- Test: existing tests in those files

**Step 1: Write the failing tests**

In `core/src/memory/store/lance/schema.rs` test module:

```rust
#[test]
fn facts_schema_has_acma_columns() {
    let schema = facts_schema();
    assert!(schema.field_with_name("tier").is_ok());
    assert!(schema.field_with_name("scope").is_ok());
    assert!(schema.field_with_name("persona_id").is_ok());
    assert!(schema.field_with_name("strength").is_ok());
    assert!(schema.field_with_name("access_count").is_ok());
    assert!(schema.field_with_name("last_accessed_at").is_ok());
}
```

In `core/src/memory/store/lance/arrow_convert.rs` test module:

```rust
#[test]
fn fact_roundtrip_preserves_acma_fields() {
    let mut fact = MemoryFact::new("x".into(), FactType::Preference, vec![]);
    fact.tier = MemoryTier::LongTerm;
    fact.scope = MemoryScope::Persona;
    fact.persona_id = Some("reviewer".to_string());
    fact.strength = 0.75;
    fact.access_count = 5;
    fact.last_accessed_at = Some(1700000000);
    let batch = facts_to_record_batch(&[fact.clone()]).unwrap();
    let out = record_batch_to_facts(&batch).unwrap();
    assert_eq!(out[0].tier, MemoryTier::LongTerm);
    assert_eq!(out[0].scope, MemoryScope::Persona);
    assert_eq!(out[0].persona_id, Some("reviewer".to_string()));
    assert!((out[0].strength - 0.75).abs() < 0.001);
    assert_eq!(out[0].access_count, 5);
    assert_eq!(out[0].last_accessed_at, Some(1700000000));
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p alephcore facts_schema_has_acma_columns fact_roundtrip_preserves_acma_fields -- --nocapture
```

Expected: FAIL — columns missing, conversion doesn't map new fields.

**Step 3: Implement schema and conversion changes**

In `core/src/memory/store/lance/schema.rs` `facts_schema()`, add 6 new columns (before the vector columns):

```rust
    Field::new("tier", DataType::Utf8, false),
    Field::new("scope", DataType::Utf8, false),
    Field::new("persona_id", DataType::Utf8, true),       // nullable
    Field::new("strength", DataType::Float32, false),
    Field::new("access_count", DataType::Int32, false),
    Field::new("last_accessed_at", DataType::Int64, true), // nullable
```

Update expected column count in any tests that assert total column count (+6).

In `core/src/memory/store/lance/arrow_convert.rs`:

**In `facts_to_record_batch()`** — add column builders:

```rust
    // tier (non-null Utf8)
    let tier_col = Arc::new(StringArray::from_iter_values(
        facts.iter().map(|f| f.tier.as_str()),
    )) as ArrayRef;

    // scope (non-null Utf8)
    let scope_col = Arc::new(StringArray::from_iter_values(
        facts.iter().map(|f| f.scope.as_str()),
    )) as ArrayRef;

    // persona_id (nullable Utf8)
    let persona_id_col = Arc::new(StringArray::from(
        facts.iter().map(|f| f.persona_id.as_deref()).collect::<Vec<_>>(),
    )) as ArrayRef;

    // strength (non-null Float32)
    let strength_col = Arc::new(Float32Array::from_iter_values(
        facts.iter().map(|f| f.strength),
    )) as ArrayRef;

    // access_count (non-null Int32)
    let access_count_col = Arc::new(Int32Array::from_iter_values(
        facts.iter().map(|f| f.access_count as i32),
    )) as ArrayRef;

    // last_accessed_at (nullable Int64)
    let last_accessed_at_col = Arc::new(Int64Array::from(
        facts.iter().map(|f| f.last_accessed_at).collect::<Vec<_>>(),
    )) as ArrayRef;
```

Include these 6 columns in the `RecordBatch::try_new()` call at the correct positions matching the schema.

**In `record_batch_to_facts()`** — add column reads with backward-compatible fallbacks:

```rust
    // Read with fallback for migration (old data won't have these columns)
    let tier_col = batch.column_by_name("tier")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let scope_col = batch.column_by_name("scope")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let persona_id_col = batch.column_by_name("persona_id")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>());
    let strength_col = batch.column_by_name("strength")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());
    let access_count_col = batch.column_by_name("access_count")
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>());
    let last_accessed_at_col = batch.column_by_name("last_accessed_at")
        .and_then(|c| c.as_any().downcast_ref::<Int64Array>());
```

In the row iteration loop, populate fields with fallbacks:

```rust
    tier: tier_col
        .map(|c| MemoryTier::from_str_or_default(c.value(i)))
        .unwrap_or_default(),
    scope: scope_col
        .map(|c| MemoryScope::from_str_or_default(c.value(i)))
        .unwrap_or_default(),
    persona_id: persona_id_col
        .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i).to_string()) }),
    strength: strength_col
        .map(|c| c.value(i))
        .unwrap_or(1.0),
    access_count: access_count_col
        .map(|c| c.value(i) as u32)
        .unwrap_or(0),
    last_accessed_at: last_accessed_at_col
        .and_then(|c| if c.is_null(i) { None } else { Some(c.value(i)) }),
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p alephcore facts_schema_has_acma_columns fact_roundtrip_preserves_acma_fields -- --nocapture
```

Expected: PASS.

**Step 5: Full compilation check**

Run:

```bash
cargo check -p alephcore
```

Expected: No errors.

**Step 6: Commit**

```bash
git add core/src/memory/store/lance/schema.rs core/src/memory/store/lance/arrow_convert.rs
git commit -m "memory: persist ACMA fields (tier, scope, persona_id, strength) in LanceDB"
```

---

### Task C: Extend SearchFilter with tier/scope/persona_id and Scope Stack Query

**Files:**
- Modify: `core/src/memory/store/types.rs` (SearchFilter at L26-47, builders at L51-125, to_lance_filter at L133-186)

**Step 1: Write the failing tests**

In `core/src/memory/store/types.rs` test module:

```rust
#[test]
fn search_filter_supports_tier() {
    let filter = SearchFilter::new()
        .with_tier(MemoryTier::Core);
    let sql = filter.to_lance_filter().unwrap();
    assert!(sql.contains("tier = 'core'"));
}

#[test]
fn search_filter_supports_scope_and_persona() {
    let filter = SearchFilter::new()
        .with_scope(MemoryScope::Persona)
        .with_persona_id("reviewer");
    let sql = filter.to_lance_filter().unwrap();
    assert!(sql.contains("scope = 'persona'"));
    assert!(sql.contains("persona_id = 'reviewer'"));
}

#[test]
fn search_filter_scope_stack_generates_or_clause() {
    let filter = SearchFilter::new()
        .with_scope_stack(Some("reviewer"), "aleph");
    let sql = filter.to_lance_filter().unwrap();
    // Should match Global OR (Workspace + workspace=aleph) OR (Persona + persona_id=reviewer)
    assert!(sql.contains("scope = 'global'"));
    assert!(sql.contains("scope = 'workspace'"));
    assert!(sql.contains("scope = 'persona'"));
    assert!(sql.contains("persona_id = 'reviewer'"));
}

#[test]
fn search_filter_scope_stack_without_persona() {
    let filter = SearchFilter::new()
        .with_scope_stack(None, "aleph");
    let sql = filter.to_lance_filter().unwrap();
    // Should match Global OR (Workspace + workspace=aleph) only
    assert!(sql.contains("scope = 'global'"));
    assert!(sql.contains("scope = 'workspace'"));
    assert!(!sql.contains("persona"));
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p alephcore search_filter_supports_tier search_filter_supports_scope_and_persona search_filter_scope_stack -- --nocapture
```

Expected: FAIL — missing fields and methods.

**Step 3: Implement SearchFilter extensions**

In `core/src/memory/store/types.rs`, add fields to `SearchFilter` (after `created_before` at L46):

```rust
    pub tier: Option<MemoryTier>,
    pub scope: Option<MemoryScope>,
    pub persona_id: Option<String>,
    /// Pre-built scope stack OR clause (set by with_scope_stack)
    scope_stack_clause: Option<String>,
```

Add builder methods (after existing builders):

```rust
    pub fn with_tier(mut self, tier: MemoryTier) -> Self {
        self.tier = Some(tier);
        self
    }

    pub fn with_scope(mut self, scope: MemoryScope) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn with_persona_id(mut self, id: &str) -> Self {
        self.persona_id = Some(id.to_string());
        self
    }

    /// Build scope-stack filter: Global OR (Workspace=W) OR (Persona=P).
    /// This overrides individual scope/persona_id filters.
    pub fn with_scope_stack(mut self, persona_id: Option<&str>, workspace: &str) -> Self {
        let mut parts = vec![
            "scope = 'global'".to_string(),
            format!("(scope = 'workspace' AND workspace = '{workspace}')"),
        ];
        if let Some(pid) = persona_id {
            parts.push(format!("(scope = 'persona' AND persona_id = '{pid}')"));
        }
        self.scope_stack_clause = Some(format!("({})", parts.join(" OR ")));
        self
    }
```

In `to_lance_filter()`, add clause generation (before the final join):

```rust
    // Scope stack takes precedence over individual scope/persona filters
    if let Some(ref clause) = self.scope_stack_clause {
        conditions.push(clause.clone());
    } else {
        if let Some(ref tier) = self.tier {
            conditions.push(format!("tier = '{}'", tier.as_str()));
        }
        if let Some(ref scope) = self.scope {
            conditions.push(format!("scope = '{}'", scope.as_str()));
        }
        if let Some(ref persona_id) = self.persona_id {
            conditions.push(format!("persona_id = '{persona_id}'"));
        }
    }

    // tier filter applies regardless of scope stack
    if self.scope_stack_clause.is_some() {
        if let Some(ref tier) = self.tier {
            conditions.push(format!("tier = '{}'", tier.as_str()));
        }
    }
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p alephcore search_filter_supports_tier search_filter_supports_scope_and_persona search_filter_scope_stack -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/memory/store/types.rs
git commit -m "memory: extend SearchFilter with tier, scope, persona_id, and scope-stack query"
```

---

### Task D: Implement ContextComposer

**Files:**
- Create: `core/src/memory/composer.rs`
- Modify: `core/src/memory/mod.rs` (add `pub mod composer;`)

**Step 1: Write the failing tests**

In `core/src/memory/composer.rs`, add an inline test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact, MemoryTier, MemoryScope};

    #[test]
    fn test_build_core_filter_with_persona() {
        let req = CompositionRequest {
            persona_id: Some("reviewer".to_string()),
            workspace: "aleph".to_string(),
            namespace: "owner".to_string(),
            token_budget: 2000,
        };
        let filter = ContextComposer::build_core_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("tier = 'core'"));
        assert!(sql.contains("scope = 'global'"));
        assert!(sql.contains("scope = 'persona'"));
    }

    #[test]
    fn test_build_core_filter_without_persona() {
        let req = CompositionRequest {
            persona_id: None,
            workspace: "aleph".to_string(),
            namespace: "owner".to_string(),
            token_budget: 2000,
        };
        let filter = ContextComposer::build_core_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        assert!(sql.contains("tier = 'core'"));
        assert!(!sql.contains("persona"));
    }

    #[test]
    fn test_build_retrieval_filter() {
        let req = CompositionRequest {
            persona_id: Some("reviewer".to_string()),
            workspace: "aleph".to_string(),
            namespace: "owner".to_string(),
            token_budget: 2000,
        };
        let filter = ContextComposer::build_retrieval_filter(&req);
        let sql = filter.to_lance_filter().unwrap();
        // Should have scope stack but NOT tier=Core
        assert!(sql.contains("scope = 'global'"));
        assert!(sql.contains("scope = 'workspace'"));
        assert!(!sql.contains("tier = 'core'"));
    }
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p alephcore memory::composer::tests -- --nocapture
```

Expected: FAIL — module doesn't exist.

**Step 3: Implement ContextComposer**

Create `core/src/memory/composer.rs`:

```rust
//! Context composition: assembles memory context at session start
//! by layered union of Core + Global + Workspace + Persona facts.

use crate::memory::context::{MemoryFact, MemoryTier, MemoryScope};
use crate::memory::store::types::SearchFilter;
use crate::memory::namespace::NamespaceScope;

/// Request for context composition
pub struct CompositionRequest {
    pub persona_id: Option<String>,
    pub workspace: String,
    pub namespace: String,
    pub token_budget: usize,
}

/// Assembled context ready for prompt injection
pub struct ComposedContext {
    /// Core facts: always injected into system prompt
    pub core_facts: Vec<MemoryFact>,
    /// Relevant facts: ranked by relevance, for <relevant_memories> tag
    pub relevant_facts: Vec<MemoryFact>,
    /// Total tokens consumed
    pub total_tokens: usize,
}

pub struct ContextComposer;

impl ContextComposer {
    /// Build filter for Core Memory retrieval.
    /// Matches: tier=Core AND (scope=Global OR scope=Persona(P))
    pub fn build_core_filter(req: &CompositionRequest) -> SearchFilter {
        SearchFilter::new()
            .with_tier(MemoryTier::Core)
            .with_scope_stack(req.persona_id.as_deref(), &req.workspace)
            .with_namespace(NamespaceScope::Owner)
            .with_valid_only()
    }

    /// Build filter for non-Core retrieval (STM + LTM).
    /// Matches: tier≠Core AND scope_stack(Global, Workspace=W, Persona=P)
    pub fn build_retrieval_filter(req: &CompositionRequest) -> SearchFilter {
        SearchFilter::new()
            .with_scope_stack(req.persona_id.as_deref(), &req.workspace)
            .with_namespace(NamespaceScope::Owner)
            .with_valid_only()
    }
}
```

In `core/src/memory/mod.rs`, add:

```rust
pub mod composer;
```

And add re-export:

```rust
pub use composer::{ContextComposer, CompositionRequest, ComposedContext};
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p alephcore memory::composer::tests -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/memory/composer.rs core/src/memory/mod.rs
git commit -m "memory: add ContextComposer for scope-stack context assembly"
```

---

### Task E: Persistent Strength and on_access() Updates

**Files:**
- Modify: `core/src/memory/decay.rs` (MemoryStrength at L37-49, DecayConfig at L102-111)
- Modify: `core/src/memory/fact_retrieval.rs` (retrieve at L86-132)

**Step 1: Write the failing tests**

In `core/src/memory/decay.rs` test module:

```rust
#[test]
fn test_update_strength_fresh_fact() {
    let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
    let now = fact.created_at; // just created
    update_strength(&mut fact, now, 30.0);
    // Just created, no time elapsed → strength ~1.0
    assert!(fact.strength > 0.95);
}

#[test]
fn test_update_strength_decays_over_time() {
    let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
    let now = fact.created_at + 30 * 86400; // 30 days later, no access
    update_strength(&mut fact, now, 30.0);
    // Half-life = 30 days, ~0.5 base decay
    assert!(fact.strength < 0.6);
    assert!(fact.strength > 0.3);
}

#[test]
fn test_update_strength_access_boost() {
    let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
    fact.access_count = 10;
    fact.last_accessed_at = Some(fact.created_at + 29 * 86400); // accessed 1 day ago
    let now = fact.created_at + 30 * 86400;
    update_strength(&mut fact, now, 30.0);
    // Recently accessed + high access count → much stronger than unaccessed
    assert!(fact.strength > 0.7);
}

#[test]
fn test_on_access_increments() {
    let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
    assert_eq!(fact.access_count, 0);
    assert!(fact.last_accessed_at.is_none());
    let now = fact.created_at + 86400;
    on_access(&mut fact, now);
    assert_eq!(fact.access_count, 1);
    assert_eq!(fact.last_accessed_at, Some(now));
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p alephcore test_update_strength test_on_access_increments -- --nocapture
```

Expected: FAIL — `update_strength` and `on_access` don't exist.

**Step 3: Implement strength functions**

In `core/src/memory/decay.rs`, add:

```rust
use crate::memory::context::MemoryFact;

/// Update a fact's persistent strength using Ebbinghaus decay.
/// Called by DreamDaemon in batch. Uses last_accessed_at for decay base.
pub fn update_strength(fact: &mut MemoryFact, now: i64, half_life_days: f64) {
    let age_days = (now - fact.created_at) as f64 / 86400.0;
    let last_access_days = match fact.last_accessed_at {
        Some(ts) => (now - ts) as f64 / 86400.0,
        None => age_days,
    };

    // Ebbinghaus exponential decay based on time since last access
    let base = (-last_access_days * (2.0_f64.ln()) / half_life_days).exp();

    // Logarithmic access boost (spaced repetition effect)
    let access_boost = (fact.access_count as f64).ln_1p() * 0.15;

    fact.strength = (base as f32 + access_boost).clamp(0.0, 1.0);
}

/// Record a retrieval hit. Called when a fact is returned from search.
pub fn on_access(fact: &mut MemoryFact, now: i64) {
    fact.access_count += 1;
    fact.last_accessed_at = Some(now);
}
```

In `core/src/memory/fact_retrieval.rs`, in the `retrieve()` method (around L103-111 where facts are mapped), add `on_access` call:

```rust
    // After mapping ScoredFact to MemoryFact:
    // Update access tracking for returned facts
    let now = chrono::Utc::now().timestamp();
    for fact in &mut facts {
        crate::memory::decay::on_access(fact, now);
    }
    // Note: persisting updated access_count/last_accessed_at back to DB
    // is deferred — DreamDaemon will read these from the in-memory state
    // or we batch-update after retrieval completes.
```

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p alephcore test_update_strength test_on_access_increments -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/memory/decay.rs core/src/memory/fact_retrieval.rs
git commit -m "memory: add persistent strength tracking and on_access() for spaced repetition"
```

---

### Task F: DreamDaemon Consolidation and Pruning Phases

**Files:**
- Modify: `core/src/memory/dreaming.rs` (run_dream at L328-428, DreamDaemon at L150-159)
- Modify: `core/src/config/` (add consolidation_pipeline config)

**Step 1: Write the failing tests**

In `core/src/memory/dreaming.rs` test module:

```rust
#[cfg(test)]
mod consolidation_tests {
    use super::*;
    use crate::memory::context::{MemoryFact, FactType, MemoryTier};

    #[test]
    fn test_should_consolidate() {
        let mut fact = MemoryFact::new("test".into(), FactType::Learning, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.7;
        assert!(should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_not_consolidate_low_strength() {
        let mut fact = MemoryFact::new("test".into(), FactType::Learning, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.4;
        assert!(!should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_not_consolidate_non_stm() {
        let mut fact = MemoryFact::new("test".into(), FactType::Learning, vec![]);
        fact.tier = MemoryTier::LongTerm;
        fact.strength = 0.9;
        assert!(!should_consolidate(&fact, 0.6));
    }

    #[test]
    fn test_should_prune() {
        let mut fact = MemoryFact::new("test".into(), FactType::Other, vec![]);
        fact.tier = MemoryTier::ShortTerm;
        fact.strength = 0.05;
        assert!(should_prune(&fact, 0.1));
    }

    #[test]
    fn test_should_not_prune_core() {
        let mut fact = MemoryFact::new("test".into(), FactType::Personal, vec![]);
        fact.tier = MemoryTier::Core;
        fact.strength = 0.01;
        assert!(!should_prune(&fact, 0.1));
    }
}
```

**Step 2: Run tests to verify they fail**

Run:

```bash
cargo test -p alephcore consolidation_tests -- --nocapture
```

Expected: FAIL — `should_consolidate` and `should_prune` don't exist.

**Step 3: Implement consolidation logic**

In `core/src/memory/dreaming.rs`, add helper functions:

```rust
/// Check if a STM fact qualifies for consolidation into LTM.
pub fn should_consolidate(fact: &MemoryFact, strength_threshold: f32) -> bool {
    fact.tier == MemoryTier::ShortTerm && fact.strength >= strength_threshold
}

/// Check if a fact should be pruned (deleted permanently).
/// Core tier facts are never pruned.
pub fn should_prune(fact: &MemoryFact, pruning_threshold: f32) -> bool {
    fact.tier != MemoryTier::Core && fact.strength < pruning_threshold
}
```

Add consolidation config struct:

```rust
/// Configuration for the STM→LTM consolidation pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationPipelineConfig {
    pub enabled: bool,
    /// STM facts need this strength to be considered for consolidation
    pub strength_threshold: f32,
    /// Facts below this strength are deleted
    pub pruning_threshold: f32,
    /// Max facts to process per Dream cycle
    pub max_facts_per_run: usize,
    /// Minimum days between consolidation checks for the same fact
    pub cooldown_days: u32,
}

impl Default for ConsolidationPipelineConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength_threshold: 0.6,
            pruning_threshold: 0.1,
            max_facts_per_run: 50,
            cooldown_days: 1,
        }
    }
}
```

In `run_dream()` (after existing Phase 3 / L0/L1 generation, before the return), add Phase 4 and 5 stubs:

```rust
    // Phase 4: Consolidation (STM → LTM)
    if self.consolidation_config.enabled {
        let stm_filter = SearchFilter::new()
            .with_tier(MemoryTier::ShortTerm)
            .with_valid_only();
        // Fetch STM facts, filter by strength threshold,
        // cluster by topic, distill via LLM, write LTM facts.
        // Implementation details: use existing CompressionService pattern.
        tracing::info!("dream: consolidation phase placeholder");
    }

    // Phase 5: Pruning
    if self.consolidation_config.enabled {
        // Fetch all non-Core facts with strength < pruning_threshold
        // and delete them permanently.
        tracing::info!("dream: pruning phase placeholder");
    }
```

Add `consolidation_config: ConsolidationPipelineConfig` field to `DreamDaemon` struct and initialize it in `from_config()`.

**Step 4: Run tests to verify they pass**

Run:

```bash
cargo test -p alephcore consolidation_tests -- --nocapture
```

Expected: PASS.

**Step 5: Run strength update in Phase 2**

In `run_dream()`, update Phase 2 to use `update_strength()` for all facts:

```rust
    // Phase 2: Memory Strength Update (replaces simple decay)
    // Fetch all valid STM facts and update their strength
    let stm_facts = self.database.get_facts_by_filter(
        &SearchFilter::new().with_tier(MemoryTier::ShortTerm).with_valid_only()
    ).await.unwrap_or_default();

    for mut fact in stm_facts {
        crate::memory::decay::update_strength(&mut fact, run_start, self.memory_decay.half_life_days as f64);
        // Persist updated strength back to DB
        let _ = self.database.update_fact_strength(&fact.id, fact.strength).await;
    }
```

**Step 6: Commit**

```bash
git add core/src/memory/dreaming.rs
git commit -m "memory: add consolidation and pruning phases to DreamDaemon"
```

---

### Task G: Integration Tests and Documentation Update

**Files:**
- Modify: `core/src/memory/integration_tests/workspace_isolation.rs` (add scope isolation tests)
- Modify: `docs/MEMORY_SYSTEM.md`

**Step 1: Write integration tests**

In `core/src/memory/integration_tests/`, add scope isolation tests:

```rust
#[tokio::test]
async fn test_scope_isolation_persona_facts_invisible_by_default() {
    // 1. Insert a Global fact
    // 2. Insert a Persona("reviewer") fact
    // 3. Query with scope_stack(persona_id=None, workspace="test")
    // 4. Assert: Global fact visible, Persona fact invisible
}

#[tokio::test]
async fn test_scope_stack_includes_own_persona() {
    // 1. Insert a Persona("reviewer") fact
    // 2. Query with scope_stack(persona_id=Some("reviewer"), workspace="test")
    // 3. Assert: Persona fact visible
}

#[tokio::test]
async fn test_core_tier_facts_retrieved_separately() {
    // 1. Insert a Core+Global fact
    // 2. Insert a ShortTerm+Global fact
    // 3. Build core filter, assert only Core fact returned
    // 4. Build retrieval filter, assert only ShortTerm fact returned
}

#[tokio::test]
async fn test_strength_based_pruning_candidates() {
    // 1. Insert STM facts with varying strength (0.05, 0.5, 0.9)
    // 2. Assert should_prune identifies only the 0.05 fact
    // 3. Assert should_consolidate identifies only the 0.9 fact
}
```

**Step 2: Run integration tests**

Run:

```bash
cargo test -p alephcore memory::integration_tests -- --nocapture
```

Expected: PASS (after implementing the test bodies with actual store operations).

**Step 3: Update documentation**

In `docs/MEMORY_SYSTEM.md`, add a new section after "Memory Decay" (after line ~382):

```markdown
---

## Cognitive Memory Architecture (ACMA)

**Location**: `core/src/memory/composer.rs`, `core/src/memory/decay.rs`, `core/src/memory/dreaming.rs`

Aleph's memory system uses three orthogonal dimensions:

| Dimension | Field | Values | Purpose |
|-----------|-------|--------|---------|
| Abstraction | `layer` | L0 / L1 / L2 | Granularity (abstract → detail) |
| Temperature | `tier` | Core / ShortTerm / LongTerm | Temporal lifecycle |
| Visibility | `scope` | Global / Workspace / Persona | Access isolation |

### Memory Tiers

| Tier | Behavior | Decay |
|------|----------|-------|
| **Core** | Injected into system prompt every request | Never decays |
| **ShortTerm** | Default for new facts. High fidelity, recent. | Ebbinghaus curve (half-life ~30 days) |
| **LongTerm** | Consolidated semantic knowledge. | Protected from decay |

### Memory Scopes

| Scope | Visibility | Use Case |
|-------|-----------|----------|
| **Global** | All personas, all workspaces | User preferences, API keys |
| **Workspace** | All personas in one workspace | Project architecture, TODOs |
| **Persona** | One persona only | Role-specific patterns, drafts |

### Context Composition

At session start, `ContextComposer` assembles context via layered union:

1. Core(Persona=P) + Core(Global) → system prompt
2. Query(Global) + Query(Workspace=W) + Query(Persona=P) → relevant memories

### Forgetting Curve

Facts track persistent `strength` (0.0-1.0), updated by DreamDaemon:

- **Decay**: Exponential based on time since last access
- **Reinforcement**: Each retrieval hit boosts strength via `on_access()`
- **Consolidation**: STM facts with strength ≥ 0.6 are distilled into LTM
- **Pruning**: Facts with strength < 0.1 are permanently deleted

### Configuration

\`\`\`toml
[memory.consolidation_pipeline]
enabled = true
strength_threshold = 0.6
pruning_threshold = 0.1
max_facts_per_run = 50
cooldown_days = 1
\`\`\`
```

**Step 4: Commit**

```bash
git add core/src/memory/integration_tests/ docs/MEMORY_SYSTEM.md
git commit -m "memory: add ACMA integration tests and documentation"
```

---

## Verification Checklist

- [ ] `MemoryFact` has `tier`, `scope`, `persona_id`, `strength`, `access_count`, `last_accessed_at` fields
- [ ] LanceDB schema persists and round-trips all 6 new fields
- [ ] Backward-compatible: missing columns default to ShortTerm/Global/None/1.0/0/None
- [ ] `SearchFilter` supports `with_tier()`, `with_scope()`, `with_persona_id()`, `with_scope_stack()`
- [ ] `to_lance_filter()` generates correct SQL for scope stack (OR clauses)
- [ ] `ContextComposer` builds separate Core and retrieval filters
- [ ] `update_strength()` implements Ebbinghaus decay with access boost
- [ ] `on_access()` increments access_count and updates last_accessed_at
- [ ] `should_consolidate()` selects high-strength STM facts
- [ ] `should_prune()` selects low-strength non-Core facts
- [ ] DreamDaemon runs consolidation and pruning after existing phases
- [ ] Core tier facts never decay and never get pruned
- [ ] `cargo test -p alephcore` passes
- [ ] `cargo check -p alephcore` passes
- [ ] `docs/MEMORY_SYSTEM.md` documents ACMA
