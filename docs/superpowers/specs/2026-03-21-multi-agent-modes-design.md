# Multi-Agent Modes Taxonomy Design

**Date**: 2026-03-21
**Status**: Approved
**Scope**: Formalize Aleph's multi-agent collaboration modes, reorganize tool groups, add /team command

## Background

Aleph's multi-agent capabilities have grown organically across several subsystems: ephemeral sub-agents, named agent communication, team coordination (just implemented), and swarm sensing. These capabilities lack a unified conceptual model — users and developers see a flat list of tools with no clear organizing principle.

This design formalizes three user-triggerable collaboration modes + one infrastructure layer, reorganizes tool groups to reflect this taxonomy, and adds a `/team` slash command as the entry point for team mode.

## Multi-Agent Mode Taxonomy

### Architecture

```
┌─────────────────────────────────────────────────────┐
│         User-Triggerable Collaboration Modes         │
│                                                       │
│  ┌──────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │  Spawn   │   │   Delegate   │   │     Team     │ │
│  │(subagent)│   │(session_send)│   │   (/team)    │ │
│  │          │   │              │   │              │ │
│  │ Temp     │   │ Peer-to-peer │   │ Structured   │ │
│  │ worker   │   │ messaging    │   │ DAG + Leader │ │
│  └──────────┘   └──────────────┘   └──────────────┘ │
├─────────────────────────────────────────────────────┤
│       Swarm Sensing (automatic infrastructure)       │
│    Event Bus · Context Injector · Collective Memory  │
└─────────────────────────────────────────────────────┘
```

### Mode Definitions

| | Spawn | Delegate | Team |
|---|---|---|---|
| **Trigger** | LLM automatic | LLM automatic | User `/team` command |
| **Tools** | `subagent` | `session_send` | `team_*` + `task_*` |
| **Lifecycle** | Ephemeral (destroyed on completion) | Persistent (agents exist independently) | Persistent (disband to end) |
| **Relationship** | Vertical (parent → child) | Horizontal (peer ↔ peer) | Hierarchical (Leader → Members) |
| **Communication** | Return value | Messages (fire-and-forget or wait-for-reply) | Task board + messages |
| **Use cases** | Focused sub-tasks, parallel search, translation | Ask expert agent, hand off work, cross-domain collaboration | Multi-person projects, code review, multi-analyst decisions |

### Mode 1: Spawn (Sub-Agent Dispatch)

**Tool**: `subagent`

The main agent spawns a temporary, ephemeral AgentLoop to handle a focused sub-task. The sub-agent has its own tool registry (excluding `subagent` to prevent recursion), token budget, and timeout. It returns a result and is destroyed.

- **When LLM uses it**: Tasks that benefit from isolated context — parallel searches, code analysis, format conversion, translation
- **No user action needed**: LLM decides when to spawn based on task complexity
- **Swarm integration**: Sub-agent events are NOT published to the swarm Event Bus (ephemeral, not a named agent)

### Mode 2: Delegate (Peer Communication)

**Tool**: `session_send`

A named agent sends a message to another named agent. Two semantics:
- **Fire-and-forget** (`timeout_seconds=0`): "FYI, I found this" — notification
- **Wait-for-reply** (`timeout_seconds>0`): "Please analyze this and tell me what you find" — delegation

- **When LLM uses it**: When another agent has relevant expertise, when work should be transferred to a specialist, when coordination is needed between peers
- **No user action needed**: LLM decides based on agent descriptions and current task
- **Swarm integration**: Both agents are named — their activities are visible to all other agents via Event Bus
- **A2A Policy**: `AgentToAgentPolicy` controls which agents can communicate

### Mode 3: Team (Structured Coordination)

**Tools**: `team_create`, `team_launch`, `team_list`, `team_disband`, `task_create`, `task_update`, `task_list`, `task_wait`

**Entry point**: `/team` slash command

A user explicitly creates a team of agents with a Leader, members with defined roles, and a task DAG with dependencies. The Leader coordinates work via prompt-driven intelligence (R8/R10).

- **When user triggers it**: Multi-person projects requiring structured work breakdown — code reviews, research teams, parallel analysis
- **Explicit entry**: User must use `/team` to create/manage teams. LLM does not spontaneously create teams.
- **Swarm integration**: All team members are named agents — full Event Bus visibility. Context Injector dynamically injects Leader/Member prompts with task board state.

### Infrastructure: Swarm Sensing

**Components**: Event Bus, Rules Engine, Semantic Aggregator, Context Injector, Collective Memory

