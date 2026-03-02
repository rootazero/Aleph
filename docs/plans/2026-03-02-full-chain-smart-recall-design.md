# Full Chain Completion + Smart Recall Design

## Goal

Complete the workspace wiring chain (3 remaining disconnections) and implement
Two-Phase Smart Recall for automatic cross-workspace knowledge association.

## Context

The workspace system is ~85% wired: WorkspaceManager, ProfileConfig, ActiveWorkspace
resolution, Thinker integration, and memory filtering all work. Three gaps remain
in tool-level workspace propagation. Beyond fixing these, we add Smart Recall —
Aleph's unique advantage over OpenClaw's fully-isolated per-agent memory.

## Architecture

Two-part evolution:
1. **Full Chain** — fix 3 disconnections so workspace switching is 100% effective
2. **Smart Recall** — Two-Phase Retrieval that auto-expands to cross-workspace
   search when primary results are sparse

---

## Part 1: Full Chain Completion

### 1.1 Memory Search Tool Runtime Workspace Injection

**Problem:** ExecutionEngine resolves `active_workspace.workspace_id` but the
memory_search tool's `default_workspace` handle still reads "default".

**Solution:** After resolving ActiveWorkspace in `run_agent_loop()`, propagate
`workspace_id` to the tool executor. The memory_search tool already has a
`default_workspace_handle()` returning `Arc<RwLock<String>>` — the engine needs
to write to it.

**Approach:** Add a `set_workspace_context(workspace_id)` method to the builtin
tool registry or executor that updates all workspace-aware tools.

**Files:**
- `core/src/gateway/execution_engine/engine.rs` — call set after workspace resolution
- `core/src/executor/` — add workspace propagation method
- `core/src/builtin_tools/memory_search.rs` — already has the handle, just needs wiring

### 1.2 Workspace CRUD RPC Registration

**Problem:** Only `workspace.switch` and `workspace.getActive` are registered.
Missing: create, list, get, update, archive.

**Solution:** Check which handlers exist in `gateway/handlers/workspace.rs`,
add any missing ones, then register them all in `builder/handlers.rs`.

**Files:**
- `core/src/gateway/handlers/workspace.rs` — verify/add CRUD handlers
- `core/src/bin/aleph_server/commands/start/builder/handlers.rs` — register all

### 1.3 Tool-Chain Workspace Propagation

**Problem:** `workspace_id` is buried in `RequestContext.metadata["workspace_id"]`.
Tools can't directly access it.

**Solution:** Add `workspace_id: Option<String>` to the tool execution context
(ToolContext or equivalent). The executor populates it from loop state metadata.
Tools that need workspace context (memory_search, memory_store, etc.) read it
directly rather than relying on the default_workspace handle alone.

**Files:**
- `core/src/executor/` — add workspace_id to tool context
- `core/src/builtin_tools/` — update workspace-aware tools to read from context

---

## Part 2: Smart Recall — Two-Phase Retrieval

### Overview

When a user queries memory in their current workspace, Smart Recall automatically
detects when results are sparse or low-relevance and expands the search across
all workspaces. Cross-workspace results are tagged with their source workspace
so the LLM can naturally reference cross-domain knowledge.

### 2.1 SmartRecallConfig

Added to `ProfileConfig` so each workspace profile can tune recall behavior:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartRecallConfig {
    /// Enable automatic cross-workspace recall
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Phase 2 triggers when top result score is below this threshold
    #[serde(default = "default_score_threshold")]
    pub score_threshold: f32,  // default: 0.60

    /// Phase 2 triggers when primary result count is below this
    #[serde(default = "default_min_results")]
    pub min_primary_results: usize,  // default: 2

    /// Max cross-workspace results to include
    #[serde(default = "default_max_cross")]
    pub max_cross_results: usize,  // default: 3
}
```

**Files:**
- `core/src/config/types/profile.rs` — add SmartRecallConfig field to ProfileConfig

### 2.2 FactRetrieval Extension

New method on FactRetrieval:

```rust
pub async fn retrieve_with_smart_recall(
    &self,
    query: &str,
    primary_workspace: &str,
    config: &SmartRecallConfig,
) -> Result<SmartRetrievalResult, AlephError>
```

**Logic:**
1. Phase 1: `retrieve_with_filter(query, WorkspaceFilter::Single(primary_workspace))`
2. Check trigger conditions:
   - `top_score < config.score_threshold` OR
   - `result_count < config.min_primary_results`
3. If triggered, Phase 2: `retrieve_with_filter(query, WorkspaceFilter::All)`
   - Exclude results from primary_workspace (already have them)
   - Take top `config.max_cross_results`
   - Tag each with source workspace

**Return type:**
```rust
pub struct SmartRetrievalResult {
    /// Primary workspace results
    pub primary: RetrievalResult,
    /// Cross-workspace results (empty if recall not triggered)
    pub cross_workspace: Vec<CrossWorkspaceFact>,
    /// Whether Phase 2 was triggered
    pub recall_triggered: bool,
    /// Reason for trigger (for logging/debugging)
    pub trigger_reason: Option<String>,
}

