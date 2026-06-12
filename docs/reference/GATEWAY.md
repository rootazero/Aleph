# Gateway System

> WebSocket control plane, JSON-RPC protocol, and multi-channel messaging

---

## Overview

The Gateway is Aleph's control plane, providing:
- WebSocket server for real-time communication
- JSON-RPC 2.0 protocol for structured requests
- Multi-interface message routing (Telegram, Discord, iMessage, CLI)
- Event distribution and streaming
- Session management and persistence

**Location**: `src/gateway/`

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Gateway Server                            │
│                  ws://127.0.0.1:18790/ws                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Inbound    │     │   Handler    │     │   Outbound   │    │
│  │   Router     │ ──▶ │   Registry   │ ──▶ │   Emitter    │    │
│  │              │     │              │     │              │    │
│  │ • Parse req  │     │ • Route      │     │ • Stream     │    │
│  │ • Validate   │     │ • Execute    │     │ • Events     │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
│  ┌──────────────┐     ┌──────────────┐     ┌──────────────┐    │
│  │   Session    │     │    Event     │     │  Interface   │    │
│  │   Manager    │     │     Bus      │     │   Registry   │    │
│  │              │     │              │     │              │    │
│  │ • SQLite     │     │ • Pub/Sub    │     │ • Telegram   │    │
│  │ • Compaction │     │ • Topics     │     │ • Discord    │    │
│  │ • History    │     │ • Subscribe  │     │ • iMessage   │    │
│  └──────────────┘     └──────────────┘     └──────────────┘    │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## JSON-RPC Protocol

### Message Format

**Request (Client → Gateway)**:
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-xxx",
  "method": "agent.run",
  "params": {
    "message": "Hello",
    "session_key": "agent:main:main"
  }
}
```

**Response (Gateway → Client)**:
```json
{
  "jsonrpc": "2.0",
  "id": "uuid-xxx",
  "result": {
    "run_id": "run-123",
    "status": "running"
  }
}
```

**Event (Gateway → Client)**:
```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.chunk",
    "data": {
      "run_id": "run-123",
      "content": "Hello! How can I help you?"
    }
  }
}
```

---

## RPC Methods

### Agent Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `agent.run` | Start agent execution | `message`, `session_key`, `thinking?`, `model?` |
| `agent.status` | Get run status | `run_id` |
| `agent.cancel` | Cancel running agent | `run_id` |
| `agent.abort` | Force abort | `run_id` |

### Session Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `session.get` | Get session info | `session_key` |
| `session.list` | List all sessions | `filter?` |
| `session.history` | Get message history | `session_key`, `limit?` |
| `session.compact` | Compress session | `session_key` |
| `session.delete` | Delete session | `session_key` |

### Config Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `config.get` | Get current config | - |
| `config.patch` | Partial update | `patch` (JSON Merge Patch) |
| `config.apply` | Full replace | `config` |
| `config.reload` | Reload from file | - |

### Event Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `events.subscribe` | Subscribe to topic | `pattern` (glob) |
| `events.unsubscribe` | Unsubscribe | `pattern` |
| `events.list` | List subscriptions | - |

### Memory Methods

| Method | Description | Parameters |
|--------|-------------|------------|
| `memory.store` | Store fact | `content`, `metadata?` |
| `memory.search` | Search facts | `query`, `limit?` |
| `memory.delete` | Delete fact | `fact_id` |
| `memory.stats` | Get statistics | - |

### Browser Methods (CDP)

| Method | Description | Parameters |
|--------|-------------|------------|
| `browser.navigate` | Go to URL | `url` |
| `browser.click` | Click element | `selector` |
| `browser.type` | Type text | `selector`, `text` |
| `browser.screenshot` | Take screenshot | `selector?` |
| `browser.evaluate` | Run JavaScript | `script` |

### Other Methods

| Domain | Methods |
|--------|---------|
| `connect` | LAN-trust handshake (no auth; always `operator`) |
| `pairing.*` | `list`, `approve`, `reject` — **channel** sender approval (iMessage/Telegram unknown senders), not device auth |
| `interface.*` | `status`, `config` |
| `mcp.*` | `start`, `stop`, `list`, `call` |
| `plugins.*` | `install`, `uninstall`, `list`, `enable`, `disable` |
| `skills.*` | `list`, `install`, `activate` |
| `runs.*` | `list`, `status`, `wait`, `queue` |
| `models.*` | `list`, `config` |
| `generation.*` | `image`, `video` |
| `cron.*` | `list`, `add`, `remove`, `run` |

---

## Event Topics

Subscribe to events using glob patterns:

| Pattern | Events |
|---------|--------|
| `stream.*` | All streaming events |
| `stream.chunk` | Text chunks |
| `stream.agent_trace` | Structured loop-originated execution trace |
| `stream.tool_start` | Tool execution start |
| `stream.tool_end` | Tool execution end |
| `agent.*` | Agent lifecycle events |
| `agent.started` | Run started |
| `agent.completed` | Run completed |
| `agent.error` | Run error |
| `session.*` | Session events |
| `config.*` | Configuration changes |

---

## Interfaces

**Location**: `src/gateway/interfaces/`

### Interface Trait

```rust
#[async_trait]
pub trait Interface: Send + Sync {
    fn name(&self) -> &str;

    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;

