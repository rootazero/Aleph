# Memory Auto-Injection Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Automatically inject relevant LanceDB memories and facts into LLM prompts via a new `MemoryAugmentationLayer` in the PromptPipeline.

**Architecture:** Pre-fetch memory results asynchronously before prompt assembly (matching the `WorkspaceFiles` pattern), then inject them synchronously via a new PromptLayer at priority 1575. Also clean up two unused legacy modules (`augmentation.rs`, `retrieval.rs`).

**Tech Stack:** Rust, Tokio (async), LanceDB (vector search), PromptPipeline (layered prompt assembly)

**Design doc:** `docs/plans/2026-03-07-memory-auto-injection-design.md`

**Critical constraint:** `PromptLayer::inject()` is synchronous. Async LanceDB queries must happen before prompt assembly. Results are passed through `LayerInput` — the same pattern used by `WorkspaceFiles`.

---

### Task 1: Add `MemoryContext` data structure

**Files:**
- Create: `core/src/thinker/memory_context.rs`
- Modify: `core/src/thinker/mod.rs`

**Step 1: Create the data structure**

Create `core/src/thinker/memory_context.rs`:

```rust
//! Pre-fetched memory context for prompt injection.
//!
//! Memory retrieval is async (embedding + LanceDB), but PromptLayer::inject()
//! is sync. This struct holds pre-fetched results to bridge that gap.

use crate::memory::context::MemoryFact;
use crate::memory::store::ScoredFact;

/// Pre-fetched memory context ready for prompt injection.
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 2 facts (compressed knowledge), sorted by relevance.
    pub facts: Vec<ScoredFact>,
    /// Layer 1 memory summaries (raw conversation excerpts).
    pub memory_summaries: Vec<MemorySummary>,
}

/// A brief summary of a past conversation for prompt injection.
#[derive(Debug, Clone)]
pub struct MemorySummary {
    /// Date string (YYYY-MM-DD)
    pub date: String,
    /// User's question/input (truncated)
    pub user_input: String,
    /// AI's response (truncated)
    pub ai_output: String,
    /// Similarity score
    pub score: f32,
}

impl MemoryContext {
    /// Whether there is any content to inject.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.memory_summaries.is_empty()
    }

    /// Format into a prompt section string.
    pub fn format_for_prompt(&self) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut output = String::from("## Relevant Memory\n\n");

        if !self.facts.is_empty() {
            output.push_str("**Facts:**\n");
            for sf in &self.facts {
                output.push_str(&format!("- {}\n", sf.fact.content));
            }
            output.push('\n');
        }

        if !self.memory_summaries.is_empty() {
            output.push_str("**Past Conversations:**\n");
            for ms in &self.memory_summaries {
                output.push_str(&format!(
                    "- [{}] Q: {} A: {}\n",
                    ms.date, ms.user_input, ms.ai_output
                ));
            }
            output.push('\n');
        }

        output
    }
}
```

**Step 2: Register the module**

In `core/src/thinker/mod.rs`, add after the `pub mod workspace_files;` line (~line 44):

```rust
pub mod memory_context;
```

And add a re-export after the workspace_files re-export (~line 85):

```rust
pub use memory_context::{MemoryContext, MemorySummary};
```

**Step 3: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles with no errors

**Step 4: Commit**

```bash
git add core/src/thinker/memory_context.rs core/src/thinker/mod.rs
git commit -m "thinker: add MemoryContext data structure for pre-fetched memory"
```

---

### Task 2: Add `memory_context` field to `LayerInput`

**Files:**
- Modify: `core/src/thinker/prompt_layer.rs:35-130`

**Step 1: Add field to LayerInput**

In `LayerInput` struct (after `workspace` field, ~line 50), add:

```rust
    /// Pre-fetched memory context from LanceDB (facts + memory summaries).
    pub memory_context: Option<&'a super::memory_context::MemoryContext>,
```

**Step 2: Initialize to None in all constructors**

In each constructor (`basic`, `hydration`, `soul`, `context`), add `memory_context: None` to the struct initializer.

**Step 3: Add builder method**

After `with_workspace_opt` (~line 118), add:

```rust
    /// Attach pre-fetched memory context.
    pub fn with_memory_context(mut self, ctx: &'a super::memory_context::MemoryContext) -> Self {
        self.memory_context = Some(ctx);
        self
    }

    /// Attach optional pre-fetched memory context.
    pub fn with_memory_context_opt(mut self, ctx: Option<&'a super::memory_context::MemoryContext>) -> Self {
        self.memory_context = ctx;
        self
    }
```

**Step 4: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles with no errors

**Step 5: Commit**

```bash
git add core/src/thinker/prompt_layer.rs
git commit -m "prompt_layer: add memory_context field to LayerInput"
```

---

### Task 3: Implement `MemoryAugmentationLayer`

**Files:**
- Create: `core/src/thinker/layers/memory_augmentation.rs`
- Modify: `core/src/thinker/layers/mod.rs`

**Step 1: Create the layer**

Create `core/src/thinker/layers/memory_augmentation.rs`:

```rust
//! MemoryAugmentationLayer — inject pre-fetched LanceDB memory context (priority 1575)
//!
//! Sits between WorkspaceFilesLayer (1550) and LanguageLayer (1600).
//! The async retrieval happens before prompt assembly; this layer only
//! formats and injects the pre-fetched results.

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct MemoryAugmentationLayer;

impl PromptLayer for MemoryAugmentationLayer {
    fn name(&self) -> &'static str {
        "memory_augmentation"
    }

    fn priority(&self) -> u32 {
        1575
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }

    fn supports_mode(&self, mode: PromptMode) -> bool {
        // Include in Full and Compact, skip in Minimal (save tokens)
        !matches!(mode, PromptMode::Minimal)
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.memory_context {
            Some(ctx) if !ctx.is_empty() => ctx,
            _ => return,
        };

        output.push_str(&ctx.format_for_prompt());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::PromptLayer as _;
    use crate::thinker::memory_context::{MemoryContext, MemorySummary};
    use crate::memory::store::ScoredFact;
    use crate::memory::context::MemoryFact;

    #[test]
    fn metadata() {
        let layer = MemoryAugmentationLayer;
        assert_eq!(layer.name(), "memory_augmentation");
        assert_eq!(layer.priority(), 1575);
        assert!(layer.paths().contains(&AssemblyPath::Basic));
        assert!(layer.paths().contains(&AssemblyPath::Soul));
    }

    #[test]
    fn supports_full_and_compact_not_minimal() {
        let layer = MemoryAugmentationLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }

    #[test]
    fn skips_when_no_memory_context() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn skips_when_empty_context() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();
        let ctx = MemoryContext::default();
        let input = LayerInput::basic(&config, &[]).with_memory_context(&ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn injects_facts_and_memories() {
        let layer = MemoryAugmentationLayer;
        let config = PromptConfig::default();

        let ctx = MemoryContext {
            facts: vec![ScoredFact {
                fact: MemoryFact::new("User prefers dark mode"),
                score: 0.9,
            }],
            memory_summaries: vec![MemorySummary {
                date: "2026-03-05".to_string(),
                user_input: "How to configure embedding?".to_string(),
                ai_output: "Use aleph.toml...".to_string(),
                score: 0.8,
            }],
        };

        let input = LayerInput::basic(&config, &[]).with_memory_context(&ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Relevant Memory"));
        assert!(out.contains("**Facts:**"));
        assert!(out.contains("User prefers dark mode"));
        assert!(out.contains("**Past Conversations:**"));
        assert!(out.contains("[2026-03-05]"));
        assert!(out.contains("How to configure embedding?"));
    }
}
```

**Step 2: Register in layers/mod.rs**

In `core/src/thinker/layers/mod.rs`, add after the workspace_files section (~line 40):

```rust
// --- Memory augmentation layer ---
mod memory_augmentation;
```

And in the re-exports section (~line 74), add:

```rust
pub use memory_augmentation::MemoryAugmentationLayer;
```

**Step 3: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -5`
Expected: compiles (tests may need adjustment if `MemoryFact::new` signature differs — check and adapt)

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib thinker::layers::memory_augmentation 2>&1 | tail -20`
Expected: all tests pass

**Step 5: Commit**

