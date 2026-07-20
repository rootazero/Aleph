# Agent Runtime: Three-Layer Dispatch + Prompt Fork Caching

**Date**: 2026-04-06
**Status**: Approved
**Scope**: agent_loop module refactoring + prompt cache optimization

---

## Problem Statement

Aleph's current subagent dispatch is a two-layer structure: SubagentTool directly calls `run_subagent()` (a bare function in subagent_runner.rs), which constructs and runs an AgentLoop. This design has two deficiencies:

1. **No lifecycle management**: No transcript recording, no hook events (SubagentStart/End), no structured cleanup. Debugging subagent failures requires log grep.
2. **No prompt cache reuse**: Each subagent rebuilds the full system prompt from scratch. For Anthropic API, the stable portion (~60-70% of prompt) could be cache-hit if the prefix bytes match the parent's prompt. Current design wastes this opportunity.

### Reference: Claude Code's Approach

Claude Code uses a three-layer architecture: `AgentTool.call()` (dispatch/routing) → `runAgent()` (context construction + lifecycle) → `query()` (model loop). Key innovations:
- Fork path: subagents share the parent's system prompt prefix (byte-identical) to maximize Anthropic prompt cache hits
- Lifecycle management: transcript recording, metadata writing, hook execution, cleanup
- Specialization: 6 built-in agents with distinct prompt/tool constraints

---

## Design: Three-Layer Architecture

```
SubagentTool  (dispatch layer)
  │  Unchanged: parse_args(), schema(), LoopTool impl
  │  Changed: Run branch constructs AgentRuntime instead of calling run_subagent()
  │
  │  AgentRuntimeConfig
  ▼
AgentRuntime  (lifecycle layer) — NEW
  │  1. Prompt strategy: fork path (reuse snapshot) vs fresh path
  │  2. Tool registry filtering (agent allowed/denied tools)
  │  3. Lifecycle events (SubagentStart, SubagentEnd)
  │  4. Transcript recording (structured tracing)
  │  5. Timeout + cleanup
  │
  │  AgentLoop::new(...) + .run()
  ▼
AgentLoop  (execution layer) — UNCHANGED
  │  think→act loop, unaware of being a subagent
```

### Key Decisions

- **AgentRuntime is a struct, not a trait** — single implementation, no over-abstraction (P6 Simplicity)
- **AgentRuntime owns all resources needed to construct AgentLoop** — provider, tool_registry_factory, safety_guard_factory
- **SubagentTool passes decisions via AgentRuntimeConfig** — fork/fresh, model override, timeout
- **AgentLoop has zero changes to its think→act loop** — only gains an optional SharedSnapshot field

---

## AgentRuntime Detailed Design

### Core Structures

```rust
/// Sub-agent runtime — manages the full lifecycle from construction to cleanup.
///
/// All types defined in `src/agent_loop/agent_runtime.rs`.
pub struct AgentRuntime {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: ChainContext,
    cancel_token: CancellationToken,
}

/// Per-run configuration, constructed by SubagentTool dispatch layer.
pub struct AgentRuntimeConfig {
    pub agent_def: AgentDef,
    pub task: String,
    pub context_summary: Option<String>,
    pub model: Option<String>,
    pub timeout_secs: u64,
    pub prompt_snapshot: Option<PromptSnapshot>,
}
```

### Lifecycle Phases

```
AgentRuntime::run(config) -> Result<LoopRunResult, String>

Phase 1 — Preparation:
  1. Resolve model: config.model > agent_def.model_hint > default
  2. Build tool registry + filter by agent_def.is_tool_allowed()
  3. Build prompt:
     - if config.prompt_snapshot.is_some() → fork path
     - else → fresh path
  4. Build LoopConfig from agent_def

Phase 2 — Lifecycle Events:
  5. Emit SubagentStart tracing span (agent_id, agent_type, task summary)

Phase 3 — Execution:
  6. Create AgentLoop
  7. Prepare effective_task (inject context_summary if present)
  8. tokio::time::timeout(duration, agent_loop.run(&effective_task))

Phase 4 — Cleanup (always runs):
  9. Record transcript via structured tracing
  10. Emit SubagentEnd tracing span (agent_id, outcome, duration_ms, iterations)
  11. Return result
```

### Transcript

```rust
pub struct SubagentTranscript {
    pub agent_id: String,
    pub agent_type: String,
    pub task_summary: String,       // first 200 chars
    pub outcome: TranscriptOutcome, // Success / Error / Timeout
    pub iterations: usize,
    pub duration_ms: u64,
    pub tokens_used: usize,
}

pub enum TranscriptOutcome {
    Success,
    Error(String),
    Timeout,
}
```

Transcript output via `tracing::info!` structured logging. No separate persistence (P6 simplicity).

---

## Prompt Fork Caching

### PromptSnapshot

```rust
/// Parent agent's stable prompt snapshot for fork path reuse.
pub struct PromptSnapshot {
    /// Stable layers assembly output (excludes agent role, session context, etc.)
    pub stable_prefix: String,
    /// Source assembly path
    pub path: AssemblyPath,
}
```

### Fork Path vs Fresh Path

**Fork path** (snapshot present):
```
prompt = snapshot.stable_prefix + pipeline.execute_dynamic_only(path, input_with_agent_def)
```
Reuses parent's stable layers output (Bootstrap, Role, Guidelines, Tools, etc.), only rebuilds dynamic layers (AgentRole, SessionContext, Memory, etc.).

