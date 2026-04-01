# Panel Agent Routing Fix

> Fix: Panel webchat routes all messages to default agent "main" instead of the selected agent.

## Status

**Approved** — 2026-03-19

## Problem

Panel webchat has an agent list sidebar. Users click an agent to open its chat. But `chat.send` RPC does not include the agent_id, so the gateway router falls back to the default agent "main" for every first message. Since the returned session_key encodes `agent:main:...`, all subsequent messages also route to main. The bug propagates from the first message to the entire conversation.

Telegram works correctly because it uses channel bindings (e.g., `telegram:*` → bound agent).

## Root Cause

```
Panel sends:  { message, session_key: null, channel: "gui:chat" }
Router sees:  no session_key, no binding for "gui:chat" → default "main"
```

Panel knows the agent_id (from the sidebar selection) but doesn't pass it.

## Fix

Add optional `agent_id` parameter to `chat.send` RPC. Panel passes it. Router uses it.

### Router Priority (unchanged for existing paths, new priority 2 added)

1. **session_key** — existing conversation, highest priority
2. **explicit agent_id** — NEW, panel/API specifies target agent
3. **channel binding** — external channels (Telegram, Slack, etc.)
4. **default agent** — fallback to "main"

## Changes

### 1. `src/gateway/handlers/chat.rs` — SendParams

```rust
pub struct SendParams {
    pub message: String,
    pub session_key: Option<String>,
    pub channel: Option<String>,
    pub stream: bool,
    pub thinking: Option<String>,
    pub attachments: Vec<Attachment>,
    pub agent_id: Option<String>,  // NEW
}
```

Forward `agent_id` to `AgentRunParams`.

### 2. `src/gateway/handlers/agent.rs` — AgentRunParams + start_run

```rust
pub struct AgentRunParams {
    // ... existing fields ...
    pub agent_id: Option<String>,  // NEW
}
```

Pass `agent_id` to `router.route()`.

### 3. `src/gateway/router.rs` — route()

```rust
pub async fn route(
    &self,
    session_key: Option<&str>,
    channel: Option<&str>,
    peer_id: Option<&str>,
    agent_id: Option<&str>,  // NEW
) -> SessionKey {
    // 1. Explicit session_key takes precedence
    if let Some(key_str) = session_key {
        if let Some(key) = SessionKey::from_key_string(key_str) {
            return key;
        }
    }

    // 2. Explicit agent_id (from panel or API)
    if let Some(aid) = agent_id {
        return match peer_id {
            Some(pid) => SessionKey::peer(aid, pid),
            None => SessionKey::main(aid),
        };
    }

    // 3. Channel binding (Telegram, Slack, etc.)
    // 4. Default agent fallback
    let resolved = self.resolve_agent(channel, peer_id).await;
    // ... existing logic ...
}
```

### 4. `apps/panel/src/api/chat.rs` — send()

```rust
let params = serde_json::json!({
    "message": message,
    "session_key": session_key,
    "channel": "gui:chat",
    "stream": true,
    "attachments": attachments_json,
    "agent_id": agent_id,  // NEW — from current chat context
});
```

Panel `send()` signature gains `agent_id: Option<&str>` parameter. The caller (chat view) passes the current agent_id from the sidebar/route context.

### 5. Panel chat view — pass agent_id

The chat view already knows which agent is selected (from URL route or sidebar state). Pass it through to `ChatApi::send()`.

## Scope

- Panel webchat fix (primary)
- `chat.send` RPC gains `agent_id` parameter (benefits all callers)
- Router gains explicit agent_id priority level
- No changes to Telegram/external channels (they already work via bindings)
- No changes to SessionKey format
- No changes to ExecutionEngine

## Out of Scope

- Agent switching UI in panel (already exists via sidebar)
- Channel binding management UI
- New routing module integration (`src/routing/`)
