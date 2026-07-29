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

### Factory registration is what makes a channel configurable

`handlers::channel::create_channel_from_config` resolves a configured
`[channels.<type>]` entry through the plugin table in
`interfaces/plugin.rs`, and returns `None` for a type that is not in it —
after which `initialize_channels` logs `Failed to create channel` and continues.
**A `ChannelFactory` that is not registered in `interfaces::register_channel_plugins`
is therefore unreachable, however complete it is.**

The table landed 2026-04-05. Channels added after it registered themselves in the
same commit; the ten that predate it (Slack, Discord, Matrix, Mattermost, Signal,
IRC, Nostr, XMPP, Email, Webhook) were never back-filled and were silently
unconfigurable until 2026-07-26. `imessage` and `cli` are deliberately absent —
iMessage is constructed directly in `initialize_channels`, which `continue`s before
consulting the table, and CLI is not a configurable channel type.

New adapters must be added to `register_channel_plugins` by hand.
`every_configurable_channel_type_is_registered` pins the current set against
regression, but it cannot enumerate `impl ChannelFactory`, so it will not catch a
*future* adapter that forgets to register — adding the name to that list is the
same manual step as the registration.

### Addressing: channel vs conversation

Three different things get called "channel". Keep them apart when reading an error:

| Term | Type | Example | Where it comes from |
|------|------|---------|---------------------|
| **channel** — the transport | `ChannelId` | `"slack"` | `[channels.*]` config, registered into `ChannelRegistry` |
| **conversation** — the room | `ConversationId` | `"C0A1B2C3"` | opaque platform handle |
| **capability** — what this transport can do | `ChannelCapabilities` | `reactions: true` | the adapter's own `capabilities()` |

`OutboundMessage` needs a `ConversationId`, and until 2026-07-26 the only source of
one was an *inbound* message — so the agent could only ever reply where it had been
spoken to. `Channel::list_conversations(query, limit) -> ConversationPage` closes
that: it is the trait's only read, and it reads **routing metadata only** (name, id,
`is_member`), never message content. That line is deliberate — content fetched by a
*pull* would arrive with none of the access control that `inbound_router::check_permission`
(dm/group policy, pairing, allowlists) applies to *pushed* messages.

`ConversationPage` is `{ conversations, warnings }` rather than a bare `Vec` because a
roster lookup has a real **partial** outcome: an app granted `channels:read` but not
`users:read` can list every channel and no people. A bare `Vec` can only say "no match",
which would have the model report that a person does not exist when the truth is that it
was never allowed to look. Slack therefore fails the call only when **both** sweeps fail;
one failing degrades and names the reason. The same field carries page-cap truncation, so
"we stopped looking" is never mistaken for "not in the roster".

Model-facing, this is the read-only `channel_directory` tool feeding `channel_message`.
They are two tools on purpose: `ToolFacts::idempotent` is keyed on the tool **name**
(`registry_adapter::READ_ONLY_TOOLS`), so folding a lookup into non-idempotent
`channel_message` would gate it under the `Ask` exec tier — and a tier never widens,
so there would be no way back.

Slack implements it in `interfaces/slack/directory.rs::ConversationDirectory`
(`conversations.list` + `users.list`, cursor-paginated, 15-minute TTL cache, hard page
cap). The cap is not cosmetic: `ChannelRegistry` holds the channel's **read guard**
across the adapter call, so an unbounded sweep would block the write lock that
`stop_channel` / `restart_channel` need.

### Capability flags are promises

Each `ChannelCapabilities` bool claims the matching optional `Channel` method works.
An adapter that sets one **must** override that method: the default bodies now return
`ChannelError::UnsupportedFeature` naming the adapter, where they used to return
`Ok(())` and let the caller report a success that never happened. Six shipped adapters
were in exactly that state — `msteams.reactions` and `whatsapp.deletion` made
`channel_message` answer `delivered: true` for a no-op. Pinned by
`declared_but_unimplemented_optional_methods_fail_loudly` in `channel.rs`.

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

## Busy Input & Wait Lane

**Location**: `src/gateway/busy_queue/` (lane + delivery loop + config)

Exactly one run may be in flight per session (`execution_engine::SessionRunRegistry`).
A message that arrives while its session is busy is routed by the originating
channel's declared `BusyInputMode`:

| Mode | Behaviour |
|---|---|
| `Steer` (default) | Injected into the live event log; the running loop consumes it at its next turn boundary. |
| `Interrupt` | Cancels the session's run **and its delegated child runs**, then the message restarts as a fresh run via the lane. |
| `Queue` | Leaves the running task alone; the message waits in the lane. |

Anything that cannot be delivered inline joins its session's **FIFO wait lane**.
All three surfaces share the one lane — the inbound router (channels) and both
`aleph-server` RPC handlers (`agent.run`, `chat.send`, via
`busy_queue::spawn_queued_run`) call `busy_queue::register` on the arrival path
and `busy_queue::deliver_with_ticket` inside the spawned delivery task.