    async fn send_message(
        &self,
        target: &InterfaceTarget,
        message: &str,
    ) -> Result<()>;

    fn is_running(&self) -> bool;
}
```

### Available Interfaces

| Interface | Feature Flag | Description |
|-----------|--------------|-------------|
| CLI | `cli` | Command-line interface |
| Telegram | `telegram` | Telegram Bot API |
| Discord | `discord` | Discord Bot |
| iMessage | (macOS only) | Apple iMessage |
| WebChat | `gateway` | Built-in web chat |

### Interface Configuration

```json5
{
  "interfaces": {
    "telegram": {
      "token": "BOT_TOKEN",
      "allowFrom": ["+1234567890"],
      "groups": {
        "*": { "requireMention": true }
      }
    },
    "discord": {
      "token": "BOT_TOKEN",
      "guilds": ["guild-id-1"]
    }
  }
}
```

---

## Session Routing

**Location**: `src/routing/session_key.rs`

### Session Key Variants

| Variant | Format | Use Case |
|---------|--------|----------|
| **Main** | `agent:main:main` | Cross-channel shared session |
| **DirectMessage** | `agent:main:telegram:dm:user123` | Per-user DM |
| **Group** | `agent:main:discord:group:guild-id` | Group/channel chat |
| **Task** | `agent:main:cron:daily-summary` | Cron jobs, webhooks |
| **Subagent** | `subagent:agent:main:translator` | Sub-agent delegation |
| **Ephemeral** | `agent:main:ephemeral:uuid` | Temporary, no persistence |

### DM Scope Strategies

```rust
pub enum DmScope {
    Main,           // All DMs share main session
    PerPeer,        // Isolated per user (default)
    PerChannelPeer, // Isolated per channel + user
}
```

---

## Session Manager

**Location**: `src/gateway/session_manager.rs`

### Storage Schema

```sql
CREATE TABLE sessions (
    session_key TEXT PRIMARY KEY,
    messages TEXT,           -- JSON array
    created_at INTEGER,
    updated_at INTEGER,
    message_count INTEGER,
    token_count INTEGER
);

CREATE TABLE session_metadata (
    session_key TEXT PRIMARY KEY,
    agent_id TEXT,
    channel TEXT,
    last_compaction INTEGER
);
```

### Compaction

When session exceeds token threshold:

1. Extract key facts from old messages
2. Store facts in memory system
3. Replace old messages with summary
4. Update token count

---

## Security

**LAN-trust model**: the gateway has no authentication step. The trust
boundary is the network boundary — whoever can reach the socket is the
owner. The default bind is `127.0.0.1` (loopback only); set
`[gateway] host = "0.0.0.0"` to open the LAN, which grants every device on
that network complete control over the agent. The only retained protocol
guardrail is the WS Origin check (`src/gateway/origin_policy.rs`), which
blocks public web pages from cross-origin-driving the local daemon. See
[SECURITY.md#auth-ux](SECURITY.md#auth-ux) for the full model.

### Connect handshake

The first frame on a `/ws` connection must be `connect`. The handshake
carries no authentication — it only delivers a state-version baseline and
keepalive policy, and always reports `role: operator`
(`src/gateway/handlers/connect.rs`). Legacy `token` / `device_name` params
from pre-revert clients are accepted and ignored, never validated.

```json
{
  "method": "connect",
  "params": {
    "minProtocol": 1,
    "maxProtocol": 1,
    "client": {
      "id": "macos-app",
      "version": "1.0.0",
      "platform": "macos"
    }
  }
}
```

---

## Hot Reload

**Location**: `src/gateway/hot_reload.rs`

Configuration changes are detected via file watcher:

```
~/.aleph/config.json modified
    │
    ▼