**Fresh path** (no snapshot):
```
prompt = pipeline.execute(path, input_with_agent_def)
```
Identical to current subagent_runner.rs behavior.

### Fork Trigger Conditions

SubagentTool dispatch layer decides whether to pass a snapshot:

```
Fork conditions (ALL must be true):
  1. No agent_type specified (using "default" agent)
  2. No model override specified (same provider)
  3. Not in team mode (no team_name)
  4. Parent agent has a valid PromptSnapshot
```

Any condition not met → fresh path. Conservative strategy, safety first.

### PromptBuilder Changes

Two new methods:

```rust
impl PromptBuilder {
    /// Capture current stable layers output as a snapshot.
    pub fn capture_snapshot(&self, tools: &[ToolInfo]) -> PromptSnapshot;

    /// Build sub-agent prompt from snapshot (fork path).
    pub fn build_from_snapshot(
        &self,
        snapshot: &PromptSnapshot,
        agent_def: &AgentDef,
        tools: &[ToolInfo],
    ) -> String;
}
```

### Provider Benefits

- **Anthropic**: stable_prefix is byte-identical to parent's system prompt prefix → automatic prompt cache hit (cache_control: ephemeral already set). No provider layer changes needed.
- **OpenAI / Gemini / Others**: No prompt caching mechanism, but fork path still saves CPU time (skips 30+ stable layers inject computation).

---

## Snapshot Injection Chain

### Shared Reference Pattern

```rust
pub type SharedSnapshot = Arc<RwLock<Option<PromptSnapshot>>>;
```

**Injection flow:**
```
run_loop.rs:
  shared_snapshot = Arc::new(RwLock::new(None))
  → inject into SubagentTool (read)
  → inject into AgentLoop (write)

AgentLoop think step:
  After building system prompt, capture snapshot once:
  if snapshot is None → capture and store

SubagentTool execute (Run branch):
  Read current snapshot from shared reference
```

### Lock Safety

- `unwrap_or_else(|e| e.into_inner())` for poison handling (P7 Defensive Design)
- Single writer (main loop think step), multiple readers (SubagentTool execute)
- RwLock naturally supports many-readers-one-writer

### Snapshot Invalidation

When PromptConfig changes during a session (e.g., skill_mode toggle), the main loop resets the snapshot to None. Next think step recaptures. This ensures snapshot consistency with current config, with automatic fallback to fresh path.

---

## Change Manifest

### New Files

| File | Est. Lines | Content |
|------|-----------|---------|
| `src/agent_loop/agent_runtime.rs` | ~250 | AgentRuntime, AgentRuntimeConfig, PromptSnapshot, SubagentTranscript |

### Modified Files

| File | Change | Est. Lines |
|------|--------|-----------|
| `src/agent_loop/subagent_tool.rs` | Run branch uses AgentRuntime; add shared_snapshot + should_fork() | ~40 changed |
| `src/agent_loop/loop_core.rs` | Add shared_snapshot field + with_shared_snapshot() + capture in think step | ~15 added |
| `src/agent_loop/mod.rs` | Add `pub mod agent_runtime;`, remove subagent_runner export | ~3 changed |
| `src/thinker/prompt_builder/mod.rs` | Add capture_snapshot() + build_from_snapshot() | ~30 added |
| `src/gateway/execution_engine/run_loop.rs` | Construct SharedSnapshot, inject into SubagentTool and AgentLoop | ~10 changed |

### Deleted Files

| File | Reason |
|------|--------|
| `src/agent_loop/subagent_runner.rs` | Logic fully migrated to agent_runtime.rs |

### Untouched

- `loop_core.rs` think→act loop
- `prompt_pipeline.rs` (stable/dynamic execute methods already exist)
- `prompt_layer.rs` (LayerStability enum already exists)
- All layer implementations (`src/thinker/layers/*`)
- All providers (`src/providers/*`)
- Agent definitions (`src/agents/*`)

### Totals

- **Added**: ~280 lines
- **Modified**: ~70 lines
- **Deleted**: ~98 lines (subagent_runner.rs)
- **Net**: ~180 lines

---

## Risk Assessment

| Risk | Level | Mitigation |
|------|-------|-----------|
| subagent_runner deletion breaks compilation | Low | Incremental migration: implement in agent_runtime.rs first, verify compilation, then delete |
| Snapshot diverges from actual prompt | Low | Conservative invalidation: any config change resets to None, fallback to fresh path |
| SharedSnapshot lock contention | Very Low | Single-writer-multiple-reader pattern, RwLock is ideal |
| Existing 25 SubagentTool tests break | Low | parse/schema tests unaffected; only execute path changes |

---

## Test Strategy

- **SubagentTool**: Existing 25 parse/schema tests unchanged
- **AgentRuntime**: New unit tests for fork path prompt assembly, fresh path prompt assembly, timeout handling, lifecycle event emission
- **PromptBuilder**: New tests for capture_snapshot() and build_from_snapshot() — verify fork output = stable_prefix + dynamic_only
- **Integration**: Verify SubagentTool → AgentRuntime → AgentLoop end-to-end with mock provider

---

## Future Work (Out of Scope)

These are enabled by the three-layer architecture but not part of this spec:

- **Agent-specific MCP servers**: AgentRuntime starts/stops MCP servers per agent lifetime
- **Agent-scoped hooks**: HookExecutor filtering by agent_id context
- **Agent observability**: Structured transcript persistence, Perfetto integration
- **Worktree isolation**: AgentRuntime manages git worktree lifecycle
