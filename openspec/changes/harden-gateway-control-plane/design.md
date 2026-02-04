# Design: Gateway Control Plane Hardening

## Overview

This change wires the existing auth, pairing, routing, and event subsystems into the gateway startup path so the control plane behaves consistently for both WS clients and channel inbound messages.

## Key Decisions

1. **Connection‑level auth gate**: Enforce a `connect` handshake when `require_auth` is enabled. Unauthorized requests return `AUTH_REQUIRED` and the connection is closed to align with the documented handshake expectation.
2. **Per‑connection event filters**: Use `SubscriptionManager` to maintain connection filters. Event routing matches either JSON‑RPC `method` (for `stream.*`) or `topic` fields (for `TopicEvent` payloads), with default "receive all" behavior.
3. **Unified bindings**: Reuse `AgentRouter` bindings for inbound channel routing. Inbound messages derive a channel string as `{channel_id}:{conversation_id}` to allow `channel:*` bindings while preserving per‑conversation specificity.
4. **ExecutionAdapter reuse**: Inbound routing invokes the same `ExecutionEngine` via `ExecutionAdapter`, and responses are routed back via `ReplyEmitter`.

## Compatibility Notes

- If `require_auth` is disabled, behavior remains backward compatible (no handshake required).
- Event subscription is opt‑in; existing clients that do not subscribe continue to receive all events.

---

## Connection Handshake Protocol

### When `require_auth: true`

The first message from a client **MUST** be a `connect` request. Any other method results in `AUTH_REQUIRED` error and connection closure.

```json
// Client → Gateway
{
  "jsonrpc": "2.0",
  "method": "connect",
  "id": "1",
  "params": {
    "minProtocol": 1,
    "maxProtocol": 1,
    "client": { "id": "cli", "version": "0.1.0", "platform": "macos" },
    "role": "operator",
    "auth": { "token": "bearer_token_here" }
  }
}

// Gateway → Client (success)
{
  "jsonrpc": "2.0",
  "id": "1",
  "result": {
    "type": "hello-ok",
    "protocol": 1,
    "auth": { "deviceToken": "...", "role": "operator" }
  }
}
```

### When `require_auth: false`

No handshake required. Clients can immediately call any RPC method.

---

## RPC Methods

### Agent Control

| Method | Description | Auth Required |
|--------|-------------|---------------|
| `agent.run` | Execute agent with input, returns `run_id` | Yes (when enabled) |
| `agent.status` | Query run status by `run_id` | Yes |
| `agent.cancel` | Cancel an active run by `run_id` | Yes |

**Example: agent.run**
```json
{
  "method": "agent.run",
  "params": {
    "input": "Hello, how are you?",
    "session_key": "agent:main:main",  // optional
    "channel": "gui:window1",           // optional, for binding resolution
    "stream": true,
    "thinking": "medium"                // optional: off/minimal/low/medium/high/xhigh
  }
}
```

**Example: agent.status**
```json
{
  "method": "agent.status",
  "params": { "run_id": "uuid-xxx" }
}
// Response
{
  "result": {
    "run_id": "uuid-xxx",
    "session_key": "agent:main:main",
    "status": "running",  // running/completed/failed/cancelled
    "elapsed_ms": 1234
  }
}
```

### Event Subscription

| Method | Description |
|--------|-------------|
| `events.subscribe` | Subscribe to event patterns (glob matching) |
| `events.unsubscribe` | Remove subscription patterns |
| `events.list` | List current subscriptions |

**Example: Subscribe to stream events only**
```json
{
  "method": "events.subscribe",
  "params": { "patterns": ["stream.*"] }
}
```

**Behavior:**
- No subscription → receive ALL events (default)
- With subscriptions → only matching events delivered
- Patterns use glob matching: `stream.*`, `config.*`, `agent.*`

### Auth & Device Management

| Method | Description |
|--------|-------------|
| `connect` | Authenticate connection (required when `require_auth: true`) |
| `pairing.list` | List pending pairing requests |
| `pairing.approve` | Approve a pairing request by code |
| `pairing.reject` | Reject a pairing request by code |
| `devices.list` | List approved devices |
| `devices.revoke` | Revoke device access |

### Session Management

| Method | Description |
|--------|-------------|
| `sessions.list` | List all sessions |
| `sessions.history` | Get message history for a session |
| `sessions.reset` | Clear session messages |
| `sessions.delete` | Delete a session |

### Channel Management

| Method | Description |
|--------|-------------|
| `channels.list` | List registered channels |
| `channels.status` | Get channel status |
| `channel.start` | Start a channel |
| `channel.stop` | Stop a channel |
| `channel.send` | Send message via channel |

---

## Agent Router Bindings

The `AgentRouter` provides unified routing for both WebSocket `agent.run` calls and inbound channel messages.

### Binding Patterns

```rust
// Register agents
router.register_agent("work").await;
router.register_agent("personal").await;

// Add bindings (pattern → agent_id)
router.add_binding("imessage:*", "personal").await;
router.add_binding("telegram:*", "work").await;
router.add_binding("cli:*", "main").await;
```

### Resolution Order

1. **Explicit session_key** - If provided in request, use directly
2. **Pattern match** - Check bindings for exact match, then wildcard
3. **Default agent** - Fall back to `main`

### Inbound Channel Routing

When `InboundMessageRouter` receives a message:

1. Channel ID is extracted (e.g., `imessage`)
2. `AgentRouter.resolve_agent()` is called with channel
3. Matching binding determines which agent handles the message
4. Session key is generated based on `DmScope` config

**Example flow:**
```
iMessage DM from +1234567890
  → channel: "imessage"
  → binding: "imessage:*" → "personal"
  → session_key: "agent:personal:peer:dm:+1234567890"
```

---

## Event Flow

```
Client A                    Gateway                    Client B
    |                          |                          |
    |-- agent.run ------------>|                          |
    |                          |-- stream.run_accepted -->|
    |                          |-- stream.reasoning ----->|
    |                          |-- stream.response ------>|
    |                          |-- stream.complete ------>|
    |<-- result ---------------|                          |
```

With subscription filtering:
```
Client A (subscribed: stream.*)     Client B (no subscription)
    |                                    |
    |<-- stream.run_accepted            |<-- stream.run_accepted
    |<-- stream.reasoning               |<-- stream.reasoning
    |<-- stream.response                |<-- stream.response
    |                                   |<-- config.reloaded (filtered for A)
```

---

## Configuration

```toml
[gateway]
port = 18789
bind = "127.0.0.1"
require_auth = true   # Enable connection-level auth gate
max_connections = 1000

[routing]
default_agent = "main"
dm_scope = "per-peer"  # main / per-peer / per-channel-peer
auto_start_channels = true
```

---

## Implementation Files

| File | Purpose |
|------|---------|
| `gateway/server.rs` | Connection handling, auth gating, subscription filtering |
| `gateway/handlers/events.rs` | SubscriptionManager, subscribe/unsubscribe handlers |
| `gateway/handlers/agent.rs` | AgentRunManager, status/cancel handlers |
| `gateway/handlers/auth.rs` | Connect handshake, device management |
| `gateway/inbound_router.rs` | Unified routing with AgentRouter |
| `gateway/router.rs` | AgentRouter bindings and resolution |
| `bin/aleph_gateway.rs` | Startup, handler registration |