```bash
git add core/src/thinker/layers/memory_augmentation.rs core/src/thinker/layers/mod.rs
git commit -m "layers: add MemoryAugmentationLayer (priority 1575)"
```

---

### Task 4: Add layer to PromptPipeline and update tests

**Files:**
- Modify: `core/src/thinker/prompt_pipeline.rs:107-162`

**Step 1: Add layer to default_layers**

In `default_layers()` (~line 159), add after `WorkspaceFilesLayer`:

```rust
            Box::new(MemoryAugmentationLayer),
```

**Step 2: Update test assertions**

In the same file, update the following tests:

1. `test_default_layers_count` (~line 265): change `assert_eq!(pipeline.layer_count(), 25)` to `26`
2. `compact_mode_excludes_heavy_layers` (~line 310): Add `"memory_augmentation"` to the `excluded_in_compact` array — wait, the layer DOES support Compact. So no change needed here.
3. `minimal_mode_only_core_layers` (~line 345): The layer does NOT support Minimal, so it should NOT be in `included_in_minimal`. The existing assertion logic should work since `memory_augmentation` is not in the list and supports_mode(Minimal) returns false. No change needed.

So only the count needs updating.

**Step 3: Verify it compiles and tests pass**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline 2>&1 | tail -20`
Expected: all tests pass

**Step 4: Commit**

```bash
git add core/src/thinker/prompt_pipeline.rs
git commit -m "pipeline: register MemoryAugmentationLayer in default_layers"
```

---

### Task 5: Create `MemoryContextProvider` for async pre-fetch

**Files:**
- Create: `core/src/thinker/memory_context_provider.rs`
- Modify: `core/src/thinker/mod.rs`

**Step 1: Create the async provider**

Create `core/src/thinker/memory_context_provider.rs`:

```rust
//! Async memory context provider — fetches relevant memories before prompt assembly.
//!
//! PromptLayer::inject() is sync, so we pre-fetch LanceDB results here
//! and store them in MemoryContext for the layer to format.

use crate::memory::EmbeddingProvider;
use crate::memory::store::{MemoryBackend, SearchFilter, ScoredFact};
use crate::memory::store::types::MemoryFilter;
use crate::memory::context::MemoryEntry;
use crate::sync_primitives::Arc;
use super::memory_context::{MemoryContext, MemorySummary};
use tracing::{debug, warn};

/// Configuration for memory context retrieval.
pub struct MemoryContextConfig {
    /// Maximum number of facts to retrieve.
    pub max_facts: usize,
    /// Maximum number of memories to retrieve.
    pub max_memories: usize,
    /// Minimum cosine similarity threshold.
    pub similarity_threshold: f32,
    /// Maximum characters for the formatted output.
    pub max_output_chars: usize,
}

impl Default for MemoryContextConfig {
    fn default() -> Self {
        Self {
            max_facts: 5,
            max_memories: 5,
            similarity_threshold: 0.3,
            max_output_chars: 8000, // ~2000 tokens
        }
    }
}

/// Provides pre-fetched memory context for prompt injection.
pub struct MemoryContextProvider {
    memory_db: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: MemoryContextConfig,
}

