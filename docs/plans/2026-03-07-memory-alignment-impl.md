# Memory Alignment Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add agent_id-scoped workspace filtering to LanceDB memory queries and wire daily memory file loading into the agent execution loop.

**Architecture:** `MemoryContextProvider` is a shared singleton — pass `agent_id` into `fetch()` rather than storing it on the struct. Daily memory files from `workspace/memory/YYYY-MM-DD.md` are loaded via the existing `WorkspaceFileLoader` and merged into `MemoryContext` as a new `daily_notes` field.

**Tech Stack:** Rust, LanceDB, tokio, chrono

**Design doc:** `docs/plans/2026-03-07-memory-alignment-dual-directory-design.md`

---

### Task 1: Add agent_id parameter to MemoryContextProvider.fetch()

Add workspace filtering to LanceDB searches so each agent only sees its own memories.

**Files:**
- Modify: `core/src/thinker/memory_context_provider.rs`

**Step 1: Update `fetch()` signature to accept agent_id**

Change `fetch(&self, query: &str)` to `fetch(&self, query: &str, agent_id: &str)`:

```rust
/// Fetch relevant memory context for a user query, scoped to a specific agent.
///
/// Returns empty context on any failure (never blocks LLM calls).
pub async fn fetch(&self, query: &str, agent_id: &str) -> MemoryContext {
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

    // 2. Search facts and memories in parallel (scoped to agent workspace)
    let facts_future = self.search_facts(&embedding, dim, agent_id);
    let memories_future = self.search_memories(&embedding, agent_id);

    let (facts, memories) = tokio::join!(facts_future, memories_future);

    // 3. Build context
    let mut ctx = MemoryContext {
        facts: facts.unwrap_or_default(),
        memory_summaries: memories.unwrap_or_default(),
        ..Default::default()
    };

    // 4. Truncate to character budget
    self.truncate_to_budget(&mut ctx);

    debug!(
        facts = ctx.facts.len(),
        memories = ctx.memory_summaries.len(),
        agent_id = agent_id,
        "Memory context fetched for prompt augmentation"
    );

    ctx
}
```

**Step 2: Add agent_id param to `search_facts()` and apply workspace filter**

```rust
async fn search_facts(
    &self,
    embedding: &[f32],
    dim: u32,
    agent_id: &str,
) -> Result<Vec<ScoredFact>, ()> {
    use crate::gateway::workspace::WorkspaceFilter;

    let filter = SearchFilter::new()
        .with_workspace(WorkspaceFilter::Single(agent_id.to_string()));
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
```

**Step 3: Add agent_id param to `search_memories()` and apply workspace filter**

```rust
async fn search_memories(
    &self,
    embedding: &[f32],
    agent_id: &str,
) -> Result<Vec<MemorySummary>, ()> {
    use crate::gateway::workspace::WorkspaceFilter;

    let filter = MemoryFilter {
        workspace: Some(WorkspaceFilter::Single(agent_id.to_string())),
        ..Default::default()
    };
    // ... rest unchanged
}
```

**Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -40`
Expected: Compilation errors in callers (ExecutionEngine) — fixed in Task 2.

---

### Task 2: Update ExecutionEngine caller to pass agent_id

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs:748-754`

**Step 1: Pass agent_id to provider.fetch()**

At line 748-754, update the `memory_context` block. The `agent_id` is already resolved at line 644 (`let agent_id = request.session_key.agent_id().to_string();`):

```rust
// Pre-fetch LanceDB memory context for prompt augmentation
let memory_context = if let Some(ref provider) = self.memory_context_provider {
    let ctx = provider.fetch(&request.input, &agent_id).await;
    if ctx.is_empty() { None } else { Some(ctx) }
} else {
    None
};
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Clean compilation (no errors).

**Step 3: Run existing tests**

Run: `cargo test -p alephcore --lib memory_context 2>&1 | tail -20`
Expected: All existing tests pass.

**Step 4: Commit**

```bash
git add core/src/thinker/memory_context_provider.rs core/src/gateway/execution_engine/engine.rs
git commit -m "memory: add agent_id workspace filtering to MemoryContextProvider"
```

---

### Task 3: Add daily_notes field to MemoryContext

**Files:**
- Modify: `core/src/thinker/memory_context.rs`

**Step 1: Add import for DailyMemory**

At the top of the file, add:
```rust
use crate::gateway::workspace_loader::DailyMemory;
```

**Step 2: Add daily_notes field to MemoryContext struct**

```rust
#[derive(Debug, Clone, Default)]
pub struct MemoryContext {
    /// Layer 2 facts (compressed knowledge), sorted by relevance.
    pub facts: Vec<ScoredFact>,
    /// Layer 1 memory summaries (raw conversation excerpts).
    pub memory_summaries: Vec<MemorySummary>,
    /// Daily memory notes from workspace/memory/YYYY-MM-DD.md files.
    pub daily_notes: Vec<DailyMemory>,
}
```

**Step 3: Update `is_empty()` to include daily_notes**

```rust
pub fn is_empty(&self) -> bool {
    self.facts.is_empty() && self.memory_summaries.is_empty() && self.daily_notes.is_empty()
}
```

**Step 4: Append daily notes section in `format_for_prompt()`**

Add after the existing `memory_summaries` block (before the final `output`):

```rust
if !self.daily_notes.is_empty() {
    output.push_str("**Recent Notes:**\n");
    for note in &self.daily_notes {
        output.push_str(&format!("### {}\n{}\n\n", note.date, note.content));
    }
}
```

**Step 5: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Clean compilation. The `..Default::default()` in Task 1's `fetch()` handles the new field.

**Step 6: Commit**

```bash
git add core/src/thinker/memory_context.rs
git commit -m "memory: add daily_notes field to MemoryContext"
```

---

### Task 4: Wire daily memory loading into ExecutionEngine

Load recent daily memory files and merge them into the MemoryContext during `run_agent_loop()`.

**Files:**
- Modify: `core/src/gateway/execution_engine/engine.rs:748-754`

**Step 1: Load daily notes after LanceDB fetch**

Replace the memory_context block (lines 748-754) with:

```rust
// Pre-fetch LanceDB memory context for prompt augmentation
let mut memory_context = if let Some(ref provider) = self.memory_context_provider {
    let ctx = provider.fetch(&request.input, &agent_id).await;
    if ctx.is_empty() { None } else { Some(ctx) }
} else {
    None
};