pub struct CrossWorkspaceFact {
    pub content: String,
    pub source_workspace: String,
    pub relevance_score: f32,
}
```

**Files:**
- `core/src/memory/fact_retrieval.rs` — add method + types
- `core/src/memory/workspace.rs` — may need to expose workspace field from search results

### 2.3 Memory Search Tool Integration

Update `memory_search` tool's `call_impl()`:

1. Read SmartRecallConfig from tool context (profile → config)
2. If smart_recall enabled AND not explicit cross_workspace request:
   - Use `retrieve_with_smart_recall()` instead of `retrieve_with_filter()`
3. Format cross-workspace results with source tags:
   ```
   [from: health] Deep work requires 90-minute uninterrupted blocks
   [from: reading] "Deep Work" by Cal Newport: time-blocking is key
   ```
4. Include in tool output so LLM sees cross-domain context

**Files:**
- `core/src/builtin_tools/memory_search.rs` — update call_impl

### 2.4 SmartRecallConfig Flow

```
config.toml
  └─ profiles.coding.smart_recall = { enabled: true, score_threshold: 0.55 }

WorkspaceManager.load_profiles()
  └─ ProfileConfig { smart_recall: SmartRecallConfig { ... } }

ActiveWorkspace.from_manager()
  └─ profile.smart_recall → stored in ActiveWorkspace

ExecutionEngine.run_agent_loop()
  └─ active_workspace.profile.smart_recall → tool context

memory_search.call_impl()
  └─ if smart_recall.enabled → retrieve_with_smart_recall()
     └─ Phase 1 → Phase 2 (if needed) → tagged results
```

---

## Data Flow Example

```
User: "写代码时怎么保持专注？" (in "coding" workspace)

1. memory_search tool called with query="写代码 保持专注"
2. Phase 1: Search "coding" workspace
   → 1 result: "Pomodoro technique for coding" (score: 0.52)
3. Phase 2 triggered: score 0.52 < threshold 0.60, count 1 < min 2
4. Expand to ALL workspaces:
   → "health": "深度工作需要90分钟无干扰" (score: 0.72)
   → "reading": "《Deep Work》核心观点" (score: 0.68)
5. Tool returns:
   Primary: "Pomodoro technique for coding"
   Cross-workspace:
     [from: health] 深度工作需要90分钟无干扰，冥想有助恢复注意力
     [from: reading] 《Deep Work》核心观点：远离社交媒体，时间分块

6. LLM response: "关于保持专注，结合你在健康和阅读方面的知识..."
```

---

## Trade-offs

| Aspect | Decision | Rationale |
|--------|----------|-----------|
| Trigger mechanism | Score + count threshold | Avoids unnecessary cross-WS queries |
| Extra embedding cost | Only when Phase 2 fires | Amortized; most queries stay in Phase 1 |
| Result tagging | `[from: workspace]` prefix | LLM-friendly, transparent |
| Config granularity | Per-profile | Different workspaces may want different sensitivity |
| OpenClaw comparison | Unique capability | OpenClaw has zero cross-agent memory |

## Non-goals (this iteration)

- Knowledge graph / fact-to-fact semantic linking
- Explicit knowledge migration between workspaces
- Memory time-decay / compression
- Timeline navigation

These can be built later on top of the Smart Recall foundation.