impl MemoryContextProvider {
    /// Create a new provider.
    pub fn new(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            memory_db,
            embedder,
            config: MemoryContextConfig::default(),
        }
    }

    /// Create with custom config.
    pub fn with_config(
        memory_db: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        config: MemoryContextConfig,
    ) -> Self {
        Self {
            memory_db,
            embedder,
            config,
        }
    }

    /// Fetch relevant memory context for a user query.
    ///
    /// Returns empty context on any failure (never blocks LLM calls).
    pub async fn fetch(&self, query: &str) -> MemoryContext {
        if query.trim().is_empty() {
            return MemoryContext::default();
        }

        // 1. Generate query embedding
        let embedding = match self.embedder.embed(query).await {
            Ok(emb) => emb,
            Err(e) => {
                warn!(error = %e, "Memory augmentation: embedding failed, skipping");
                return MemoryContext::default();
            }
        };

        let dim = embedding.len() as u32;

        // 2. Search facts and memories in parallel
        let facts_future = self.search_facts(&embedding, dim);
        let memories_future = self.search_memories(&embedding);

        let (facts, memories) = tokio::join!(facts_future, memories_future);

        // 3. Build context, truncating to budget
        let mut ctx = MemoryContext {
            facts: facts.unwrap_or_default(),
            memory_summaries: memories.unwrap_or_default(),
        };

        // 4. Truncate to character budget
        self.truncate_to_budget(&mut ctx);

        debug!(
            facts = ctx.facts.len(),
            memories = ctx.memory_summaries.len(),
            "Memory context fetched for prompt augmentation"
        );

        ctx
    }

    async fn search_facts(
        &self,
        embedding: &[f32],
        dim: u32,
    ) -> Result<Vec<ScoredFact>, ()> {
        let filter = SearchFilter::default();
        self.memory_db
            .vector_search(embedding, dim, &filter, self.config.max_facts)
            .await
            .map(|mut results| {
                results.retain(|sf| sf.score >= self.config.similarity_threshold);
                results
            })
            .map_err(|e| {
                warn!(error = %e, "Memory augmentation: facts search failed");
            })
    }

    async fn search_memories(
        &self,
        embedding: &[f32],
    ) -> Result<Vec<MemorySummary>, ()> {
        let filter = MemoryFilter::default();
        self.memory_db
            .search_memories(embedding, &filter, self.config.max_memories)
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .filter(|e| e.similarity_score.unwrap_or(0.0) >= self.config.similarity_threshold)
                    .map(|e| {
                        let date = chrono::DateTime::from_timestamp(e.context.timestamp, 0)
                            .map(|dt| dt.format("%Y-%m-%d").to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        MemorySummary {
                            date,
                            user_input: truncate_str(&e.user_input, 150),
                            ai_output: truncate_str(&e.ai_output, 200),
                            score: e.similarity_score.unwrap_or(0.0),
                        }
                    })
                    .collect()
            })
            .map_err(|e| {
                warn!(error = %e, "Memory augmentation: memories search failed");
            })
    }

    fn truncate_to_budget(&self, ctx: &mut MemoryContext) {
        let formatted = ctx.format_for_prompt();
        if formatted.len() <= self.config.max_output_chars {
            return;
        }

        // Remove memories first (facts are higher value)
        while formatted.len() > self.config.max_output_chars && !ctx.memory_summaries.is_empty() {
            ctx.memory_summaries.pop();
            let formatted = ctx.format_for_prompt();
            if formatted.len() <= self.config.max_output_chars {
                return;
            }
        }

        // Then trim facts if still over
        while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.facts.is_empty() {
            ctx.facts.pop();
        }
    }
}

/// Truncate a string to max_chars, appending "..." if truncated.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{}...", truncated)
    }
}
```

**Step 2: Register the module**

In `core/src/thinker/mod.rs`, add after `pub mod memory_context;`:

```rust
pub mod memory_context_provider;
```

And add re-export:

```rust
pub use memory_context_provider::{MemoryContextProvider, MemoryContextConfig};
```

**Step 3: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles (may have warnings about unused imports — fix as needed)

**Step 4: Commit**

```bash
git add core/src/thinker/memory_context_provider.rs core/src/thinker/mod.rs
git commit -m "thinker: add MemoryContextProvider for async memory pre-fetch"
```

---

### Task 6: Wire into Thinker's `build_prompt` path

**Files:**
- Modify: `core/src/thinker/mod.rs:191-286`

**Step 1: Add `memory_context_provider` field to ThinkerConfig**

Find the `ThinkerConfig` struct (search for `pub struct ThinkerConfig`). Add:

```rust
    /// Pre-fetched memory context for current request.
    pub memory_context: Option<MemoryContext>,
```

Initialize to `None` in the Default impl.

**Step 2: Pass memory context through `build_prompt`**

In `build_prompt` (~line 272), modify the `build_system_prompt_with_full_context` call and the `LayerInput` construction path.

Since `build_system_prompt_with_full_context` doesn't accept memory_context, we need to either:
(a) Add a new `build_system_prompt_with_full_context_and_memory` method, or
(b) Add `memory_context` to the existing method signature.

**Approach (b) — extend existing method:**

In `core/src/thinker/prompt_builder/mod.rs`, modify `build_system_prompt_with_full_context` (~line 222):

```rust
    pub fn build_system_prompt_with_full_context(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
        profile: Option<&ProfileConfig>,
        workspace: Option<&WorkspaceFiles>,
        inbound: Option<&InboundContext>,
        memory_context: Option<&super::memory_context::MemoryContext>,
    ) -> String {
        let input = LayerInput::soul(&self.config, tools, soul)
            .with_profile(profile)
            .with_workspace_opt(workspace)
            .with_inbound_opt(inbound)
            .with_memory_context_opt(memory_context);
        self.pipeline.execute(AssemblyPath::Soul, &input)
    }
