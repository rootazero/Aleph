# Multi-Agent System

Aleph provides three user-triggerable collaboration modes plus a shared sensing infrastructure layer. All modes operate through tools (R9: Everything is a Tool). The LLM chooses between Spawn and Delegate automatically; Team mode requires explicit user invocation.

## Architecture Overview

```
┌──────────────────────────────────────────────────────┐
│          User-Triggerable Collaboration Modes         │
│                                                        │
│  ┌───────────┐   ┌──────────────┐   ┌──────────────┐ │
│  │   Spawn   │   │   Delegate   │   │     Team     │ │
│  │ subagent  │   │ session_send │   │  /team, /task │ │
│  │           │   │              │   │              │ │
│  │ Ephemeral │   │ Peer-to-peer │   │ Structured   │ │
│  │ worker    │   │ messaging    │   │ DAG + Leader │ │
│  └───────────┘   └──────────────┘   └──────────────┘ │
├──────────────────────────────────────────────────────┤
│        Swarm Sensing (automatic infrastructure)       │
│     Event Bus · Context Injector · Collective Memory  │
└──────────────────────────────────────────────────────┘
```

## Mode Comparison

| | Spawn | Delegate | Team |
|---|---|---|---|
| **Trigger** | LLM automatic | LLM automatic | User `/team` command |
| **Tools** | `subagent_spawn/steer/kill` | `session_send` | `team_*` + `task_*` |
| **Lifecycle** | Ephemeral (destroyed on completion) | Persistent (agents exist independently) | Persistent (disband to end) |
| **Relationship** | Vertical (parent → child) | Horizontal (peer ↔ peer) | Hierarchical (Leader → Members) |
| **Communication** | Return value | Messages (fire-and-forget or wait) | Task board + messages |

## Mode 1: Spawn (Sub-Agent Dispatch)

**Tools**: `subagent_spawn`, `subagent_steer`, `subagent_kill`

The main agent spawns a temporary AgentLoop to handle a focused sub-task. The sub-agent has its own tool registry (excluding subagent tools to prevent recursion), token budget, and timeout. It returns a result and is destroyed.

**When LLM uses it**: Tasks benefiting from isolated context — parallel searches, code analysis, format conversion, translation.

**Swarm integration**: Sub-agent events are NOT published to the Event Bus (ephemeral, not a named agent).

## Mode 2: Delegate (Peer Communication)

**Tool**: `session_send`

A named agent sends a message to another named agent. Two semantics:
- **Fire-and-forget** (`timeout_seconds=0`): Notification — "FYI, I found this"
- **Wait-for-reply** (`timeout_seconds>0`): Delegation — "Please analyze this and reply"

**When LLM uses it**: When another agent has relevant expertise, when work should be transferred to a specialist, when coordination is needed between peers.

**Swarm integration**: Both agents are named — their activities are visible to all agents via Event Bus. Communication governed by `AgentToAgentPolicy`.

## Mode 3: Team (Structured Coordination)

**Tools**: `team_create`, `team_launch`, `team_list`, `team_disband`, `task_create`, `task_update`, `task_list`, `task_wait`

**Entry**: `/team` or `/task` slash commands

A user explicitly creates a team with a Leader, members with defined roles, and a task DAG with dependencies. The Leader coordinates work via prompt-driven intelligence (R8/R10).

**Key concepts**:
- **Team templates** — Markdown files with YAML frontmatter defining roles, tasks, and dependencies. Stored in `~/.aleph/templates/`.
- **Task DAG** — Tasks have `blocked_by` dependencies. Completing a task automatically unblocks dependents.
- **Leader/Member prompts** — Context Injector dynamically injects role-specific prompts with the current task board.
- **Immutable dependency edges** — The DAG structure is preserved for auditing and retry support.

**When user triggers it**: Multi-step projects requiring structured work breakdown — code reviews, research teams, parallel analysis.

## Infrastructure: Swarm Sensing

**Components**: Event Bus, Rules Engine, Semantic Aggregator, Context Injector, Collective Memory

Not a user-triggerable mode. Automatically active for all named agents. Provides:
- Cross-agent activity awareness (who is doing what)
- Hotspot detection (multiple agents working in same area)
- Contextual injection (team task boards, swarm activity summaries)
- Collective memory for shared knowledge

All three modes benefit from swarm sensing — it is the shared perception layer that makes multi-agent collaboration intelligent.

## Mode Selection Guide

| Need | Mode |
|------|------|
| Simple sub-task (search, compute, format) | Spawn |
| Ask another agent a question | Delegate |
| Notify another agent of a finding | Delegate (fire-and-forget) |
| Hand off a task to a specialist | Delegate (wait-for-reply) |
| Multi-step project with dependencies | Team |
| Code review with multiple reviewers | Team |
| Multi-analyst decision making | Team |

## Tool Reference

### Spawn
| Tool | Description |
|------|-------------|
| `subagent_spawn` | Spawn an ephemeral sub-agent for a focused task |
| `subagent_steer` | Send guidance to a running sub-agent |
| `subagent_kill` | Terminate a running sub-agent |

### Delegate
| Tool | Description |
|------|-------------|
| `session_send` | Send message to another agent's session (fire-and-forget or wait) |

### Team
| Tool | Description |
|------|-------------|
| `team_create` | Create a team with a leader |
| `team_launch` | Launch a team from a template with agents and tasks |
| `team_list` | List all teams and their status |
| `team_disband` | Disband a team, cancelling remaining tasks |
| `task_create` | Create a task with optional dependencies |
| `task_update` | Update task status, owner, result, or metadata |
| `task_list` | List tasks with filtering |
| `task_wait` | Wait for tasks to complete |

## Design Principles

These modes are **conceptual categories, not runtime states**. There is no mode-switching code. The LLM selects tools based on their descriptions (R8: LLM Sovereignty). The system provides the primitives; the LLM provides the intelligence.

See also:
- [AGENT_SYSTEM.md](AGENT_SYSTEM.md) — Single-agent internals (loop, guards, state machine)
- [TOOL_SYSTEM.md](TOOL_SYSTEM.md) — Tool registration and execution
