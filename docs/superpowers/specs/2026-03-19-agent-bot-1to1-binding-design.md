# Agent-Bot 1:1 Binding Simplification

**Date**: 2026-03-19
**Status**: Draft
**Scope**: Core agent routing, reply emitter, Panel UI, builtin tools

## Problem

Current design allows dynamic agent switching within a bot/channel via:
- `agent_switch` tool (LLM-driven)
- `/switch` slash command (`command_handler.rs`)
- Natural language intent detection (`switch_intent.rs`)
- Three-tier agent resolution (dynamic override > router rules > default)

This flexibility causes confusion in practice. Adding agent name prefixes to replies did not resolve the UX problem. The system needs simplification to a strict 1:1 channel-agent binding model.

## Design

### Core Principle

**One channel binds exactly one agent.** Switching happens only through Panel Channel settings, never through conversation.

### Data Layer

**Reuse `channel_active_agent` table.** Semantic shift from "per-user dynamic override" to "channel-agent binding config."

- `peer_id` 保持原样传递实际 sender_id（Telegram 用户 ID 等），**方法签名不变**
- `WorkspaceManager` 现有方法签名保留 3 参数 (`channel, peer_id, agent_id`)：
  - `get_active_agent(channel, peer_id)` — 不变
  - `set_active_agent(channel, peer_id, agent_id)` — 增加 1:1 约束
  - `clear_active_agent(channel, peer_id)` — 不变
  - **New**: `get_channel_for_agent(agent_id) -> Option<String>` (reverse lookup for Panel)
  - **New**: `get_all_agent_bindings() -> HashMap<String, String>` (bulk lookup for Panel)

**1:1 constraint enforcement** in `set_active_agent`:
- Check-and-set within a single SQLite transaction to avoid TOCTOU race
- Before binding, query if any other channel already binds the target agent
- If occupied, return error with the occupying channel name (Panel displays this to user)

**No data migration needed**: peer_id 保持原样，现有数据无需清理。

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

**No auto-binding**: New installations require manual binding through Panel. The default `main` agent exists but is not auto-bound to any channel.

### Reply Layer Cleanup

- Delete `apply_agent_prefix()` from `reply_emitter.rs` and all call sites
- Remove `agent_display_name` field from `OutboundMessage` and related `native_identity` / `format_content` prefix logic
- `executor.rs` stops passing `display_name` to `ReplyEmitter` for prefix purposes

### Agent Tool Behavioral Changes

**`agent_create`** (`builtin_tools/agent_manage/create.rs`):
- Remove auto-switch logic: creating an agent no longer calls `set_active_agent` to bind it to the current channel
- Agent is created in unbound state; user binds it via Panel

**`agent_delete`** (`builtin_tools/agent_manage/delete.rs`):
- On deletion, unbind the agent from its channel (call `clear_active_agent` for the bound channel)
- Use `get_channel_for_agent(agent_id)` to find which channel to unbind

**`agent_list`** (`builtin_tools/agent_manage/list.rs`):
- Update display: show binding status (bound to which channel) instead of per-peer active status

### Intent Detection Cleanup

- Delete `inbound_router/switch_intent.rs` entirely
- Remove `DetectedIntent::SwitchAgent` variant from `IntentDetector` enum
- If `IntentDetector` has no remaining variants/purposes, delete the entire module
- Clean up `InboundMessageRouter` field `intent_detector: Option<IntentDetector>` and `with_intent_detector()` builder method

### Event System Cleanup

- Remove `AgentLifecycleEvent::Switched` variant (no longer emitted by any code path)
- If the variant is the only lifecycle event, evaluate whether the event type itself should be removed

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

### Existing RPC Migration

| Old Method | Action |
|------------|--------|
| `workspace.switchActive` | Delete — replaced by `channels.set_agent` |
| `workspace.getActive` | Delete — replaced by `get_active_agent` in new resolution flow |

## Deletions

| File/Module | Action |
|-------------|--------|
| `builtin_tools/agent_manage/switch.rs` | Delete file, unregister `agent_switch` tool |
| `inbound_router/switch_intent.rs` | Delete entire file |
| `inbound_router/command_handler.rs` — `/switch` command | Remove handler and command registration |
| `agent_resolver.rs` — `AgentRouter` dependency and multi-layer fallback | Remove, replace with single-tier lookup |
| `reply_emitter.rs` — `apply_agent_prefix()` | Delete function and all call sites |
| `IntentDetector` — `SwitchAgent` variant | Remove variant; delete module if no other variants remain |
| `AgentLifecycleEvent::Switched` | Remove variant |
| `workspace.switchActive` / `workspace.getActive` RPC | Delete handlers |

## Modifications

| Location | Change |
|----------|--------|
| `WorkspaceManager` (`manager_ops.rs`) | Keep method signatures unchanged, add 1:1 constraint to `set_active_agent`, add `get_channel_for_agent()` and `get_all_agent_bindings()` |
| `agent_resolver.rs` | Single-tier: bound → agent, unbound → fixed message |
| `agent_create` tool | Remove auto-switch-on-create behavior |
| `agent_delete` tool | Add unbind-on-delete behavior |
| `agent_list` tool | Show binding status instead of per-peer active status |
| Panel Channel settings | Add agent dropdown with empty option, disable occupied agents |
| Panel Agent list | Read-only display of bound channel |

## Behavioral Changes

1. Bot replies no longer prefixed with agent name
2. No agent switching via conversation (tool, slash command, or natural language)
3. Unbound channels return fixed prompt instead of falling back to default agent
4. Agent binding is managed exclusively through Panel Channel settings
5. 1:1 constraint: an agent can only be bound to one channel at a time; rebinding requires unbinding first
6. Creating an agent no longer auto-binds it to the current channel
7. Deleting an agent automatically unbinds it from its channel
