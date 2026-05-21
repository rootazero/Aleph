# Agent Link Access Control Design

**Date**: 2026-03-14
**Status**: Approved

## Problem

Aleph allows any bot (Link) to freely access any agent. Unlike openclaw's forced bot-agent binding, Aleph offers higher flexibility but lacks access control. Administrators need the ability to restrict which Links (bot instances) can access specific agents.

## Design

### Approach: Agent-side whitelist (Option<Vec<String>>)

- `None` or empty = all Links allowed (default open)
- `Some(list)` = only listed LinkIds can access this agent
- Consistent with existing `skills` field semantics

### Data Model

Add field to `AgentDefinition` in `src/config/types/agents_def.rs`:

```rust
/// Link access whitelist.
/// None or empty = all links allowed (default).
/// Some(list) = only listed link IDs can access this agent.
pub allowed_links: Option<Vec<String>>,
```

TOML example:

```toml
[[agents.list]]
id = "private-agent"
allowed_links = ["telegram-bot-1"]
```

### Access Check Function

Shared function for both enforcement points:

```rust
pub fn check_link_access(
    agent: &AgentDefinition,
    link_id: &LinkId,
) -> Result<(), AccessDeniedError> {
    match &agent.allowed_links {
        None => Ok(()),
        Some(list) if list.is_empty() => Ok(()),
        Some(list) => {
            if list.iter().any(|l| l == link_id.as_str()) {
                Ok(())
            } else {
                Err(AccessDeniedError::LinkNotAllowed {
                    link_id: link_id.clone(),
                    agent_id: agent.id.clone(),
                })
            }
        }
    }
}
```

### Enforcement Points

**1. InboundRouter (message routing)**

In `src/gateway/inbound_router.rs`, after route resolution determines the target agent, before dispatching the message:

- Extract `ChannelId` from inbound message (= `LinkId`)
- Load target agent's `allowed_links`
- If denied, return error message to bridge: `Access denied: link "{link_id}" is not allowed to access agent "{agent_name}".`

**2. Agent switching**

When a user switches agents during conversation (via natural language or slash command):

- Call `check_link_access()` with the current session's link ID and the target agent
- If denied, LLM responds with access denied message

### API Layer

**No new endpoints needed:**

- `channels.list` (existing) — returns all active channel/link instances, used by UI to render toggle list
- `agents.update` (existing) — accepts `allowed_links` as part of agent definition patch

### Panel UI

In `apps/panel/src/views/agents/channels.rs`, add a "Link Access Control" section after existing Channel Bindings:

```
┌─────────────────────────────────┐
│ ★ Default Agent / Set Default   │  ← existing
├─────────────────────────────────┤
│ Channel Bindings                │  ← existing
├─────────────────────────────────┤
│ Link Access Control             │  ← new
│                                 │
│  ○ telegram-bot-1  [████ ON ]   │
│  ○ telegram-bot-2  [████ ON ]   │
│  ○ discord-bot     [░░░░ OFF]   │
└─────────────────────────────────┘
```

**Interaction:**
1. Load all links via `channels.list`
2. Read agent's `allowed_links`
3. `None`/empty → all toggles ON
4. User toggles OFF → collect remaining ON link IDs, save via `agents.update`
5. All toggles ON → save as `None` (restore default)

### Edge Cases

| Case | Behavior |
|------|----------|
| `allowed_links` is `None` or empty | All links allowed (default) |
| All links toggled OFF | Agent only accessible via Panel web UI |
| Deleted link ID in whitelist | Ignored, UI shows greyed-out entry |
| Bridge self-generated agent | Same rules, default `None` = all allowed |
| Config hot-update | Next message routing picks up changes immediately |

### Error Response

When a link is denied:

```
Access denied: link "{link_id}" is not allowed to access agent "{agent_name}".
```

## Key Identifiers

| Concept | Type | Granularity | Example |
|---------|------|-------------|---------|
| Bridge | `BridgeId` | Plugin type | `"telegram"` |
| Link | `LinkId` | Bot instance | `"telegram-bot-1"` |
| Channel | `ChannelId` | Runtime (= LinkId) | `"telegram-bot-1"` |

Access control is at **Link (bot instance)** granularity, not Bridge (platform) granularity.

## Files to Modify

1. `src/config/types/agents_def.rs` — Add `allowed_links` field
2. `src/gateway/inbound_router.rs` — Insert access check after route resolution
3. `src/gateway/` (new or existing module) — `check_link_access()` function
4. `apps/panel/src/views/agents/channels.rs` — Add Link Access Control UI section
5. Agent switching logic (if exists) — Insert access check
