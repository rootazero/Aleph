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
| iMessage | always compiled | Apple iMessage — two transports: **Local** (chat.db poll + AppleScript, macOS-only) and **BlueBubbles** (REST + webhook, any OS). See `src/gateway/interfaces/imessage/`. |
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

**Network boundary + Gateway token**: the trust boundary is the network
boundary, gated by a Gateway-token login wall. The default bind is
`127.0.0.1` (loopback only); loopback is the zero-config operator and needs
no token. Set `[gateway] host = "0.0.0.0"` to open the LAN — a remote device
can then reach the socket but is **walled until it presents a valid
credential** at `connect`. Authorization is resolved by
`connect::resolve_connect_auth` in priority order: (1) loopback ⇒ operator;
(2) a valid **device token** (`aleph-dt-*`, long-lived, bound to a paired
device, SHA-256-hashed at rest); (3) a valid **bootstrap ticket**
(`aleph-bt-*`, 5-min single-use, exchanged for a fresh device token during
onboarding); (4) the legacy shared **Gateway token** (`aleph-<uuid>`,
`SharedTokenManager`, HMAC-hashed, constant-time verified). A valid
credential = full operator authority (identical to local); a missing/invalid
one is walled — the WS dispatch refuses every method but `connect`.
Revocation is token rotation (`gateway.token.rotate`, which also force-closes
live remote sockets) or per-device revoke (`gateway.devices.revoke`). The WS
Origin check (`src/gateway/origin_policy.rs`) additionally blocks public web
pages from cross-origin-driving the local daemon. See
[SECURITY.md#auth-ux](SECURITY.md#auth-ux) for the full model.

### Connect handshake

The first frame on a `/ws` connection must be `connect`. Loopback carries no
credential (zero-config operator). A remote connection presents one of
`device_token`, `bootstrap_ticket`, or `token` (the legacy shared Gateway
token) in `connect` params; `resolve_connect_auth`
(`src/gateway/handlers/connect.rs`) stamps the resolved role (`operator` when
authorized, else `guest`) onto the connection state, and the response echoes
`role` / `authorized` / `needs_token`. A bootstrap-ticket exchange also
returns a freshly minted `device_token` the client persists for subsequent
reconnects. A rejected remote `connect` is recorded in the security audit log
(`AuditEventType::AuthFailure`, bounded by the `Auth`-scope rate limiter).

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
    },
    "device_token": "aleph-dt-…",
    "bootstrap_ticket": "aleph-bt-…"
  }
}
```

> Loopback clients omit the credential fields entirely. A remote client sends
> `bootstrap_ticket` on first pairing (receiving a `device_token` back), then
> `device_token` on every reconnect. `token` (the legacy shared Gateway token)
> is accepted as a fallback.

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

### Reverse proxies (no X-Forwarded-For resolution)

The IP-keyed abuse protections (per-IP connection cap, rate limiter, `Auth`-scope
lockout) key off the **raw socket peer address** (`peer_addr.ip()`, verbatim).
`X-Forwarded-For` / trusted-proxy resolution was removed with the LAN-trust
revert and is **not** reinstated — a `trusted_proxies` key in config is a
silently-ignored legacy field, and there is no `src/gateway/trusted_proxy.rs`.
Keeping the loopback check on the raw peer is deliberate: it means `is_loopback`
(the zero-config-operator grant) can never be forged by a spoofed `X-Forwarded-For`
header.

The trade-off: when the gateway is fronted by a reverse proxy, every client
collapses to the proxy's socket address, so the per-IP protections bound the
*proxy* rather than individual clients. Terminate client-identity trust upstream
(the proxy) if you need per-client limits, and treat the Gateway token as the
transport auth. (Restoring fail-closed, allowlist-gated trusted-proxy XFF
resolution — never letting a forwarded header influence `is_loopback` — is
tracked as a future enhancement.)

### Method-level authorization

The connection-level barrier is the **login wall**: a remote connection that has
not presented a valid Gateway credential is `guest` and may only issue `connect`
(§Connect handshake). Once authorized it is `operator`, identical to local —
there is no finer per-RPC operator-vs-guest tier on the Panel surface. A separate
classifier survives at the *channel* tool-dispatch tier
(`src/gateway/method_authz.rs`, consumed by `ScopedToolService`): the
`inbound_router` caps a chat-tier channel (Telegram/Slack, default `guest`) so it
cannot run Aleph's self-config tools. Limiting *what an agent may do* (vs *who may
connect*) is the job of the per-channel tool-permission layer, orthogonal to
connection trust.

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
