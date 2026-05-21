# Subagent System Refactor Design

**Date:** 2026-03-18
**Status:** Approved
**Goal:** Replace the three-tool subagent system (spawn/steer/kill) and fake sub-agents (McpSubAgent/SkillSubAgent) with a single `subagent` tool backed by a real LLM-powered AgentLoop.

---

## Problem

The current subagent system has three tools (`subagent_spawn`, `subagent_steer`, `subagent_kill`) and two sub-agent implementations (`McpSubAgent`, `SkillSubAgent`). None of them are real agents:

- `McpSubAgent.execute()` only lists MCP tools — does not call LLM or execute tools
- `SkillSubAgent.execute()` only matches skill names — no LLM reasoning
- `subagent_steer` and `subagent_kill` are dead code — `SubAgentRegistry` is never initialized in `agent_init.rs`
- The naming `subagent_spawn` leaks implementation details into the LLM interface

A sub-agent should be a temporary, autonomous LLM worker that runs a full think→act loop, uses real tools, and returns results to the parent.

## Design

### Tool Interface

Single tool exposed to LLM:

```
name: "subagent"
description: "Delegate a task to an autonomous sub-agent that works independently
              and returns the result. The sub-agent has access to all your tools
              and can make multiple tool calls to complete the task. Use this for
              tasks that can be done in parallel or require focused independent work."
parameters:
  task: string (required) — clear description of what to accomplish
  timeout_secs: integer (optional, default 120) — max seconds to wait
```

### Execution Flow

```
subagent(task, timeout_secs)
  1. Clone parent's LoopToolRegistry, remove "subagent" tool (prevent recursion)
  2. Build PromptBuilder with sub-agent system prompt
  3. Create temporary AgentLoop (reuse parent's LoopProvider)
  4. Run loop.run(task) — independent think→act cycle
  5. Return LoopRunResult.final_text to parent
  6. Temporary AgentLoop drops automatically
```

### Key Decisions

- **Reuse AgentLoop** — the existing think→act loop is the engine; no new loop implementation
- **Reuse parent's LoopProvider** — same LLM backend, zero configuration
- **Tool set = parent set - "subagent"** — prevents recursive sub-agent spawning
- **Synchronous wait** — parent agent blocks until sub-agent completes (or timeout)
- **No lifecycle management** — sub-agent is a function call, not a managed entity. No registry, no kill, no steer.

### Sub-agent System Prompt

The sub-agent gets a focused prompt:

```
You are a focused sub-agent executing a specific task. Complete the task using
available tools, then return a clear summary of results. Be concise and direct.
Do not ask clarifying questions — work with what you have.
```

This replaces the parent's full soul/personality prompt to keep the sub-agent focused.

## Files Changed

### Delete

| File | Reason |
|------|--------|
| `builtin_tools/subagent_manage/spawn.rs` | Replaced by new SubagentTool |
| `builtin_tools/subagent_manage/steer.rs` | Dead code (registry never initialized) |
| `builtin_tools/subagent_manage/kill.rs` | Dead code |
| `builtin_tools/subagent_manage/mod.rs` | Entire module removed |
| `agents/sub_agents/mcp_agent.rs` | Not a real agent, only pattern matching |
| `agents/sub_agents/skill_agent.rs` | Not a real agent |
| `agents/sub_agents/dispatcher.rs` | No longer needed |
| `agents/sub_agents/coordinator.rs` | Replaced by direct await |
| `agents/sub_agents/result_collector.rs` | No longer needed |
| `agents/sub_agents/registry.rs` | Never properly used |
| `agents/sub_agents/run.rs` | Part of old lifecycle management |
| `agents/sub_agents/traits.rs` | Old SubAgent trait no longer needed |
| `agents/sub_agents/mod.rs` | Entire module removed |

### Create

| File | Description |
|------|-------------|
| `builtin_tools/subagent.rs` | New SubagentTool — creates AgentLoop, runs task, returns result |

### Modify

| File | Change |
|------|--------|
| `builtin_tools/mod.rs` | Remove `subagent_manage`, add `subagent` |
| `executor/builtin_registry/config.rs` | Remove `sub_agent_dispatcher`/`sub_agent_registry` fields, add subagent provider/tools config |
| `executor/builtin_registry/builder.rs` | Register new SubagentTool |
| `executor/builtin_registry/registry.rs` | Route `"subagent"` to new tool, remove old spawn/steer/kill routes |
| `bin/aleph/commands/start/builder/agent_init.rs` | Remove old SubAgentDispatcher initialization |
| `agents/mod.rs` | Remove `sub_agents` module declaration (if it becomes empty) |

### Unaffected

- MCP tools — registered independently via ToolRegistry
- Skills — registered independently via extension/plugin system
- Main AgentLoop — unchanged, gains one new tool
- Agent switch/create — peer agent system is separate
- Safety guard — sub-agent inherits parent's safety rules

## Constraints

- Sub-agent cannot call `subagent` (recursion prevention)
- Sub-agent timeout defaults to 120s, configurable per call
- Sub-agent uses same LLM provider as parent (no separate API key)
- Sub-agent does NOT inherit parent's conversation history (clean context)

## Future Work (Not In Scope)

- Parallel execution optimization (run multiple subagent calls concurrently in loop_core)
- Sub-agent tool subset configuration (allow parent to restrict which tools sub-agent gets)
- Streaming sub-agent progress events to parent's event emitter
