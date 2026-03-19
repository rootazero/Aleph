# Agent-Bot 1:1 Binding Simplification

**Date**: 2026-03-19
**Status**: Draft
**Scope**: Core agent routing, reply emitter, Panel UI, builtin tools

## Problem

Current design allows dynamic agent switching within a bot/channel via:
- `agent_switch` tool (LLM-driven)
- Natural language intent detection (`switch_intent.rs`)
- Three-tier agent resolution (dynamic override > router rules > default)

This flexibility causes confusion in practice. Adding agent name prefixes to replies did not resolve the UX problem. The system needs simplification to a strict 1:1 channel-agent binding model.

## Design

### Core Principle

**One channel binds exactly one agent.** Switching happens only through Panel Channel settings, never through conversation.

### Data Layer

**Reuse `channel_active_agent` table.** Semantic shift from "per-user dynamic override" to "channel-agent binding config."

- `peer_id` fixed to constant `"default"` — preserves schema for future multi-user expansion
- `WorkspaceManager` method signatures simplified:
  - `get_active_agent(channel) -> Option<String>` (drop `peer_id` param, hardcode `"default"`)
  - `set_active_agent(channel, agent_id) -> Result<()>` (drop `peer_id`, add 1:1 constraint)
  - `clear_active_agent(channel) -> Result<()>` (drop `peer_id`)
  - **New**: `get_channel_for_agent(agent_id) -> Option<String>` (reverse lookup for Panel)

**1:1 constraint enforcement** in `set_active_agent`:
- Before binding, query if any other channel already binds the target agent
- If occupied, return error with the occupying channel name (Panel displays this to user)

### Agent Resolution (Simplified)

Replace three-tier resolution in `agent_resolver.rs` with single-tier:

```
Inbound Message → get_active_agent(channel)
  ├─ Some(agent_id) → execute with that agent
  └─ None → return fixed message: "此频道未绑定 Agent，请在 Panel 中配置"
```

**Deleted layers:**
- Layer 1: Dynamic per-peer override lookup
- Layer 2: `AgentRouter` multi-rule routing

### Reply Layer Cleanup

- Delete `apply_agent_prefix()` from `reply_emitter.rs` and all call sites
- Keep `agent_display_name` field on `OutboundMessage` (useful for logs/Panel), but no longer used for text prefixing
- `executor.rs` stops passing `display_name` to `ReplyEmitter` for prefix purposes

### Panel UI

**Channel Settings Page:**
- New agent dropdown selector
- Options: all agents from `agents.list()` + empty option ("未绑定")
- Agents already bound to other channels shown as disabled (greyed out + occupying channel name)
- Selection immediately calls backend to bind/unbind

**Agent List Page:**
- Each agent card shows bound channel name (read-only), or "未绑定"
- Data from `agents.bindings()` RPC

### New RPC Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `channels.set_agent` | `(channel_id: String, agent_id: Option<String>)` | Bind or unbind agent. Enforces 1:1 constraint. |
| `agents.bindings` | `() -> HashMap<String, String>` | Returns `agent_id → channel_name` mapping |

## Deletions

| File/Module | Action |
|-------------|--------|
| `builtin_tools/agent_manage/switch.rs` | Delete file, unregister `agent_switch` tool |
| `inbound_router/switch_intent.rs` | Delete entire file |
| `agent_resolver.rs` — `AgentRouter` dependency and multi-layer fallback | Remove, replace with single-tier lookup |
| `reply_emitter.rs` — `apply_agent_prefix()` | Delete function and all call sites |

## Modifications

| Location | Change |
|----------|--------|
| `WorkspaceManager` (`manager_ops.rs`) | Simplify `get/set/clear_active_agent` signatures (drop `peer_id`), add 1:1 constraint, add `get_channel_for_agent()` |
| `agent_resolver.rs` | Single-tier: bound → agent, unbound → fixed message |
| Panel Channel settings | Add agent dropdown with empty option, disable occupied agents |
| Panel Agent list | Read-only display of bound channel |

## Behavioral Changes

1. Bot replies no longer prefixed with agent name
2. No agent switching via conversation (tool or natural language)
3. Unbound channels return fixed prompt instead of falling back to default agent
4. Agent binding is managed exclusively through Panel Channel settings
5. 1:1 constraint: an agent can only be bound to one channel at a time; rebinding requires unbinding first