// Load recent daily memory notes from workspace/memory/*.md
{
    let daily_notes = {
        let mut loader = self.workspace_loader.lock().unwrap_or_else(|e| e.into_inner());
        loader.load_recent_memory(&agent_workspace_dir, 7)
    };
    if !daily_notes.is_empty() {
        let ctx = memory_context.get_or_insert_with(Default::default);
        ctx.daily_notes = daily_notes;
    }
}
```

Note: `agent_workspace_dir` is already defined at line 698 as `agent.config().workspace.clone()`.

**Step 2: Update truncation in MemoryContextProvider to handle daily_notes**

In `core/src/thinker/memory_context_provider.rs`, update `truncate_to_budget()`:

```rust
fn truncate_to_budget(&self, ctx: &mut MemoryContext) {
    // Remove daily notes first (lowest priority), then memories, then facts
    while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.daily_notes.is_empty() {
        ctx.daily_notes.pop();
    }
    while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.memory_summaries.is_empty() {
        ctx.memory_summaries.pop();
    }
    while ctx.format_for_prompt().len() > self.config.max_output_chars && !ctx.facts.is_empty() {
        ctx.facts.pop();
    }
}
```

**Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Clean compilation.

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add core/src/gateway/execution_engine/engine.rs core/src/thinker/memory_context_provider.rs
git commit -m "memory: wire daily memory loading into agent loop"
```

---

### Task 5: Add tests for workspace-filtered memory context

**Files:**
- Modify: `core/src/thinker/memory_context_provider.rs` (add to existing `#[cfg(test)]` if present, or add new test module)
- Modify: `core/src/thinker/memory_context.rs` (add tests for daily_notes formatting)

**Step 1: Add test for MemoryContext with daily_notes formatting**

In `core/src/thinker/memory_context.rs`, add a test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::workspace_loader::DailyMemory;

    #[test]
    fn test_empty_context() {
        let ctx = MemoryContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.format_for_prompt(), "");
    }

    #[test]
    fn test_daily_notes_not_empty() {
        let ctx = MemoryContext {
            daily_notes: vec![DailyMemory {
                date: "2026-03-07".to_string(),
                content: "Test note".to_string(),
            }],
            ..Default::default()
        };
        assert!(!ctx.is_empty());
    }

    #[test]
    fn test_daily_notes_format() {
        let ctx = MemoryContext {
            daily_notes: vec![
                DailyMemory {
                    date: "2026-03-07".to_string(),
                    content: "Morning standup notes".to_string(),
                },
                DailyMemory {
                    date: "2026-03-06".to_string(),
                    content: "Debug session log".to_string(),
                },
            ],
            ..Default::default()
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("Recent Notes"));
        assert!(prompt.contains("2026-03-07"));
        assert!(prompt.contains("Morning standup notes"));
        assert!(prompt.contains("2026-03-06"));
        assert!(prompt.contains("Debug session log"));
    }

    #[test]
    fn test_mixed_context_format() {
        let fact = ScoredFact {
            fact: crate::memory::context::MemoryFact::new("test-fact", "Rust is great"),
            score: 0.9,
        };
        let ctx = MemoryContext {
            facts: vec![fact],
            memory_summaries: vec![],
            daily_notes: vec![DailyMemory {
                date: "2026-03-07".to_string(),
                content: "Daily note".to_string(),
            }],
        };
        let prompt = ctx.format_for_prompt();
        assert!(prompt.contains("Facts"));
        assert!(prompt.contains("Recent Notes"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib memory_context 2>&1 | tail -20`
Expected: All tests pass.

**Step 3: Commit**

```bash
git add core/src/thinker/memory_context.rs
git commit -m "memory: add tests for daily_notes in MemoryContext"
```

---

### Task 6: Final verification

**Step 1: Full compilation check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: Clean compilation.

**Step 2: Run all library tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass (same count as before + new tests).

**Step 3: Squash commit if needed**

If all tasks committed individually, no squash needed. Final state should have these commits:
1. `memory: add agent_id workspace filtering to MemoryContextProvider`
2. `memory: add daily_notes field to MemoryContext`
3. `memory: wire daily memory loading into agent loop`
4. `memory: add tests for daily_notes in MemoryContext`
