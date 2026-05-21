# Unified Agent Dispatch Chain

> Date: 2026-04-01
> Status: Approved
> Scope: src/agent_loop/, src/agents/

## Problem

Aleph has three separate agent dispatch mechanisms that are not integrated:

1. **TaskTool** (legacy) — fire-and-forget via event bus, no direct result
2. **SubAgentDispatcher + Coordinator** — capability-based routing with sync wait
3. **SubagentTool** — nested AgentLoop with depth tracking

Meanwhile Claude Code has a unified three-layer architecture (AgentTool → runAgent → query) with specialized built-in agents, fork/context inheritance, background lifecycle, and per-agent MCP servers.

Aleph's SubagentTool is the strongest foundation but lacks: agent type selection, context inheritance, background execution, and model override.

## Decision

**Approach A: Incremental refactoring** — enhance SubagentTool as the single unified entry point, leverage existing AgentRegistry for role definitions, deprecate and remove all other dispatch paths.

## Design

### 1. Extended SubagentTool Schema

```json
{
    "task": "string (required)",
    "agent_type": "string (optional) — role from AgentRegistry, default: 'default'",
    "model": "string (optional) — override provider, e.g. 'fast'/'thinking'",
    "timeout_secs": "integer (optional, default: 120)",
    "run_in_background": "bool (optional, default: false)",
    "context_summary": "string (optional) — parent-provided context for the sub-agent"
}
```

### 2. Three-Layer Dispatch Flow

```
SubagentTool::execute()              — dispatch controller
    ├─ parse args
    ├─ AgentRegistry::get(agent_type) → AgentDef
    ├─ ChainContext depth check
    ├─ foreground/background split
    │
    ├─ foreground → run_subagent()   — runtime constructor
    │                 ├─ build PromptBuilder (AgentDef.system_prompt + context_summary)
    │                 ├─ build LoopToolRegistry (filtered by AgentDef allow/deny)
    │                 ├─ optional provider override (model param)
    │                 ├─ create AgentLoop
    │                 └─ AgentLoop::run()  — main loop (unchanged)
    │
    └─ background → tokio::spawn(run_subagent())
                     ├─ immediate return { request_id, status: "running_in_background" }
                     └─ on completion → AlephEvent::SubAgentCompleted
```

AgentLoop::run() is NOT modified. Only how it is constructed and launched changes.

### 3. AgentDef Extensions

```rust
pub struct AgentDef {
    // existing
    pub id: String,
    pub mode: AgentMode,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    pub max_iterations: Option<u32>,
    // new
    pub token_budget: Option<u32>,
    pub model_hint: Option<String>,
    pub context_mode: ContextMode,
}

pub enum ContextMode {
    Fresh,    // no parent context
    Summary,  // receives parent-provided summary
}
```

### 4. Built-in Agent Roles

| ID | Prompt Core | Tool Allowlist | context_mode |
|----|-------------|----------------|-------------|
| `default` | General sub-agent | all (minus subagent) | Summary |
| `explore` | Read-only code explorer | glob, grep, read_file, bash(ro) | Fresh |
| `plan` | Read-only planner | glob, grep, read_file, bash(ro) | Summary |
| `verify` | Adversarial verifier | glob, grep, read_file, bash, test | Summary |
| `coder` | Coding executor | read, write, edit, bash, glob, grep | Summary |

Users can register custom roles via JSON files in `~/.aleph/agents/` or via tools at runtime (R9).

### 5. Tool Filtering Logic

```
parent registry tools
    ├─ remove "subagent" (unless AgentDef allows + depth not maxed)
    ├─ if allowed_tools non-empty → keep only whitelist
    ├─ remove denied_tools (takes precedence)
    └─ produce child LoopToolRegistry
```

### 6. Context Summary Injection

- Summary provided by parent LLM via `context_summary` parameter
- Injected as a PromptBuilder section: `## Context from parent agent\n\n{summary}`
- If `context_summary` is provided, it is always injected regardless of `context_mode` — `ContextMode` is a hint to the parent LLM via schema description, not a hard gate
- No automatic summarization (zero middleware tax — R10)
- No length limit (trust LLM judgment)

### 7. Background Agent (Basic)

```rust
pub struct BackgroundAgentTracker {
    running: RwLock<HashMap<String, RunningAgent>>,
    completed: RwLock<HashMap<String, CompletedAgent>>,
}
```

- Register with CancellationToken on spawn
- Publish `AlephEvent::SubAgentCompleted` on finish
- TTL-based cleanup for completed entries
- NOT in scope: foreground/background switching, progress tracking, transcript recording

### 8. Code Cleanup

**Delete:**

| File | Reason |
|------|--------|
| `agents/task_tool.rs` | Replaced by background agent |
| `agents/sub_agents/delegate_tool.rs` | Replaced by AgentDef tool filtering |
| `agents/sub_agents/dispatcher.rs` | Routing moves to SubagentTool + AgentRegistry |
| `agents/sub_agents/coordinator.rs` | Sync wait moves to BackgroundAgentTracker |
| `agents/sub_agents/result_collector.rs` | Covered by AgentLoop RunResult |
| `agents/sub_agents/result_merger.rs` | Same |
| `agents/sub_agents/mcp_agent.rs` | MCP tools available via AgentDef allowlist |
| `agents/sub_agents/skill_agent.rs` | Skill tools same |
| `agents/sub_agents/run.rs` | Logic moves to run_subagent() |
| `agents/sub_agents/traits.rs` | SubAgent trait no longer needed |
| `agents/sub_agents/registry.rs` | Unified into AgentRegistry |

**Evaluate:** `agents/sub_agents/persistence.rs` — migrate if needed.

**Enhance:**

| File | Change |
|------|--------|
| `agent_loop/subagent_tool.rs` | Core enhancement (this design) |
| `agents/registry.rs` | Add AgentDef fields + built-in role loading |
| `agents/types.rs` | Add ContextMode enum |

**Unchanged:** `agent_loop/loop_core.rs`, `agent_loop/chain_context.rs`, `agents/swarm/*`

### 9. Cleanup Order

1. Enhance SubagentTool + AgentRegistry (new functionality works)
2. Migrate gateway callers away from old paths
3. Delete old files after confirming no references
4. Clean up `agents/sub_agents/mod.rs` module declarations

## Not In Scope

- Agent-specific MCP servers (future iteration)
- Frontmatter hooks/skills per agent (future)
- Transcript/metadata recording (future)
- Fork with full conversation history inheritance (future)
- Foreground/background switching mid-execution (future)

## Architectural Compliance

| Redline | Status |
|---------|--------|
| R3 Core Minimalism | ✅ Removes 11 files, net code reduction |
| R8 LLM Sovereignty | ✅ Context summary by LLM, no middleware intelligence |
| R9 Everything is a Tool | ✅ Agent registration exposable as tool |
| R10 Intelligence in Prompt | ✅ Per-role system prompts, zero extra LLM calls |
| P3 Extensibility | ✅ User-defined AgentDef via JSON |
| P6 Simplicity | ✅ One dispatch path replaces three |
