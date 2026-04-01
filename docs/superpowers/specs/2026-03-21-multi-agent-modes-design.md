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
| **Tools** | `subagent_spawn/steer/kill` | `session_send` | `team_*` + `task_*` |
| **Lifecycle** | Ephemeral (destroyed on completion) | Persistent (agents exist independently) | Persistent (disband to end) |
| **Relationship** | Vertical (parent → child) | Horizontal (peer ↔ peer) | Hierarchical (Leader → Members) |
| **Communication** | Return value | Messages (fire-and-forget or wait-for-reply) | Task board + messages |
| **Use cases** | Focused sub-tasks, parallel search, translation | Ask expert agent, hand off work, cross-domain collaboration | Multi-person projects, code review, multi-analyst decisions |

### Mode 1: Spawn (Sub-Agent Dispatch)

**Tools**: `subagent_spawn`, `subagent_steer`, `subagent_kill`

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
    tools: &["subagent_spawn", "subagent_steer", "subagent_kill", "escalate_task"],
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

// Automation (kept from existing agent_mgmt, not a collaboration mode)
ToolGroup {
    id: "automation",
    name: "自动化",
    tools: &["cron_manage", "clawhub"],
},
```

### Changes

- Rename `task_coordination` → `team` group, keeping same tools
- Split monolithic `agent_mgmt` into focused groups:
  - `agent_mgmt`: only agent CRUD
  - `spawn`: subagent tools + escalate_task
  - `delegate`: session_send + session_list (A2A communication)
  - `session_mgmt`: session_new + session_rename (user session ops)
  - `automation`: cron_manage + clawhub (not collaboration modes)
- All tools currently in `agent_mgmt` accounted for — `test_all_builtin_tools_have_a_group` passes

### Namespace registration

Add `"team"` and `"task"` to `TOOL_NAMESPACES` in `commands.rs`. This enables `/team launch`, `/team list`, `/task create`, `/task list` etc. as natural namespace-based command routing.

Note: These modes are **conceptual categories, not runtime states**. There is no mode-switching code. The LLM selects tools based on their descriptions.

## `/team` Slash Command

### Command mapping

The existing Gateway namespace system automatically maps `/team <sub>` to `team_<sub>` tool:

```
/team                  → browse team_* tools (namespace palette)
/team create           → team_create
/team launch           → team_launch
/team list             → team_list
/team disband          → team_disband
```

Similarly, `/task` namespace:
```
/task                  → browse task_* tools
/task create           → task_create
/task update           → task_update
/task list             → task_list
/task wait             → task_wait
```

### Implementation

Add `"team"` and `"task"` to `TOOL_NAMESPACES` array in `commands.rs`. The existing hierarchical command system handles the rest — no custom routing logic needed.

Estimated: 2 lines in commands.rs (one word each).

## Reference Document

### New file: `docs/reference/MULTI_AGENT_SYSTEM.md`

A reference document that serves as the authoritative definition of Aleph's multi-agent modes. Content mirrors this spec's taxonomy and mode definitions, formatted for developer and system prompt consumption.

Complements existing `AGENT_SYSTEM.md` (which covers single-agent internals: loop, guards, state machine). The new doc covers agent-to-agent collaboration.

Update `CLAUDE.md` documentation index table to include the new reference.

## File Changes

| File | Change | Lines |
|------|--------|-------|
| `src/executor/builtin_registry/groups.rs` | Split `agent_mgmt` into mode-based groups | ~30 |
| `src/gateway/handlers/commands.rs` | Add `"team"` and `"task"` to `TOOL_NAMESPACES` | ~2 |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | New reference document | ~150 |
| `CLAUDE.md` | Add doc index entry | ~1 |
| **Total** | | **~183** |

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