NOT a user-triggerable mode. Automatically active for all named agents. Provides:
- Cross-agent activity awareness (who is doing what)
- Hotspot detection (multiple agents in same area)
- Contextual injection (team task boards, swarm activity summaries)
- Collective memory for shared knowledge

All three modes above benefit from swarm sensing — it's the shared perception layer that makes multi-agent collaboration intelligent.

## Mode Selection Guide

| Need | Recommended Mode |
|------|-----------------|
| Simple sub-task (search, compute, format) | Spawn |
| Ask another agent a question | Delegate |
| Notify another agent of a finding | Delegate (fire-and-forget) |
| Hand off a task to a specialist | Delegate (wait-for-reply) |
| Multi-step project with dependencies | Team |
| Code review with multiple reviewers | Team (use code-review-team template) |
| Multi-analyst decision making | Team |

LLM chooses between Spawn and Delegate automatically. Team mode requires explicit user invocation via `/team`.

## Tool Group Reorganization

### Current state

Tool groups in `groups.rs` are organized by technical category, not by collaboration mode. Users see a flat list without conceptual structure.

### New groups

```rust
// Agent management (infrastructure, not a mode)
ToolGroup {
    id: "agent_mgmt",
    name: "Agent 管理",
    tools: &["agent_create", "agent_list", "agent_delete"],
},

// Spawn mode
ToolGroup {
    id: "spawn",
    name: "子 Agent 派发",
    tools: &["subagent_spawn", "subagent_steer", "subagent_kill"],
},

// Delegate mode
ToolGroup {
    id: "delegate",
    name: "Agent 间通信",
    tools: &["session_send", "session_list"],
},

// Team mode (team + task tools together)
ToolGroup {
    id: "team",
    name: "团队协调",
    tools: &[
        "team_create", "team_launch", "team_list", "team_disband",
        "task_create", "task_update", "task_list", "task_wait",
    ],
},

// Session management (user's own sessions, independent of modes)
ToolGroup {
    id: "session_mgmt",
    name: "会话管理",
    tools: &["session_new", "session_rename"],
},
```

### Changes

- Merge `task_coordination` into `team` group — task tools are part of team mode
- Move `session_send` and `session_list` from session management to `delegate` group
- Keep `session_new` / `session_rename` in session management — these are user session ops, not A2A

## `/team` Slash Command

### Command mapping

```
/team                              → team_list (show all teams)
/team launch <template>            → team_launch (start team from template)
/team launch <template> --goal "…" → team_launch with variables
/team disband <team_id>            → team_disband
/team status <team_id>             → task_list with team_id filter
```

### Implementation

Register `team` as a command namespace in the Gateway command handler (same pattern as existing `/session`, `/agent` namespaces). Sub-commands map to existing tools — no new tool logic.

The Gateway command hierarchy system (implemented in `gateway/handlers/commands.rs`) already supports namespace browsing. Adding `team` follows the established pattern.

Estimated: ~30 lines in commands.rs.

## Reference Document

### New file: `docs/reference/MULTI_AGENT_SYSTEM.md`

A reference document that serves as the authoritative definition of Aleph's multi-agent modes. Content mirrors this spec's taxonomy and mode definitions, formatted for developer and system prompt consumption.

Complements existing `AGENT_SYSTEM.md` (which covers single-agent internals: loop, guards, state machine). The new doc covers agent-to-agent collaboration.

Update `CLAUDE.md` documentation index table to include the new reference.

## File Changes

| File | Change | Lines |
|------|--------|-------|
| `core/src/executor/builtin_registry/groups.rs` | Reorganize groups to reflect three modes | ~20 |
| `core/src/gateway/handlers/commands.rs` | Add `team` command namespace + sub-command routing | ~30 |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | New reference document | ~150 |
| `CLAUDE.md` | Add doc index entry | ~1 |
| **Total** | | **~200** |

## YAGNI — Explicitly Not Doing

- **No new tools** — all tools already exist
- **No mode switching code** — LLM selects tools naturally via descriptions (R8)
- **No unified interface** — three modes have different mechanisms, abstracting over them adds complexity without benefit
- **No swarm mode as user-triggerable** — swarm is infrastructure, always active
- **No /delegate command** — session_send is already the tool, LLM uses it automatically

## Architectural Compliance

| Principle | How this design complies |
|-----------|------------------------|
| R8 LLM Sovereignty | Spawn/Delegate chosen by LLM automatically; only Team requires explicit user trigger |
| R9 Everything is a Tool | All three modes operate through existing tools |
| R10 Intelligence in Prompt | Team Leader/Member coordination is prompt-driven |
| P6 Simplicity | ~200 lines total, zero new abstractions, zero new tools |