┌─────────────────────────────────┐
│ Debounce (500ms)                │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Parse new config                │
│ Validate against schema         │
└─────────────────────────────────┘
    │
    ▼
┌─────────────────────────────────┐
│ Apply changes                   │
│ • Restart affected interfaces    │
│ • Update routing rules          │
│ • Emit config.changed event     │
└─────────────────────────────────┘
```

---

## HTTP Server

**Location**: `src/gateway/http_server.rs`

Alongside WebSocket, Gateway serves:
- Static files (WebChat UI)
- Liveness probe (`/health`) and readiness probe (`/ready`)
- Metrics endpoint (`/metrics`) — Prometheus text exposition (v0.0.4) of
  request-lifecycle counters, connection gauges, rate-limiter pressure, and a
  request-duration histogram (`aleph_gateway_request_duration_ms`, fed by the
  per-request `elapsed_ms` the metrics middleware already measures); exports
  only aggregate counts (no payloads/secrets), unauthenticated like the probes.
  Implemented in `src/gateway/server/metrics_endpoint.rs` +
  `src/gateway/middleware/latency.rs`.

Abuse protection at WS upgrade: besides the global `max_connections` cap, a
per-IP concurrent-connection cap (`gateway.max_connections_per_ip`, default 64,
`0` disables, loopback exempt) bounds slot-exhaustion — a remote peer
opening many idle sockets.

### Trusted reverse proxies

The IP-keyed abuse protections (per-IP cap, rate limiter, auth-failure lockout)
key off the client IP. Behind a reverse proxy the socket peer is the *proxy*, so
every client collapses to one address. Configure `gateway.trusted_proxies` with
the proxy IPs/CIDRs (e.g. `["10.0.0.0/8", "::1"]`) and, **only** when the socket
peer matches the allowlist, the real client IP is read from `X-Forwarded-For`
(rightmost non-proxy hop). Empty (default) ⇒ the socket peer is used verbatim and
`X-Forwarded-For` is never trusted (it is client-spoofable, so the allowlist is
the whole security boundary). Implemented in `src/gateway/trusted_proxy.rs`.

### Method-level authorization

Under LAN-trust the per-RPC operator-vs-guest authorization gate is **inert**:
every connection is an implicit `operator`, so there is no method-level
barrier on the gateway surface. A classifier survives at the *tool-dispatch*
tier (`src/gateway/method_authz.rs`, consumed by `ScopedToolService`) marking
the self-management tools that mutate Aleph's own config — but because the
caller role is always `operator`, that gate always passes. Limiting *what an
agent may do* (as opposed to *who may connect*) is the job of the per-channel
tool-permission layer (`ScopedToolService`), which is orthogonal to connection
trust and unchanged by the revert.

### Distributed-trace correlation

Each JSON-RPC request resolves a [W3C `traceparent`](https://www.w3.org/TR/trace-context/):
an inbound `params.traceparent` is honoured (its trace id adopted), otherwise a
fresh 128-bit root trace is minted. The dispatch chokepoint opens a `tracing`
span carrying `trace_id`/`span_id`, and the response echoes a `traceparent`
naming the server's span as the parent so a multi-hop call graph stitches
together. This is a lightweight propagation layer (`src/gateway/trace_context.rs`),
**not** an OpenTelemetry integration — the OTel SDK would violate core
minimalism (R3) for what is, given Aleph's own trace persistence and `tracing`
logging, a correlation feature.

> Note: the JSON-RPC middleware chain is built once at server construction and
> cloned per connection. Building it per-connection previously reinstalled the
> global request-state registry on every connect, zeroing the `/metrics`
> request-lifecycle counters and undercounting in-flight requests.

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - Agent loop
- [Security](SECURITY.md) - Exec approval system