```

**Step 3: Update all callers of `build_system_prompt_with_full_context`**

Search for all call sites and add the new `memory_context` parameter. The main call site is in `Thinker::build_prompt` (~line 274). Update to:

```rust
            self.prompt_builder.build_system_prompt_with_full_context(
                tools,
                soul,
                self.config.active_profile.as_ref(),
                self.config.workspace_files.as_ref(),
                self.config.inbound_context.as_ref(),
                self.config.memory_context.as_ref(),
            )
```

There may be other call sites (e.g., `build_native_prompt`, line ~550). Search with:

Run: `grep -rn "build_system_prompt_with_full_context" core/src/`

Update all call sites, adding `None` or the appropriate memory_context reference.

**Step 4: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles (fix any additional call sites that the compiler identifies)

**Step 5: Commit**

```bash
git add core/src/thinker/mod.rs core/src/thinker/prompt_builder/mod.rs
git commit -m "thinker: wire MemoryContext through build_prompt path"
```

---

### Task 7: Wire `MemoryContextProvider` into server startup

**Files:**
- Modify: `core/src/bin/aleph/commands/start/builder/handlers.rs`
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Step 1: Add `init_memory_context_provider` function**

In `handlers.rs`, after `init_compression_service`, add:

```rust
// --- init_memory_context_provider -------------------------------------------

pub(in crate::commands::start) fn init_memory_context_provider(
    memory_db: &alephcore::memory::store::MemoryBackend,
    embedder: std::sync::Arc<dyn alephcore::memory::EmbeddingProvider>,
    daemon: bool,
) -> std::sync::Arc<alephcore::thinker::MemoryContextProvider> {
    let provider = alephcore::thinker::MemoryContextProvider::new(
        memory_db.clone(),
        embedder,
    );

    if !daemon {
        println!("Memory context provider initialized for prompt augmentation");
    }

    std::sync::Arc::new(provider)
}
```

**Step 2: Create provider and store in AgentHandlersResult**

In `mod.rs`, inside `register_agent_handlers`, after compression service creation, add memory context provider creation:

```rust
        let memory_ctx_provider: Option<Arc<alephcore::thinker::MemoryContextProvider>> =
            embedder.as_ref().map(|emb| {
                builder::handlers::init_memory_context_provider(
                    memory_db, emb.clone(), daemon,
                )
            });
```

Add `memory_ctx_provider` to `AgentHandlersResult`.

**Step 3: Use provider in ExecutionEngine**

The `MemoryContextProvider` needs to be available where `ThinkerConfig` is constructed before each LLM call. The most practical injection point is the `ExecutionEngine`, which already has the `MemoryBackend`. Add a field:

```rust
    memory_ctx_provider: Option<Arc<crate::thinker::MemoryContextProvider>>,
```

With builder method:

```rust
    pub fn with_memory_ctx_provider(
        mut self,
        provider: Arc<crate::thinker::MemoryContextProvider>,
    ) -> Self {
        self.memory_ctx_provider = Some(provider);
        self
    }
```

In `execute_run`, before creating the Thinker, call:

```rust
        // Pre-fetch memory context for prompt augmentation
        let memory_context = if let Some(ref provider) = self.memory_ctx_provider {
            let query = &request.message; // user's current message
            Some(provider.fetch(query).await)
        } else {
            None
        };
```

Then set it on the ThinkerConfig:

```rust
        thinker_config.memory_context = memory_context;
```

**Step 4: Wire in `register_agent_handlers`**

After engine creation, inject:

```rust
        if let Some(ref mcp) = memory_ctx_provider {
            engine = engine.with_memory_ctx_provider(mcp.clone());
        }
