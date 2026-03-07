# Memory Alignment for Dual-Directory Architecture

> Audit and enhance memory system paths after agent-workspace separation.

## Background

Agent-workspace separation introduced dual directories:
- `~/.aleph/agents/{id}/` — runtime state (sessions/)
- `~/.aleph/workspaces/{id}/` — content (SOUL.md, AGENTS.md, MEMORY.md, memory/)

The memory system uses a global LanceDB backend at `~/.aleph/data/` and a global SessionManager SQLite at `~/.aleph/data/sessions.db`. This design validates existing paths and wires two missing integrations.

## Section 1: Overall Strategy

| Component | Current State | Action |
|-----------|--------------|--------|
| LanceDB | Global `~/.aleph/data/`, single instance | Keep global, add agent_id filtering |
| SessionManager | Global `~/.aleph/data/sessions.db` | Keep global, no changes |
| MEMORY.md | Loaded from `workspace_path` | Already correct |
| `workspace/memory/` | Created by `initialize_workspace` | Already correct |
| Daily memory loading | `load_recent_memory()` exists but never called at runtime | Wire into agent loop |
| `agents/{id}/sessions/` | Created by `initialize_agent_dir` | Reserved for future session export |

## Section 2: LanceDB Workspace Column Filtering

### Problem

`MemoryContextProvider.fetch()` uses `SearchFilter::default()` and `MemoryFilter::default()` with `workspace = None`, meaning all agents share the same memory pool without isolation.

### Solution

Add `agent_id` to `MemoryContextProvider` and set workspace filter on all searches:

```rust
pub struct MemoryContextProvider {
    memory_db: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    config: MemoryContextConfig,
    agent_id: String,  // NEW
}
```

In `search_facts()`:
```rust
let filter = SearchFilter::new().with_workspace(&self.agent_id);
```

In `search_memories()`:
```rust
let filter = MemoryFilter { workspace: Some(WorkspaceFilter::exact(&self.agent_id)), ..Default::default() };
```

Callers (server startup) pass the current agent ID when constructing the provider.

## Section 3: Daily Memory Runtime Loading

### Problem

`WorkspaceFileLoader.load_recent_memory()` reads `workspace/memory/YYYY-MM-DD.md` files but is never called during agent execution.

### Solution

Wire into `ExecutionEngine.run_agent_loop()`, after the existing LanceDB fetch:

```
run_agent_loop()
  -> memory_context_provider.fetch(query)           // existing: LanceDB vector search
  -> workspace_loader.load_recent_memory(ws, 7)     // NEW: recent 7 days .md files
  -> merge into MemoryContext.daily_notes            // NEW field
  -> ThinkerConfig { memory_context, ... }
  -> MemoryAugmentationLayer.inject()                // append "## Recent Notes"
```

Changes:
- `MemoryContext`: add `daily_notes: Vec<DailyMemory>` field
- `MemoryContext.format_for_prompt()`: append `## Recent Notes` section
- `ExecutionEngine`: use `workspace_path` from active workspace to call `load_recent_memory`
- Default: 7 days, truncated within character budget

## Section 4: Session Directory

`~/.aleph/agents/{id}/sessions/` is already created by `initialize_agent_dir()`. No session export functionality exists yet. Directory is reserved for future use. No code changes needed.
