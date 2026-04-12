# Memory Explore Tool — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose RippleTask as a `memory_explore` builtin tool for multi-hop knowledge exploration.

**Architecture:** New `MemoryExploreTool` struct implements the `AlephTool` trait, following the same pattern as `MemorySearchTool`. It embeds the query, retrieves seed facts, loads their embeddings, then calls `RippleTask::explore()` for BFS multi-hop expansion. Registered in the builtin registry alongside `memory_search` and `memory_browse`.

**Tech Stack:** Rust, async_trait, serde, schemars

---

## Task 1: Create MemoryExploreTool

**Files:**
- Create: `src/builtin_tools/memory_explore.rs`
- Modify: `src/builtin_tools/mod.rs` — add `pub mod memory_explore` and re-export

- [ ] **Step 1: Create the tool file**

Create `src/builtin_tools/memory_explore.rs` with the full implementation. Read `src/builtin_tools/memory_search.rs` first to understand the exact `AlephTool` trait signature (associated types `Args`, `Output`, const `NAME`, `DESCRIPTION`, methods `examples()`, `call()`).

The tool needs:

```rust
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::builtin_tools::AlephTool;
use crate::error::AlephError;
use crate::memory::context::MemoryFact;
use crate::memory::namespace::NamespaceScope;
use crate::memory::ripple::{RippleConfig, RippleTask};
use crate::memory::store::types::SearchFilter;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::EmbeddingProvider;
use crate::sync_primitives::Arc;
```

**Args struct:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryExploreArgs {
    /// Starting query to explore from
    pub query: String,
    /// Maximum exploration depth (default: 2, max: 4)
    #[serde(default = "default_max_hops")]
    pub max_hops: usize,
    /// Facts to discover per hop (default: 5, max: 10)
    #[serde(default = "default_max_per_hop")]
    pub max_per_hop: usize,
}

fn default_max_hops() -> usize { 2 }
fn default_max_per_hop() -> usize { 5 }
```

**Output struct:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryExploreOutput {
    /// Seed facts that matched the initial query
    pub seed_facts: Vec<ExploredFact>,
    /// Related facts discovered through multi-hop exploration
    pub expanded_facts: Vec<ExploredFact>,
    /// Total exploration hops performed
    pub hops_performed: usize,
    /// Human-readable summary
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExploredFact {
    pub id: String,
    pub content: String,
    pub path: String,
    pub relevance_score: f32,
}
```

**Tool struct and AlephTool impl:**
```rust
pub struct MemoryExploreTool {
    database: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl MemoryExploreTool {
    pub const NAME: &'static str = "memory_explore";
    pub const DESCRIPTION: &'static str =
        "Explore related knowledge by following semantic connections from a starting query. \
        Use when you need deeper context about a topic — discovers related facts across \
        multiple hops of similarity.";

    pub fn new(database: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self { database, embedder }
    }
}

#[async_trait]
impl AlephTool for MemoryExploreTool {
    const NAME: &'static str = "memory_explore";
    const DESCRIPTION: &'static str = /* same as above */;
    type Args = MemoryExploreArgs;
    type Output = MemoryExploreOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_explore(query='Rust async patterns')".to_string(),
            "memory_explore(query='my travel plans', max_hops=3)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> crate::Result<Self::Output> {
        // 1. Clamp parameters
        let max_hops = args.max_hops.min(4);
        let max_per_hop = args.max_per_hop.min(10);

        // 2. Embed query
        let query_embedding = self.embedder.embed(&args.query).await
            .map_err(|e| AlephError::other(format!("Failed to embed query: {e}")))?;

        // 3. Vector search for seed facts (top 3)
        let dim_hint = query_embedding.len() as u32;
        let filter = SearchFilter::valid_only(Some(NamespaceScope::Owner));
        let seed_scored = self.database
            .vector_search(&query_embedding, dim_hint, &filter, 3)
            .await?;

        if seed_scored.is_empty() {
            return Ok(MemoryExploreOutput {
                seed_facts: Vec::new(),
                expanded_facts: Vec::new(),
                hops_performed: 0,
                summary: "No related knowledge found for this query.".to_string(),
            });
        }

        // 4. Load embeddings for seed facts (needed by RippleTask::explore)
        let mut seed_facts: Vec<MemoryFact> = Vec::new();
        for sf in &seed_scored {
            let mut fact = sf.fact.clone();
            fact.similarity_score = Some(sf.score);
            // Load embedding — skip this seed if loading fails
            if self.database.load_embedding_for_fact(&mut fact).await.is_ok() {
                seed_facts.push(fact);
            }
        }

        if seed_facts.is_empty() {
            // Seeds found but no embeddings loaded
            let seeds: Vec<ExploredFact> = seed_scored.iter().map(|sf| ExploredFact {
                id: sf.fact.id.clone(),
                content: sf.fact.content.clone(),
                path: sf.fact.path.clone(),
                relevance_score: sf.score,
            }).collect();
            return Ok(MemoryExploreOutput {
                seed_facts: seeds,
                expanded_facts: Vec::new(),
                hops_performed: 0,
                summary: format!("Found {} seed facts but could not load embeddings for exploration.", seeds.len()),
            });
        }

        // 5. Ripple explore
        let config = RippleConfig {
            max_hops,
            max_facts_per_hop: max_per_hop,
            similarity_threshold: 0.7,
            ..RippleConfig::default()
        };
        let ripple = RippleTask::new(self.database.clone(), config);
        let result = ripple.explore(seed_facts.clone()).await?;

        // 6. Format output
        let seed_output: Vec<ExploredFact> = seed_facts.iter().map(|f| ExploredFact {
            id: f.id.clone(),
            content: f.content.clone(),
            path: f.path.clone(),
            relevance_score: f.similarity_score.unwrap_or(0.0),
        }).collect();

        let expanded_output: Vec<ExploredFact> = result.expanded_facts.iter().map(|f| ExploredFact {
            id: f.id.clone(),
            content: f.content.clone(),
            path: f.path.clone(),
            relevance_score: f.similarity_score.unwrap_or(0.0),
        }).collect();

        let summary = format!(
            "Explored {} seed facts across {} hops, discovered {} related facts.",
            seed_output.len(), max_hops, expanded_output.len()
        );

        Ok(MemoryExploreOutput {
            seed_facts: seed_output,
            expanded_facts: expanded_output,
            hops_performed: max_hops,
            summary,
        })
    }
}
```