Invariants worth preserving:

- **Ticket is taken synchronously on the arrival path**, before the delivery task
  is spawned. Registering inside the task makes lane order follow task
  scheduling instead of arrival order.
- **The lane is a waiting room, not a run registry.** `deliver_with_ticket`
  holds its ticket across the whole `attempt()`, and `attempt()` *is* the agent
  run — so `SessionRunRegistry::try_claim` calls `busy_queue::mark_admitted` to
  withdraw the ticket the moment the run is admitted (the exact mirror of
  `release` → `notify_slot_free`). Without it the running message sits at the
  head of its own lane for the run's entire lifetime, every follow-up parks
  behind the very run it wants to change, and `Steer` / `Interrupt` — which only
  mean anything *while* a sibling runs — silently degrade to `Queue`. The same
  root cause made `/stop` count the message it was stopping among the "queued
  messages dropped" and inflated `busy_queue.total_waiting` by one per busy
  session. FIFO constrains only the messages that are still waiting.
- **Waiters do not poll.** They park on a per-session `Notify` fired by
  `SessionRunRegistry::release` (the authoritative slot-free edge),
  `mark_admitted` (the symmetric "the lane just got shorter" edge), and ticket
  departures. `busy_queue_wake_fallback_secs` is a missed-signal safety net, not
  the delivery latency.
- **`TicketGuard` is the only way in or out.** Its `Drop` is load-bearing: a
  panic while holding the front ticket would otherwise wedge the lane until
  daemon restart.
- **A stale or unknown ticket fails open** — the engine's gate is the real
  authority, so the worst case is one redundant delivery attempt.
- **Report a failure once.** `DeliveryOutcome::Executed(_)` means the run's own
  emitter already sent a `RunError`; only the never-ran outcomes are the
  caller's to report.

Stopping has two granularities: `/stop` purges the whole session lane
(`busy_queue::purge`, wired only in `command_handler::handle_stop` — the
`Interrupt` mode depends on the lane to restart its own message, so
`cancel_session` must not purge), while `chat.abort` reaches a single queued
message by `run_id` (`busy_queue::cancel_queued_run`, wired in
`AgentRunManager::cancel_run` — a queued run has no `active_runs` entry, so the
engine's own cancel cannot see it).

Knobs live in `[execution]`: `busy_queue_max_per_session` (32),
`busy_queue_max_wait_secs` (1800), `busy_queue_wake_fallback_secs` (30),
`max_pending_steering` (16). Backlog is observable via
`gateway.metrics.run_concurrency` → `busy_queue`.

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
live remote sockets) or per-device revoke (`gateway.devices.revoke`, which
drops that device's live sessions to the login wall and then closes their
sockets with WS 4001 `device_revoked`) — both effective immediately, not at the
next handshake. The WS
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
>
> `device_id` is **client-asserted** and the `devices` table is one namespace
> shared with cluster nodes, so the exchange refuses a `device_id` that already
> names a non-Panel device (and `cluster::admit_node` refuses the mirror case).
> Without that guard one ticket buys an operator token the Panel roster cannot
> see and no revoke path can reach.

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

### Channel webhook ingestion

Channels that receive over HTTP POST (`generic webhook`, and future ones)
return a handler from `Channel::webhook_handler()`. `initialize_channels`
collects those after every channel has started and hands the resulting router
to `GatewayServer::set_webhook_routes()`, which merges it in `build_router()`.

- **One port.** Webhook traffic rides the gateway's own listener, so it
  inherits `[gateway] host`, TLS, and `SecurityHeadersLayer`. `WebhookReceiver`
  deliberately owns no listener — the version that bound `0.0.0.0` itself would
  have opened a LAN surface regardless of the configured host.
- **Auth is per-handler HMAC**, not the login wall — an external platform
  cannot present a device token. Same posture as `/health`, `/metrics`, `/a2a`:
  no transport-level auth, no rate limiter (that lives in `MiddlewareChain`,
  on the JSON-RPC/WS path only). The signature also binds no timestamp or
  nonce (unlike Stripe/GitHub's `t=…,v1=…`), so replay protection is
  incidental — it comes only from inbound dedup at
  `src/gateway/inbound_router/dedup.rs`, whose window is **5 minutes**; a
  captured signed request replayed after that re-triggers an agent run. This
  is posture, not a known gap requiring action.
- **`path` is operator-writable**, so a collision with a gateway route would
  panic `Router::merge` at boot. `is_reserved_route()` in `server/mod.rs` skips
  those with a warning. Add every new gateway route to
  `RESERVED_ROUTE_PREFIXES` in the same edit.
- ⚠️ The sink is the channel's **own** `ChannelState::sender()`, not the
  registry's. Going direct to the registry bypasses
  `start_message_forwarder`, the only place inbound traffic stamps
  `health.record_event()` — the channel would receive while health monitoring
  reported it dead.

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - Agent loop
- [Security](SECURITY.md) - Exec approval system