```

**Step 5: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles

**Step 6: Commit**

```bash
git add core/src/bin/aleph/commands/start/builder/handlers.rs \
       core/src/bin/aleph/commands/start/mod.rs \
       core/src/gateway/execution_engine/engine.rs
git commit -m "startup: wire MemoryContextProvider into server and ExecutionEngine"
```

---

### Task 8: Clean up unused `augmentation.rs` module

**Files:**
- Delete: `core/src/memory/augmentation.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Verify no active callers**

Run: `grep -rn "PromptAugmenter\|memory::augmentation\|augmentation::" core/src/ --include="*.rs" | grep -v "mod.rs" | grep -v "augmentation.rs"`

Expected: No results (or only re-exports in mod.rs). If there ARE callers, do NOT delete — update them first.

**Step 2: Remove module declaration and re-export**

In `core/src/memory/mod.rs`:
- Remove `pub mod augmentation;` (~line 23)
- Remove `pub use augmentation::PromptAugmenter;` (~line 83)

**Step 3: Delete the file**

```bash
rm core/src/memory/augmentation.rs
```

**Step 4: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles with no errors. If there are errors about missing `PromptAugmenter`, those callers need updating first.

**Step 5: Commit**

```bash
git add -A core/src/memory/augmentation.rs core/src/memory/mod.rs
git commit -m "memory: remove unused PromptAugmenter module"
```

---

### Task 9: Clean up unused `retrieval.rs` module

**Files:**
- Delete: `core/src/memory/retrieval.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Verify no active callers**

Run: `grep -rn "MemoryRetrieval\b" core/src/ --include="*.rs" | grep -v "mod.rs" | grep -v "retrieval.rs"`

Expected: No results. If there ARE callers, do NOT delete — update them first.

**Step 2: Remove module declaration and re-export**

In `core/src/memory/mod.rs`:
- Remove `pub mod retrieval;` (~line 38)
- Remove `pub use retrieval::MemoryRetrieval;` (~line 105)

**Step 3: Delete the file**

```bash
rm core/src/memory/retrieval.rs
```

**Step 4: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles with no errors

**Step 5: Commit**

```bash
git add -A core/src/memory/retrieval.rs core/src/memory/mod.rs
git commit -m "memory: remove unused MemoryRetrieval module"
```

---

### Task 10: Clean up unused `ai_retrieval.rs` module

**Files:**
- Possibly delete: `core/src/memory/ai_retrieval.rs`
- Modify: `core/src/memory/mod.rs`

**Step 1: Verify no active callers**

Run: `grep -rn "AiMemoryRetriever\|AiMemoryRequest\|AiMemoryResult\|MemoryCandidate\|ai_retrieval" core/src/ --include="*.rs" | grep -v "mod.rs" | grep -v "ai_retrieval.rs"`

If there ARE callers, SKIP this task (do not delete).

**Step 2: If no callers, remove module declaration and re-exports**

In `core/src/memory/mod.rs`:
- Remove `pub mod ai_retrieval;` (~line 19)
- Remove `pub use ai_retrieval::{...};` (~line 66)

**Step 3: Delete the file**

```bash
rm core/src/memory/ai_retrieval.rs
```

**Step 4: Verify it compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: compiles with no errors

**Step 5: Commit**

```bash
git add -A core/src/memory/ai_retrieval.rs core/src/memory/mod.rs
git commit -m "memory: remove unused AiMemoryRetriever module"
```

---

### Task 11: End-to-end verification

**Step 1: Full compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: clean compile

**Step 2: Run all thinker tests**

Run: `cargo test -p alephcore --lib thinker 2>&1 | tail -30`
Expected: all tests pass

**Step 3: Run memory tests**

Run: `cargo test -p alephcore --lib memory 2>&1 | tail -30`
Expected: all tests pass (modulo pre-existing failures in `markdown_skill`)

**Step 4: Run pipeline tests specifically**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline 2>&1 | tail -20`
Expected: all tests pass, layer count = 26

**Step 5: Start the server**

Run: `cargo run --bin aleph 2>&1 | head -50`

Verify output includes:
- `Memory context provider initialized for prompt augmentation` (if embedding provider configured)
- OR no error if embedding not configured (graceful degradation)