**Important**: Read the actual `AlephTool` trait definition to confirm exact signatures. The above follows the `MemorySearchTool` pattern but adapt if the trait differs.

- [ ] **Step 2: Add module declaration and re-export in mod.rs**

In `src/builtin_tools/mod.rs`, add:

```rust
pub mod memory_explore;
```

And in the re-exports section:
```rust
pub use memory_explore::{MemoryExploreArgs, MemoryExploreOutput, MemoryExploreTool};
```

- [ ] **Step 3: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -20
```

Fix any type mismatches. Common issues:
- `AlephTool` trait may have a different error type (check `call` return type)
- `JsonSchema` derive may require additional bounds
- Import paths may differ

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/memory_explore.rs src/builtin_tools/mod.rs
git commit -m "memory: add memory_explore tool wrapping RippleTask

New builtin tool for multi-hop knowledge exploration. Embeds query,
retrieves seed facts, loads embeddings, then uses RippleTask BFS to
discover related facts across configurable hops. Parameters: query,
max_hops (default 2, max 4), max_per_hop (default 5, max 10)."
```

---

## Task 2: Register in Builtin Registry

**Files:**
- Modify: `src/executor/builtin_registry/builder.rs` — create and register `MemoryExploreTool`

- [ ] **Step 1: Read builder.rs to understand the registration pattern**

Read `src/executor/builtin_registry/builder.rs` lines 165-194 to see how `MemorySearchTool` and `MemoryBrowseTool` are created and stored. Also check where `tool_declarations()` registers them (around line 1074-1120).

- [ ] **Step 2: Add MemoryExploreTool creation**

In the import section of `builder.rs`, add `MemoryExploreTool` to the import from `builtin_tools`:

```rust
use crate::builtin_tools::{
    // ... existing imports ...
    MemoryExploreTool,
};
```

In the memory tools creation block (around line 170), after `MemoryBrowseTool` creation, add:

```rust
let memory_explore_tool = if let (Some(ref db), Some(ref embedder)) = (&config.memory_db, &config.embedder) {
    Some(MemoryExploreTool::new(db.clone(), Arc::clone(embedder)))
} else {
    None
};
```

The exact placement depends on the builder struct — it may need to be stored as a field or local variable. Follow the pattern used by `memory_search_tool` and `memory_browse_tool`.

- [ ] **Step 3: Register in tool_declarations**

Find the `tool_declarations` method (around line 1074) where `memory_search_tool` and `memory_browse_tool` are registered. Add `memory_explore_tool` registration following the same pattern:

```rust
if memory_explore_tool.is_some() {
    declarations.push(ToolDeclaration {
        name: MemoryExploreTool::NAME.to_string(),
        description: MemoryExploreTool::DESCRIPTION.to_string(),
        // ... follow exact same pattern as memory_search_tool registration
    });
}
```

- [ ] **Step 4: Store the tool in the builder's struct**

Follow the pattern of `memory_search_tool` field — add `memory_explore_tool` as `Option<MemoryExploreTool>` to the builder/registry struct, and handle it in the `call_tool` dispatch (where tool name is matched to execute the right tool).

- [ ] **Step 5: Compile to verify**

```bash
cargo check -p alephcore 2>&1 | head -20
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p alephcore --lib memory::ripple -- -v 2>&1 | tail -10
cargo test -p alephcore --lib builtin_tools -- -v 2>&1 | tail -10
```

- [ ] **Step 7: Commit**

```bash
git add src/executor/builtin_registry/builder.rs
git commit -m "memory: register memory_explore tool in builtin registry

MemoryExploreTool created alongside memory_search/browse when both
database and embedder are available. Registered in tool_declarations
for LLM discovery."
```

---

## Task 3: Final Verification

- [ ] **Step 1: Full compilation**

```bash
cargo check -p alephcore 2>&1 | head -10
```

- [ ] **Step 2: Run all memory tests**

```bash
cargo test -p alephcore --lib memory:: -- --test-threads=1 2>&1 | tail -10
```

- [ ] **Step 3: Run clippy**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | grep "error\[" | head -10
```

- [ ] **Step 4: Verify tool is declared**

```bash
grep -n "memory_explore" src/executor/builtin_registry/builder.rs src/builtin_tools/mod.rs src/builtin_tools/memory_explore.rs | head -20
```

Expected: tool creation, registration, and implementation all connected.

- [ ] **Step 5: Commit if clippy fixes needed**

```bash
git add -A
git commit -m "memory: fix clippy warnings after memory_explore tool addition"
```
